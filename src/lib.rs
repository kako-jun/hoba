//! hoba — a minimal random number library.
//!
//! Provides a small, dependency-light surface (`random`, `randint`, `choice`,
//! `random_bool`) backed by the operating system's cryptographic random
//! source.
//!
//! When the `mic` feature is enabled (default), a background thread monitors
//! the default audio input for an environmental trigger tone. While that
//! trigger is active, the **least significant bit** of every `random_u64()`
//! result is cleared (mask `0xFFFF_FFFF_FFFF_FFFE`, even-only output). The
//! release default is a single 1–10 Hz infrasound bucket — anywhere in the
//! window flips the trigger (see [`audio::DetectorConfig::release_default`]).
//! The graded 1–4 bit "depth" mask from v0.4.x and earlier was removed in
//! v0.5.0; the mask is binary, matching both the original `notes/dev/hoba.md`
//! design and the binary HOS trigger in *Patlabor: The Movie* (1989).
//! Security-sensitive callers should check [`is_compromised`] and either
//! skip RNG calls or fall back to another source.
//!
//! When the `whiten` feature is also enabled, callers can opt into a
//! whitening pipeline that mixes OS entropy with CPU jitter (and `rdrand`
//! on x86) and passes the mix through BLAKE3 before the trigger mask is
//! applied. The pipeline defaults to off; toggle at runtime via
//! `set_whitening`. The mask is always applied last, so the trigger
//! behavior remains observable through whitened output.
//!
//! Monitoring is opt-in. The background detector is only spawned when the
//! environment variable `HOBA_MONITOR=1` is set in the process. Without
//! that variable hoba is a passive RNG and never opens the microphone.
//!
//! The auto-spawned detector picks up runtime configuration from three
//! optional env vars (no recompile needed):
//! `HOBA_BUCKETS=20,30,40,50` (comma-separated centre Hz),
//! `HOBA_SNR=6` (dB SNR threshold; default 6, replaces v0.3.x's
//! `HOBA_THRESHOLD` raw-power knob), and
//! `HOBA_PEAK_BAND=18:55` (lo:hi Hz for peak reporting and the noise-floor
//! estimate). The legacy `HOBA_BUCKETS=hz:depth,…` form is still parsed for
//! back-compat but the `:depth` portion is ignored — the depth concept was
//! removed in v0.5.0. See [`audio::DetectorConfig`] for the library
//! equivalent.
//!
//! When the `log` feature is enabled (off by default), the detector
//! appends one JSON line to a per-host event log on every quiet → trigger
//! → quiet cycle. Read recent events back via [`recent_events`].
//!
//! For diagnosing a quiet host, two read-only counters complement
//! [`is_compromised`]:
//! [`audio_active`] reports whether the background detector is reading
//! live audio (mic feature), and [`dropped_event_count`] tallies events
//! the on-disk log silently rejected (log feature). Together they
//! distinguish "no triggers happened" from "the monitor never started"
//! from "every write was thrown away".
//!
//! Named after Hoba Eiichi (帆場暎一).

pub mod audio;
#[cfg(feature = "mic")]
pub mod check;

use getrandom::getrandom;
#[cfg(any(feature = "whiten", feature = "mic"))]
use std::sync::atomic::AtomicBool;
#[cfg(feature = "log")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

/// Mask applied to `random_u64` while the detector reports compromise.
/// Clears the least significant bit, forcing every output to be even —
/// restoring the original `notes/dev/hoba.md` design where any single
/// binary decision (gacha index parity, A/B test arm, etc.) collapses to
/// one side. Hidden features built on hoba can lean on this deterministic
/// shape without worrying about graded depth surprising downstream logic.
const COMPROMISE_MASK: u64 = 0xFFFF_FFFF_FFFF_FFFE;

static COMPROMISED: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "whiten")]
static WHITEN_ENABLED: AtomicBool = AtomicBool::new(false);

/// Set to `true` by the detector thread once it has successfully opened the
/// default audio input and is reading samples; cleared back to `false` when
/// the thread exits (mic disappears, panic, etc.). Surfaced via
/// [`audio_active`].
#[cfg(feature = "mic")]
static MIC_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Process-global tally of events the on-disk log silently dropped because
/// some step in `append_event_with_retention` failed (open / lock / temp
/// open / write / flush / fsync / rename). Bumped only inside that function;
/// surfaced via [`dropped_event_count`].
#[cfg(feature = "log")]
static DROPPED_EVENTS: AtomicU64 = AtomicU64::new(0);

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
    // Opt-in gate: with HOBA_MONITOR unset the library never opens the mic
    // and never spawns a background thread. Crucially this check runs BEFORE
    // `INIT.call_once`, so a process that calls `random_u64()` once without
    // the env var, then sets it and calls again, will spawn on the second
    // call (the Once is not consumed by the early-return path).
    if std::env::var("HOBA_MONITOR").as_deref() != Ok("1") {
        return;
    }
    INIT.call_once(|| {
        // If the detector loop ever exits (panic, mic disappears), clear the
        // compromise flag so the library degrades to plain RNG instead of
        // latching the last compromised state. Same goes for MIC_ACTIVE: any
        // callers polling audio_active() should observe the monitor going
        // inactive once the thread is gone.
        struct ClearOnDrop;
        impl Drop for ClearOnDrop {
            fn drop(&mut self) {
                COMPROMISED.store(false, Ordering::Release);
                MIC_ACTIVE.store(false, Ordering::Release);
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
                // Mic opened successfully; signal to audio_active() callers that the
                // monitor is now reading live audio.
                MIC_ACTIVE.store(true, Ordering::Release);
                // If the user pinned a band via env vars, honour it; otherwise
                // fall back to the compile-time default (release / audible-test).
                let mut detector = match audio::DetectorConfig::from_env() {
                    Some(cfg) => audio::Detector::with_config(mic, cfg),
                    None => audio::Detector::with_source(mic),
                };
                #[cfg(feature = "log")]
                let mut active: Option<EventInProgress> = None;
                loop {
                    detector.poll();
                    let triggered = detector.is_compromised();
                    COMPROMISED.store(triggered, Ordering::Release);

                    #[cfg(feature = "log")]
                    {
                        let peak_hz = detector.peak_hz();
                        let peak_db = detector.peak_db();
                        match (active.as_mut(), triggered) {
                            (None, true) => {
                                active = Some(EventInProgress {
                                    start: time::OffsetDateTime::now_utc(),
                                    peak_hz,
                                    peak_db,
                                });
                            }
                            (Some(ev), true) if peak_db > ev.peak_db => {
                                ev.peak_hz = peak_hz;
                                ev.peak_db = peak_db;
                            }
                            (Some(_), false) => {
                                if let Some(ev) = active.take() {
                                    let event = ev.finalize();
                                    append_event_with_retention(&event);
                                }
                            }
                            _ => {}
                        }
                    }

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
/// trigger mask is applied. The mask, when active, clears the LSB so the
/// result is forced even.
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
/// the entropy quality. While `true`, the LSB of every `random_u64()` result
/// is cleared (output forced even, mask `0xFFFF_FFFF_FFFF_FFFE`).
///
/// Calling this lazily starts the background mic monitor on first use
/// (under the `mic` feature).
pub fn is_compromised() -> bool {
    ensure_detector_running();
    COMPROMISED.load(Ordering::Acquire)
}

/// Returns `true` if the background mic monitor is actively reading audio
/// from a live input device.
///
/// Returns `false` when the `mic` feature is off, when `HOBA_MONITOR=1` is
/// unset, when the device failed to open (no device, permission denied,
/// no compatible sample format), or when the monitor thread has not yet
/// had a chance to initialise.
///
/// **Startup race:** the first call after the gating env var is set merely
/// schedules the detector thread; it has not yet run `MicSource::new()`,
/// so this function may briefly return `false`. Callers wanting a
/// definitive answer should poll for ~100 ms after the first call;
/// subsequent calls converge to the actual state.
///
/// Lazily starts the background mic monitor on first use, like
/// [`is_compromised`].
#[cfg(feature = "mic")]
pub fn audio_active() -> bool {
    ensure_detector_running();
    MIC_ACTIVE.load(Ordering::Acquire)
}

/// Stub returned when the `mic` feature is disabled — the mic monitor cannot
/// run, so the answer is always `false`.
#[cfg(not(feature = "mic"))]
pub fn audio_active() -> bool {
    false
}

fn current_mask() -> u64 {
    if COMPROMISED.load(Ordering::Acquire) {
        COMPROMISE_MASK
    } else {
        u64::MAX
    }
}

#[cfg(test)]
#[doc(hidden)]
fn set_compromised_for_test(triggered: bool) {
    COMPROMISED.store(triggered, Ordering::Release);
}

/// One detection event recorded by the background monitor.
///
/// Lifecycle: emitted on the quiet → trigger → quiet transition, so
/// `duration_ms` reflects the entire active span. `peak_hz` and `peak_db`
/// capture the strongest peak observed across the active span.
#[cfg(feature = "log")]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Event {
    /// ISO-8601 UTC timestamp marking the start of the active span.
    pub ts: String,
    /// Frequency in Hz of the peak bin observed during the span.
    pub peak_hz: f32,
    /// Peak magnitude in dBFS observed during the span.
    pub peak_db: f32,
    /// Total wall-clock milliseconds the trigger was active.
    pub duration_ms: u64,
}

/// Default retention window for the on-disk event log, in days.
#[cfg(feature = "log")]
const DEFAULT_RETENTION_DAYS: i64 = 30;

/// Runtime-overridable retention window. Default 30 days; callers can change it
/// via `set_retention_days`. Stored as `i64` so it can feed `time::Duration::days`.
#[cfg(feature = "log")]
static RETENTION_DAYS: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(DEFAULT_RETENTION_DAYS);

/// Sets the rolling retention window for the on-disk event log, in days.
/// Events older than this are pruned on the next append. Pass 0 to keep none.
#[cfg(feature = "log")]
pub fn set_retention_days(days: u32) {
    RETENTION_DAYS.store(days as i64, std::sync::atomic::Ordering::Release);
}

/// Returns the current retention window in days.
#[cfg(feature = "log")]
pub fn retention_days() -> u32 {
    RETENTION_DAYS.load(std::sync::atomic::Ordering::Acquire) as u32
}

/// Returns the number of events the background detector has tried — and
/// failed — to write to the on-disk log over the lifetime of this process.
///
/// The counter is incremented whenever
/// `append_event_with_retention` bails out before the temp file is
/// successfully renamed over `events.jsonl`: missing log directory, lock
/// contention, full disk, fsync error, rename error, etc.
///
/// Useful for diagnosing why a host running with `HOBA_MONITOR=1` is
/// producing a sparser-than-expected log: a non-zero count means the writes
/// reached the function and were rejected by the filesystem path, not that
/// the detector itself failed to fire.
///
/// The counter is process-global and never resets while the process is
/// alive; callers that want a delta should snapshot before and after.
#[cfg(feature = "log")]
pub fn dropped_event_count() -> u64 {
    DROPPED_EVENTS.load(Ordering::Acquire)
}

// Only used inside the detector thread (gated on `mic` & `not(test)`).
// Compiling under `--all-features` plus tests would otherwise warn dead-code.
#[cfg(all(feature = "log", feature = "mic", not(test)))]
struct EventInProgress {
    start: time::OffsetDateTime,
    peak_hz: f32,
    peak_db: f32,
}

#[cfg(all(feature = "log", feature = "mic", not(test)))]
impl EventInProgress {
    fn finalize(self) -> Event {
        use time::format_description::well_known::Iso8601;
        let now = time::OffsetDateTime::now_utc();
        let duration_ms = (now - self.start).whole_milliseconds().max(0) as u64;
        Event {
            ts: self.start.format(&Iso8601::DEFAULT).unwrap_or_default(),
            peak_hz: self.peak_hz,
            peak_db: self.peak_db,
            duration_ms,
        }
    }
}

/// Returns events recorded within the given window from the on-disk log.
///
/// The window is measured backwards from the current UTC time. Events
/// that fail to parse or have malformed timestamps are skipped silently.
/// Returns an empty `Vec` if the log file does not exist.
#[cfg(feature = "log")]
pub fn recent_events(within: std::time::Duration) -> Vec<Event> {
    use time::format_description::well_known::Iso8601;
    let secs = within.as_secs().min(i64::MAX as u64) as i64;
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::seconds(secs);
    read_all_events()
        .into_iter()
        .filter(|e| {
            time::OffsetDateTime::parse(&e.ts, &Iso8601::DEFAULT)
                .map(|t| t >= cutoff)
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(feature = "log")]
fn log_path() -> Option<std::path::PathBuf> {
    // Hidden override for tests and embedding scenarios. Not part of the
    // public API contract; do not document.
    if let Ok(custom) = std::env::var("HOBA_LOG_PATH_OVERRIDE") {
        let p = std::path::PathBuf::from(custom);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).ok()?;
        }
        return Some(p);
    }

    let dir = if cfg!(target_os = "macos") {
        // dirs::data_dir() on macOS returns ~/Library/Application Support
        dirs::data_dir().map(|d| d.join("dev.kako-jun.hoba"))
    } else if cfg!(windows) {
        // dirs::data_dir() on Windows returns %APPDATA%
        dirs::data_dir().map(|d| d.join("hoba"))
    } else {
        // Linux & other XDG-style platforms: ~/.local/state/hoba
        dirs::state_dir().map(|d| d.join("hoba"))
    }?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("events.jsonl"))
}

#[cfg(feature = "log")]
fn read_all_events() -> Vec<Event> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Event>(line).ok())
        .collect()
}

#[cfg(feature = "log")]
fn append_event_with_retention(event: &Event) {
    use std::io::Write;
    use time::format_description::well_known::Iso8601;

    let Some(path) = log_path() else {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        return;
    };

    // We coordinate writers through a sibling lock file with a stable path.
    // The canonical `events.jsonl` is replaced by atomic rename below, so its
    // inode keeps changing — flock(2) is per-open-file-description, so locking
    // a path that gets renamed out from under us would not actually serialise
    // a second writer that opens the new inode after the rename. A dedicated
    // sibling that is *only* opened-and-locked, never renamed, fixes that.
    let mut lock_path = path.clone();
    let lock_name = match path.file_name() {
        Some(n) => {
            let mut s = n.to_os_string();
            s.push(".lock");
            s
        }
        None => std::ffi::OsString::from("events.jsonl.lock"),
    };
    lock_path.set_file_name(&lock_name);

    let Ok(lock_file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
    else {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        return;
    };
    // `lock_file` is owned for the duration of the critical section below; the
    // lock is released when the handle is dropped. Routed explicitly through
    // the trait so it keeps working under future MSRV bumps where the inherent
    // `std::fs::File::lock` shadows the trait method.
    if fs4::FileExt::lock(&lock_file).is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        return;
    }

    // Read the canonical file. Missing file is fine (first write); any other
    // I/O failure (transient EIO, permission flap) must abort — silently
    // discarding the read and rewriting the tmp would clobber the existing
    // history with just the new event, breaking the crash-safety contract.
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => {
            DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
            return;
        }
    };

    let cutoff = time::OffsetDateTime::now_utc()
        - time::Duration::days(RETENTION_DAYS.load(std::sync::atomic::Ordering::Acquire));
    let mut new_contents = String::with_capacity(contents.len() + 256);
    for line in contents.lines() {
        let keep = serde_json::from_str::<Event>(line)
            .ok()
            .and_then(|e| time::OffsetDateTime::parse(&e.ts, &Iso8601::DEFAULT).ok())
            .map(|t| t >= cutoff)
            .unwrap_or(false);
        if keep {
            new_contents.push_str(line);
            new_contents.push('\n');
        }
    }
    let new_line = serde_json::to_string(event).unwrap_or_default();
    if !new_line.is_empty() {
        new_contents.push_str(&new_line);
        new_contents.push('\n');
    }

    // Sibling temp file so the rename is guaranteed to land on the same
    // filesystem (POSIX rename(2) and Win32 MoveFileEx atomic rename both
    // require this). Tagging with the current pid avoids confusing temps left
    // behind by a previously crashed writer; concurrent writers within the
    // same process are already serialised by the exclusive lock above.
    let pid = std::process::id();
    let mut tmp_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("events.jsonl"));
    tmp_name.push(format!(".tmp.{pid}"));
    let tmp_path = match path.parent() {
        Some(parent) => parent.join(&tmp_name),
        None => std::path::PathBuf::from(&tmp_name),
    };

    // The `lock_file` handle is held to the end of the function; its Drop
    // releases the flock automatically. Adding explicit unlock calls in each
    // error tail is redundant and clutters the failure paths.
    let Ok(mut tmp) = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp_path)
    else {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        return;
    };
    if tmp.write_all(new_contents.as_bytes()).is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if tmp.flush().is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    if tmp.sync_data().is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }
    // Drop the temp handle before renaming. On Windows, leaving an open handle
    // on the source can interfere with rename; closing it first is portable.
    drop(tmp);
    if std::fs::rename(&tmp_path, &path).is_err() {
        DROPPED_EVENTS.fetch_add(1, Ordering::AcqRel);
        let _ = std::fs::remove_file(&tmp_path);
    }
    drop(lock_file);
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

    /// Compromise mask: every output is even (LSB cleared) while the
    /// detector reports compromise. Pin the binary v0.5.0 contract — the
    /// graded depth from v0.4.x is gone.
    #[test]
    fn random_u64_lsb_cleared_when_compromised() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_for_test(true);
        for _ in 0..1000 {
            let r = random_u64();
            assert_eq!(
                r & 1,
                0,
                "compromise mask must clear the LSB (always even); got {r:#066b}"
            );
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

    /// Each preset bucket centre should fire the detector and, while it
    /// fires, push every `random_u64` output to even.
    #[test]
    fn random_u64_forced_even_under_each_preset_bucket() {
        let _guard = TEST_MUTEX.lock().unwrap();
        for &freq in &audio::BUCKETS {
            let mut detector = audio::Detector::with_source(audio::SineSource::new(freq, 0.5));
            for _ in 0..12 {
                detector.poll();
            }
            assert!(
                detector.is_compromised(),
                "{freq} Hz tone should trip the detector"
            );
            set_compromised_for_test(detector.is_compromised());
            for _ in 0..1000 {
                assert_eq!(
                    random_u64() & 1,
                    0,
                    "compromised → output must be even ({freq} Hz)"
                );
            }
        }
        set_compromised_for_test(false);
    }

    #[test]
    fn is_compromised_reflects_global_flag() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_compromised_for_test(true);
        assert!(is_compromised());
        set_compromised_for_test(false);
        assert!(!is_compromised());
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_passes_lsb_clearing_through() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(true);
        set_compromised_for_test(true);
        for _ in 0..1000 {
            let r = random_u64();
            assert_eq!(
                r & 1,
                0,
                "whitening must not bypass the compromise mask; got {r:#066b}"
            );
        }
        set_compromised_for_test(false);
        set_whitening(false);
    }

    #[cfg(feature = "whiten")]
    #[test]
    fn whiten_monobit_balance() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_whitening(true);
        set_compromised_for_test(false);
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
        set_compromised_for_test(false);
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
        set_compromised_for_test(false);
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

#[cfg(all(test, feature = "log"))]
mod log_tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use time::format_description::well_known::Iso8601;

    // Tests in this module mutate process-global env vars (HOBA_LOG_PATH_OVERRIDE)
    // and a shared filesystem path; serialise them so concurrent test threads do
    // not stomp on each other.
    static LOG_TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn unique_tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hoba-test-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn now_iso() -> String {
        time::OffsetDateTime::now_utc()
            .format(&Iso8601::DEFAULT)
            .unwrap()
    }

    #[test]
    fn event_serializes_roundtrip() {
        let e = Event {
            ts: "2026-05-02T13:42:11.000000000Z".into(),
            peak_hz: 19120.5,
            peak_db: -32.1,
            duration_ms: 850,
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ts, e.ts);
        assert_eq!(back.duration_ms, e.duration_ms);
        assert!((back.peak_hz - e.peak_hz).abs() < 1e-3);
        assert!((back.peak_db - e.peak_db).abs() < 1e-3);
    }

    #[test]
    fn append_event_creates_file_and_recent_events_finds_it() {
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("append");
        let path = dir.join("events.jsonl");
        std::fs::remove_file(&path).ok();
        // SAFETY: env access is serialised by LOG_TEST_MUTEX above.
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        let event = Event {
            ts: now_iso(),
            peak_hz: 19_000.0,
            peak_db: -20.0,
            duration_ms: 500,
        };
        append_event_with_retention(&event);

        let events = recent_events(Duration::from_secs(60));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].duration_ms, 500);

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_events_returns_empty_when_log_missing() {
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("missing");
        let path = dir.join("does-not-exist.jsonl");
        std::fs::remove_file(&path).ok();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        let events = recent_events(Duration::from_secs(60));
        assert!(events.is_empty());

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn recent_events_filters_by_window() {
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("window");
        let path = dir.join("events.jsonl");
        std::fs::remove_file(&path).ok();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        // Old event: 2 hours ago. Should be filtered out by a 60s window.
        let old_ts = (time::OffsetDateTime::now_utc() - time::Duration::hours(2))
            .format(&Iso8601::DEFAULT)
            .unwrap();
        append_event_with_retention(&Event {
            ts: old_ts,
            peak_hz: 19_000.0,
            peak_db: -25.0,
            duration_ms: 100,
        });
        // Fresh event: now. Should be returned.
        append_event_with_retention(&Event {
            ts: now_iso(),
            peak_hz: 20_000.0,
            peak_db: -10.0,
            duration_ms: 200,
        });

        let events = recent_events(Duration::from_secs(60));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].duration_ms, 200);

        let all = recent_events(Duration::from_secs(60 * 60 * 24));
        assert_eq!(all.len(), 2);

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_prunes_events_past_retention() {
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("retention");
        let path = dir.join("events.jsonl");
        std::fs::remove_file(&path).ok();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        // Pre-seed file with an ancient event (60 days ago) — past 30-day retention.
        let ancient_ts = (time::OffsetDateTime::now_utc() - time::Duration::days(60))
            .format(&Iso8601::DEFAULT)
            .unwrap();
        let ancient = Event {
            ts: ancient_ts,
            peak_hz: 19_000.0,
            peak_db: -25.0,
            duration_ms: 100,
        };
        std::fs::write(
            &path,
            format!("{}\n", serde_json::to_string(&ancient).unwrap()),
        )
        .unwrap();

        // Trigger an append; this should drop the ancient line.
        append_event_with_retention(&Event {
            ts: now_iso(),
            peak_hz: 20_000.0,
            peak_db: -10.0,
            duration_ms: 200,
        });

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "ancient event should be pruned, kept = {lines:?}"
        );
        let parsed: Event = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.duration_ms, 200);

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn appends_are_concurrency_safe_under_lock() {
        // Spawn N threads that each append M events to the same file. The
        // exclusive file lock should serialise them so the final file has N*M
        // valid JSONL lines and no corruption.
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("concurrent");
        let path = dir.join("events.jsonl");
        std::fs::remove_file(&path).ok();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        let n_threads = 4;
        let per_thread = 5;
        let mut handles = Vec::new();
        for t in 0..n_threads {
            handles.push(std::thread::spawn(move || {
                for i in 0..per_thread {
                    let ev = Event {
                        ts: now_iso(),
                        peak_hz: 19_000.0 + (t * 10 + i) as f32,
                        peak_db: -30.0,
                        duration_ms: 100,
                    };
                    append_event_with_retention(&ev);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        let mut count = 0usize;
        for line in contents.lines() {
            let _: Event = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("corrupt jsonl line {line:?}: {e}"));
            count += 1;
        }
        assert_eq!(count, n_threads * per_thread);

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dropped_count_zero_on_normal_append() {
        // The counter is process-global and shared across all tests in the
        // same binary; snapshot before, append a known number of events, and
        // assert the delta is 0. Comparing against raw 0 would fail under
        // any other test in this module that intentionally exercises a
        // failure path.
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = unique_tmp_dir("dropped-zero");
        let path = dir.join("events.jsonl");
        std::fs::remove_file(&path).ok();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        let before = dropped_event_count();
        for i in 0..5 {
            append_event_with_retention(&Event {
                ts: now_iso(),
                peak_hz: 19_000.0 + i as f32,
                peak_db: -25.0,
                duration_ms: 100,
            });
        }
        let after = dropped_event_count();
        assert_eq!(
            after - before,
            0,
            "normal appends should not bump the dropped counter (before={before}, after={after})"
        );

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dropped_count_increments_on_unwritable_path() {
        // /dev/null is not a directory, so create_dir_all on a path beneath
        // it always fails. log_path() returns None in that case and
        // append_event_with_retention takes its first dropped-counter branch.
        // This is a deterministic failure across Linux and macOS CI.
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let unwritable = std::path::PathBuf::from("/dev/null/hoba-not-a-dir/events.jsonl");
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &unwritable);

        let before = dropped_event_count();
        append_event_with_retention(&Event {
            ts: now_iso(),
            peak_hz: 19_000.0,
            peak_db: -25.0,
            duration_ms: 100,
        });
        let after = dropped_event_count();
        assert_eq!(
            after - before,
            1,
            "an unwritable log path should bump the dropped counter exactly once \
             (before={before}, after={after})"
        );

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
    }

    #[test]
    fn dropped_count_increments_on_read_failure_keeps_history() {
        // If `events.jsonl` exists but is unreadable (here: replaced by a
        // directory of the same name so read_to_string returns EISDIR), we
        // must abort and bump the counter — NOT silently treat the read
        // failure as "missing file" and rewrite the log with only the new
        // event. The latter would clobber any existing history.
        let _guard = LOG_TEST_MUTEX.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("hoba-test-readfail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        // Make `path` itself a directory so `read_to_string(&path)` returns
        // an error other than NotFound.
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var("HOBA_LOG_PATH_OVERRIDE", &path);

        let before = dropped_event_count();
        append_event_with_retention(&Event {
            ts: now_iso(),
            peak_hz: 19_000.0,
            peak_db: -25.0,
            duration_ms: 100,
        });
        let after = dropped_event_count();
        assert_eq!(
            after - before,
            1,
            "read failure on existing log path must bump the dropped counter \
             exactly once (before={before}, after={after})"
        );
        assert!(
            path.is_dir(),
            "events.jsonl path must remain a directory — abort before rename \
             is the contract that preserves history on transient I/O failures"
        );

        std::env::remove_var("HOBA_LOG_PATH_OVERRIDE");
        std::fs::remove_dir_all(&dir).ok();
    }
}
