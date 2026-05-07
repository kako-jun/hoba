//! Audio input abstraction and a sine-wave test fixture.
//!
//! ```no_run
//! use hoba::audio::{Detector, SineSource};
//!
//! // 1 Hz sits inside the production trigger band (1–10 Hz infrasound).
//! // Note: real consumer speakers cannot reproduce this — see
//! // [`DetectorConfig::release_default`] for the rationale.
//! let mut detector = Detector::with_source(SineSource::new(1.0, 0.5));
//! for _ in 0..12 {
//!     detector.poll();
//! }
//! assert!(detector.is_compromised());
//! ```

use core::f64::consts::TAU;

/// A pull-based source of mono `f32` audio samples.
pub trait AudioSource {
    /// Returns the source's sample rate in Hz.
    fn sample_rate(&self) -> u32;

    /// Fills `buf` with samples and returns the number of samples written.
    fn read(&mut self, buf: &mut [f32]) -> usize;
}

/// A deterministic sine-wave fixture used by tests and examples.
///
/// No Nyquist check is performed: setting `frequency >= sample_rate / 2`
/// will alias silently. This is intentional so the fixture can stand in
/// for ultrasonic test tones without extra wrapping.
#[derive(Debug, Clone)]
pub struct SineSource {
    frequency: f32,
    amplitude: f32,
    sample_rate: u32,
    phase: f64,
}

impl SineSource {
    /// Creates a new sine source at the given frequency (Hz) and amplitude, defaulting to 48 kHz.
    pub fn new(frequency: f32, amplitude: f32) -> Self {
        Self::with_sample_rate(frequency, amplitude, 48_000)
    }

    /// Creates a new sine source at the given frequency (Hz), amplitude, and sample rate (Hz).
    pub fn with_sample_rate(frequency: f32, amplitude: f32, sample_rate: u32) -> Self {
        Self {
            frequency,
            amplitude,
            sample_rate,
            phase: 0.0,
        }
    }
}

impl AudioSource for SineSource {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        let step = TAU * f64::from(self.frequency) / f64::from(self.sample_rate);
        for slot in buf.iter_mut() {
            *slot = (self.phase.sin() as f32) * self.amplitude;
            self.phase += step;
        }
        self.phase = self.phase.rem_euclid(TAU);
        buf.len()
    }
}

/// Default FFT window for the audible-test preset and any caller that does not
/// override [`DetectorConfig::fft_size`]. 2048 samples = bin width 23.4 Hz at
/// 48 kHz; comfortably resolves kHz-scale buckets.
pub(crate) const DEFAULT_FFT_SIZE: usize = 2048;
/// FFT window for the infrasound release default. 65536 samples ≈ 1.37 s at
/// 48 kHz, giving a bin width of ~0.73 Hz — fine enough to discriminate the
/// 1 / 3 / 5 / 10 Hz buckets even with rectangular-window leakage. Stepping
/// down to 32768 collapses the 1 Hz and 3 Hz bins; stepping up to 131072
/// doubles per-poll FFT cost without buying meaningful selectivity at the
/// chosen bucket spacing.
pub(crate) const RELEASE_FFT_SIZE: usize = 65536;
/// Per-bucket center frequency (Hz) and corresponding mask depth (1–4 low bits cleared).
/// Kept `pub(crate)` as a test fixture pinning the current default band; live
/// detection logic reads from [`DetectorConfig`], so production code should
/// reach the same values via [`DetectorConfig::release_default`] /
/// [`DetectorConfig::audible_test`].
#[cfg(not(feature = "audible-test"))]
#[allow(dead_code)]
pub(crate) const BUCKETS: [(f32, u8); 4] = [(1.0, 1), (3.0, 2), (5.0, 3), (10.0, 4)];
/// Audible-band stand-in for development. Most laptop and consumer speakers
/// reproduce 1–2.5 kHz cleanly, so the detector can be exercised through a
/// tone generator without specialized hardware.
#[cfg(feature = "audible-test")]
#[allow(dead_code)]
pub(crate) const BUCKETS: [(f32, u8); 4] = [(1_000.0, 1), (1_500.0, 2), (2_000.0, 3), (2_500.0, 4)];
/// Default per-bucket half-width in Hz, used when a [`DetectorConfig`] does
/// not specify [`DetectorConfig::bucket_half_width_hz`] and as the margin in
/// [`DetectorConfig::peak_band_from_buckets`]. Sized for kHz-scale buckets
/// (the audible-test preset and historical ultrasonic overrides); the
/// infrasound release default narrows this to 1.0 Hz so 1 / 3 / 5 / 10 Hz
/// buckets do not pollute each other.
const DEFAULT_BUCKET_HALF_WIDTH_HZ: f32 = 100.0;

/// dBFS value reported by [`Detector::peak_db`] when no signal is present
/// (peak magnitude is zero). Callers comparing against silence should use
/// this constant rather than the literal.
pub const SILENCE_DB: f32 = -100.0;
/// ~128 ms at 48 kHz with 2048-sample windows. Boundary case (2 windows) covered by tests.
const STREAK_TO_FLIP: u32 = 3;
/// Internal sample rate the audible-test config is calibrated against. Detector logic
/// recomputes bin sizes from the live source's reported sample rate, so this is
/// only a documentation hint for env-var / CLI overrides.
const DEFAULT_SAMPLE_RATE: u32 = 48_000;
/// Sample rate hint for the infrasound release default. Most consumer mics
/// run at 44.1 or 48 kHz; either is fine — what matters is the FFT window
/// length in seconds, not the rate. 44.1 kHz makes the docs honest about
/// which rate "civilian" devices typically deliver.
const INFRASOUND_SAMPLE_RATE: u32 = 44_100;

/// Runtime-tunable detector knobs.
///
/// The default behaviour of [`Detector::new`] / [`Detector::with_source`] is
/// equivalent to `DetectorConfig::release_default()` (or
/// `DetectorConfig::audible_test()` when the `audible-test` cargo feature is
/// on). Pass a custom config to [`Detector::with_config`] when you want to
/// monitor a different band (sub-bass, ultrasonic at a non-standard center,
/// CI-friendly tones, …) without a recompile.
///
/// Construction conventions:
/// - `buckets`: 1–N entries of `(center_hz, depth)`. `depth` is the LSB-clear
///   count reported when that bucket dominates. Empty `buckets` makes the
///   detector unable to trigger; the constructor accepts it but the resulting
///   detector permanently reports `depth() == 0`.
/// - `power_threshold`: minimum raw band power (post-FFT) for the dominant
///   bucket to count. Same units as the legacy const.
/// - `peak_band_hz`: `(lo, hi)` window scanned for `peak_hz` / `peak_db`. Use
///   [`DetectorConfig::peak_band_from_buckets`] to derive a sensible default
///   covering all bucket centers plus margin.
/// - `sample_rate`: documentation hint only; the live source's reported rate
///   is what the FFT uses.
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// `(center_hz, depth)` pairs that define the trigger buckets.
    pub buckets: Vec<(f32, u8)>,
    /// Minimum raw band power required for the dominant bucket to flip the trigger.
    pub power_threshold: f32,
    /// `(lo, hi)` Hz bounds for the peak-search band reported via `peak_hz` / `peak_db`.
    pub peak_band_hz: (f32, f32),
    /// Documentation hint — the live source dictates actual FFT bin width.
    pub sample_rate: u32,
    /// FFT window length in samples. Larger = finer Hz resolution at the cost
    /// of latency and per-poll work. The infrasound release default uses 65536
    /// (~1.4 s at 48 kHz) so 1 / 3 / 5 / 10 Hz buckets are separable; the
    /// audible-test preset stays at 2048 since 23 Hz bin width is plenty for
    /// 1–2.5 kHz tones. Must be a power of two for `rustfft` planner efficiency,
    /// though the planner accepts other sizes.
    pub fft_size: usize,
    /// Half-width of each bucket window in Hz; band power for a bucket is
    /// summed over `center ± this`. Tighten for closely-spaced buckets
    /// (infrasound default uses 1.0 so 1 vs 3 Hz buckets stay distinct);
    /// widen when the source has poor frequency stability.
    pub bucket_half_width_hz: f32,
}

impl DetectorConfig {
    /// Production default: **infrasound 1 / 3 / 5 / 10 Hz**, all below the
    /// human audibility floor (~20 Hz). Inspired by the HOS ("バビロンプロジェクト")
    /// in *Patlabor: The Movie* (1989), which fires from low-frequency wind
    /// resonance rather than anything speakers can play. A consumer playback
    /// chain physically cannot trigger this default — the detector waits for
    /// real environmental energy: earthquakes, typhoon gusts, heavy machinery,
    /// large HVAC, subway passes. That "doesn't fire in everyday life" is the
    /// whole point.
    ///
    /// Threshold and FFT window are sized together: the 65536-sample window
    /// (~1.4 s at 48 kHz) gives ~0.73 Hz bin resolution, just fine enough to
    /// keep the buckets distinct under rectangular-window leakage.
    ///
    /// Callers who explicitly want a different band (sub-bass HVAC, the old
    /// ultrasonic 19–21 kHz placeholder, audible CI tones) can still get
    /// there without a recompile via `HOBA_BUCKETS` or
    /// [`Detector::with_config`].
    pub fn release_default() -> Self {
        Self {
            buckets: vec![(1.0, 1), (3.0, 2), (5.0, 3), (10.0, 4)],
            power_threshold: 10_000.0,
            peak_band_hz: (0.5, 12.0),
            sample_rate: INFRASOUND_SAMPLE_RATE,
            fft_size: RELEASE_FFT_SIZE,
            bucket_half_width_hz: 1.0,
        }
    }

    /// Audible-band preset for development, CI, and live demos. Reachable
    /// without the `audible-test` cargo feature: pass this config to
    /// [`Detector::with_config`] from a release build.
    pub fn audible_test() -> Self {
        Self {
            buckets: vec![(1_000.0, 1), (1_500.0, 2), (2_000.0, 3), (2_500.0, 4)],
            power_threshold: 100.0,
            peak_band_hz: (900.0, 2_600.0),
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: DEFAULT_FFT_SIZE,
            bucket_half_width_hz: DEFAULT_BUCKET_HALF_WIDTH_HZ,
        }
    }

    /// Reads `HOBA_BUCKETS` / `HOBA_THRESHOLD` / `HOBA_PEAK_BAND` from the
    /// process environment and merges them with [`Self::release_default`] (or
    /// [`Self::audible_test`] when the `audible-test` feature is on).
    ///
    /// Returns `None` if no relevant env vars are set, so callers can skip
    /// the override path entirely. Returns `Some(config)` if at least one
    /// override is present; unrecognised values fall back to the matching
    /// default field. When `HOBA_DEBUG=1` is set, parse failures emit a
    /// single line on stderr; otherwise failures are silent (consistent with
    /// hoba's library-fails-quietly contract).
    pub fn from_env() -> Option<Self> {
        let buckets_raw = std::env::var("HOBA_BUCKETS").ok();
        let threshold_raw = std::env::var("HOBA_THRESHOLD").ok();
        let peak_band_raw = std::env::var("HOBA_PEAK_BAND").ok();
        if buckets_raw.is_none() && threshold_raw.is_none() && peak_band_raw.is_none() {
            return None;
        }

        let mut config = base_default();
        let debug = std::env::var("HOBA_DEBUG").as_deref() == Ok("1");

        if let Some(s) = buckets_raw.as_deref() {
            match parse_buckets_env(s) {
                Ok(buckets) if !buckets.is_empty() => {
                    config.buckets = buckets;
                    // If the user did not also supply a peak band, recompute it
                    // from the new buckets so peak_hz reporting stays meaningful.
                    if peak_band_raw.is_none() {
                        config.peak_band_hz = Self::peak_band_from_buckets(&config.buckets);
                    }
                }
                Ok(_) => {
                    if debug {
                        eprintln!(
                            "hoba: HOBA_BUCKETS parsed but yielded zero entries; using default"
                        );
                    }
                }
                Err(e) => {
                    if debug {
                        eprintln!("hoba: HOBA_BUCKETS invalid ({e}); using default");
                    }
                }
            }
        }

        if let Some(s) = threshold_raw.as_deref() {
            match s.trim().parse::<f32>() {
                Ok(v) if v.is_finite() && v >= 0.0 => config.power_threshold = v,
                Ok(_) | Err(_) => {
                    if debug {
                        eprintln!("hoba: HOBA_THRESHOLD invalid ({s:?}); using default");
                    }
                }
            }
        }

        if let Some(s) = peak_band_raw.as_deref() {
            match parse_peak_band_env(s) {
                Ok(band) => config.peak_band_hz = band,
                Err(e) => {
                    if debug {
                        eprintln!("hoba: HOBA_PEAK_BAND invalid ({e}); using default");
                    }
                }
            }
        }

        Some(config)
    }

    /// Derives a `(lo, hi)` peak-search band from a bucket list: spans the
    /// min/max bucket center with a small margin (1.5× the per-bucket
    /// half-width on each side) to absorb FFT leakage at the edges. Returns
    /// `(0.0, 0.0)` for an empty bucket list.
    pub fn peak_band_from_buckets(buckets: &[(f32, u8)]) -> (f32, f32) {
        if buckets.is_empty() {
            return (0.0, 0.0);
        }
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &(c, _) in buckets {
            if c < lo {
                lo = c;
            }
            if c > hi {
                hi = c;
            }
        }
        let margin = DEFAULT_BUCKET_HALF_WIDTH_HZ * 1.5;
        ((lo - margin).max(0.0), hi + margin)
    }
}

impl Default for DetectorConfig {
    fn default() -> Self {
        base_default()
    }
}

#[cfg(not(feature = "audible-test"))]
fn base_default() -> DetectorConfig {
    DetectorConfig::release_default()
}

#[cfg(feature = "audible-test")]
fn base_default() -> DetectorConfig {
    DetectorConfig::audible_test()
}

fn parse_buckets_env(s: &str) -> Result<Vec<(f32, u8)>, String> {
    let mut out = Vec::new();
    for raw in s.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let (hz_s, depth_s) = part
            .split_once(':')
            .ok_or_else(|| format!("'{part}' is missing ':<depth>'"))?;
        let hz: f32 = hz_s
            .trim()
            .parse()
            .map_err(|e| format!("center_hz '{hz_s}' invalid: {e}"))?;
        if !hz.is_finite() || hz <= 0.0 {
            return Err(format!("center_hz must be positive and finite: {hz_s}"));
        }
        let depth: u8 = depth_s
            .trim()
            .parse()
            .map_err(|e| format!("depth '{depth_s}' invalid: {e}"))?;
        if !(1..=4).contains(&depth) {
            return Err(format!("depth must be 1..=4: {depth_s}"));
        }
        out.push((hz, depth));
    }
    Ok(out)
}

fn parse_peak_band_env(s: &str) -> Result<(f32, f32), String> {
    let (lo_s, hi_s) = s
        .split_once(':')
        .ok_or_else(|| format!("expected 'lo:hi', got '{s}'"))?;
    let lo: f32 = lo_s
        .trim()
        .parse()
        .map_err(|e| format!("lo '{lo_s}' invalid: {e}"))?;
    let hi: f32 = hi_s
        .trim()
        .parse()
        .map_err(|e| format!("hi '{hi_s}' invalid: {e}"))?;
    if !lo.is_finite() || !hi.is_finite() || lo < 0.0 || hi <= lo {
        return Err(format!("require 0 <= lo < hi, got ({lo}, {hi})"));
    }
    Ok((lo, hi))
}

/// Pulls samples from an [`AudioSource`] and detects trigger tones inside the
/// configured bucket band.
///
/// Note: `Debug` is hand-implemented because the boxed FFT trait object is
/// not itself `Debug`. `Clone` is not implemented because the FFT planner
/// state is not naturally cloneable; sources that hold non-cloneable
/// resources (e.g. a future cpal stream handle in #3) compose cleanly with
/// this.
///
/// The trigger band is approximate. Rectangular-window leakage means tones
/// just outside the bucket centers (within roughly half the bin width) still
/// flip the flag. For "is the environment in the trigger band" this is
/// desirable; for narrowband classification, post-process the FFT
/// separately.
pub struct Detector<S: AudioSource> {
    source: S,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<rustfft::num_complex::Complex<f32>>,
    pull_buf: Vec<f32>,
    high_streak: u32,
    low_streak: u32,
    depth: u8,
    last_peak_hz: f32,
    last_peak_db: f32,
    config: DetectorConfig,
}

impl<S: AudioSource + core::fmt::Debug> core::fmt::Debug for Detector<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Detector")
            .field("source", &self.source)
            .field("high_streak", &self.high_streak)
            .field("low_streak", &self.low_streak)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

impl<S: AudioSource> Detector<S> {
    /// Constructs a detector that reads from `source`, using the compile-time
    /// default configuration ([`DetectorConfig::release_default`], or
    /// [`DetectorConfig::audible_test`] when the `audible-test` feature is on).
    pub fn with_source(source: S) -> Self {
        Self::with_config(source, base_default())
    }

    /// Constructs a detector that reads from `source` using the supplied
    /// runtime configuration. Prefer this when you need a band other than
    /// the compile-time default — sub-bass HVAC monitoring, a non-standard
    /// ultrasonic center, an audible test tone in a release build, etc.
    pub fn with_config(source: S, config: DetectorConfig) -> Self {
        // Defensive fallback: a zero or absurdly small fft_size would crash
        // the FFT planner. Clamp to DEFAULT_FFT_SIZE so misconfigured input
        // degrades to the audible-test window rather than panicking.
        let fft_size = if config.fft_size >= 64 {
            config.fft_size
        } else {
            DEFAULT_FFT_SIZE
        };
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        Self {
            source,
            fft,
            scratch: vec![rustfft::num_complex::Complex::new(0.0, 0.0); fft_size],
            pull_buf: vec![0.0f32; fft_size],
            high_streak: 0,
            low_streak: 0,
            depth: 0,
            last_peak_hz: 0.0,
            last_peak_db: SILENCE_DB,
            config,
        }
    }

    /// Returns a reference to the active detector configuration.
    pub fn config(&self) -> &DetectorConfig {
        &self.config
    }

    /// Returns the underlying source's sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    /// Pulls one FFT window worth of samples, runs an FFT, and updates the mask depth.
    pub fn poll(&mut self) {
        for slot in &mut self.pull_buf {
            *slot = 0.0;
        }
        let n = self.source.read(&mut self.pull_buf);
        let fft_size = self.scratch.len();
        for (slot, sample) in self.scratch.iter_mut().zip(self.pull_buf.iter()) {
            *slot = rustfft::num_complex::Complex::new(*sample, 0.0);
        }
        if n < fft_size {
            for slot in &mut self.scratch[n..] {
                *slot = rustfft::num_complex::Complex::new(0.0, 0.0);
            }
        }
        self.fft.process(&mut self.scratch);

        // Cache peak metrics across the trigger band (covers all configured
        // buckets plus a small margin for spectral leakage).
        let (peak_lo, peak_hi) = self.config.peak_band_hz;
        let (peak_bin, peak_norm_sqr) = self.peak_in_band(peak_lo, peak_hi);
        let bin_hz = f64::from(self.source.sample_rate()) / fft_size as f64;
        self.last_peak_hz = (peak_bin as f64 * bin_hz) as f32;
        let peak_magnitude = peak_norm_sqr.sqrt();
        let max_magnitude = (fft_size / 2) as f32;
        self.last_peak_db = if peak_magnitude > 0.0 {
            20.0 * (peak_magnitude / max_magnitude).log10()
        } else {
            SILENCE_DB
        };

        match self.dominant_bucket() {
            Some(depth) => {
                self.high_streak = self.high_streak.saturating_add(1);
                self.low_streak = 0;
                if self.high_streak >= STREAK_TO_FLIP {
                    self.depth = depth;
                }
            }
            None => {
                self.low_streak = self.low_streak.saturating_add(1);
                self.high_streak = 0;
                if self.low_streak >= STREAK_TO_FLIP {
                    self.depth = 0;
                }
            }
        }
    }

    /// Returns whether the detector is currently flagging a trigger condition.
    pub fn is_compromised(&self) -> bool {
        self.depth > 0
    }

    /// Returns the current mask depth (0–4). 0 means no trigger; higher values
    /// mean more low bits will be cleared from `random_u64`.
    pub fn depth(&self) -> u8 {
        self.depth
    }

    /// Frequency in Hz of the strongest bin in the trigger band as of the
    /// most recent [`poll`](Self::poll). Returns 0.0 if `poll` has not been
    /// called yet.
    pub fn peak_hz(&self) -> f32 {
        self.last_peak_hz
    }

    /// dBFS magnitude of the peak bin from the most recent
    /// [`poll`](Self::poll). Returns [`SILENCE_DB`] if no signal was present.
    pub fn peak_db(&self) -> f32 {
        self.last_peak_db
    }

    fn peak_in_band(&self, lo_hz: f32, hi_hz: f32) -> (usize, f32) {
        let fft_size = self.scratch.len();
        let nyquist_bin = fft_size / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / fft_size as f64;
        let lo_bin = ((f64::from(lo_hz) / bin_hz).floor() as usize).min(nyquist_bin);
        let hi_bin = ((f64::from(hi_hz) / bin_hz).ceil() as usize).min(nyquist_bin);
        if lo_bin > hi_bin {
            return (0, 0.0);
        }
        let mut best = (lo_bin, 0.0f32);
        for i in lo_bin..=hi_bin {
            let p = self.scratch[i].norm_sqr();
            if p > best.1 {
                best = (i, p);
            }
        }
        best
    }

    fn dominant_bucket(&self) -> Option<u8> {
        // Strict `>` keeps the first-listed (lower-depth) bucket on exact ties — clears
        // fewer bits when the signal is ambiguous, which is the conservative default.
        let half_width = self.config.bucket_half_width_hz;
        let mut best: Option<(f32, u8)> = None;
        for &(center, depth) in &self.config.buckets {
            let p = self.band_power_between(center - half_width, center + half_width);
            if best.map_or(true, |(bp, _)| p > bp) {
                best = Some((p, depth));
            }
        }
        best.and_then(|(p, d)| {
            if p >= self.config.power_threshold {
                Some(d)
            } else {
                None
            }
        })
    }

    fn band_power_between(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let fft_size = self.scratch.len();
        let nyquist_bin = fft_size / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / fft_size as f64;
        let lo_bin = ((f64::from(lo_hz) / bin_hz).floor() as usize).min(nyquist_bin);
        let hi_bin = ((f64::from(hi_hz) / bin_hz).ceil() as usize).min(nyquist_bin);
        if lo_bin > hi_bin {
            return 0.0;
        }
        self.scratch[lo_bin..=hi_bin]
            .iter()
            .map(|c| c.norm_sqr())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detector_constructible_from_sine_source() {
        let detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
        assert_eq!(detector.sample_rate(), 48_000);
        assert!(!detector.is_compromised());
        assert_eq!(detector.depth(), 0);
    }

    #[test]
    fn sine_source_amplitude_bounded() {
        let mut source = SineSource::new(1_000.0, 0.5);
        let mut buf = [0.0f32; 4096];
        source.read(&mut buf);
        for s in buf.iter() {
            assert!(*s >= -0.5 && *s <= 0.5, "sample {s} out of bounds");
        }
    }

    #[test]
    fn sine_source_phase_continuous_across_reads() {
        let mut split = SineSource::with_sample_rate(1_000.0, 1.0, 48_000);
        let mut whole = SineSource::with_sample_rate(1_000.0, 1.0, 48_000);
        let mut a = [0.0f32; 256];
        let mut b = [0.0f32; 256];
        split.read(&mut a);
        split.read(&mut b);
        let mut combined = [0.0f32; 512];
        whole.read(&mut combined);
        for i in 0..256 {
            assert!((b[i] - combined[i + 256]).abs() < 1e-6);
        }
    }

    /// A `Vec<f32>`-backed source that yields pre-scripted samples in chunks.
    struct ScriptedSource {
        samples: Vec<f32>,
        cursor: usize,
        sample_rate: u32,
    }

    impl ScriptedSource {
        fn new(samples: Vec<f32>, sample_rate: u32) -> Self {
            Self {
                samples,
                cursor: 0,
                sample_rate,
            }
        }
    }

    impl AudioSource for ScriptedSource {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn read(&mut self, buf: &mut [f32]) -> usize {
            let remaining = self.samples.len().saturating_sub(self.cursor);
            let n = remaining.min(buf.len());
            buf[..n].copy_from_slice(&self.samples[self.cursor..self.cursor + n]);
            self.cursor += n;
            for slot in &mut buf[n..] {
                *slot = 0.0;
            }
            n
        }
    }

    fn sine_window(frequency: f32, amplitude: f32, sample_rate: u32, len: usize) -> Vec<f32> {
        let mut src = SineSource::with_sample_rate(frequency, amplitude, sample_rate);
        let mut buf = vec![0.0f32; len];
        src.read(&mut buf);
        buf
    }

    #[test]
    fn detector_depth_bucket_1_tone() {
        let mut detector = Detector::with_source(SineSource::new(BUCKETS[0].0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 1);
        assert!(detector.is_compromised());
    }

    #[test]
    fn detector_depth_bucket_2_tone() {
        let mut detector = Detector::with_source(SineSource::new(BUCKETS[1].0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 2);
    }

    #[test]
    fn detector_depth_bucket_3_tone() {
        let mut detector = Detector::with_source(SineSource::new(BUCKETS[2].0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 3);
    }

    #[test]
    fn detector_depth_bucket_4_tone() {
        let mut detector = Detector::with_source(SineSource::new(BUCKETS[3].0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 4);
    }

    /// 5 kHz sits well outside the trigger band in both default (0.5–12 Hz
    /// infrasound) and `audible-test` (0.9–2.6 kHz) configurations;
    /// rectangular-window leakage at that distance is negligible at amp 0.5.
    #[test]
    fn detector_stays_uncompromised_under_out_of_band_tone() {
        let mut detector = Detector::with_source(SineSource::new(5_000.0, 0.5));
        for _ in 0..24 {
            detector.poll();
            assert!(
                !detector.is_compromised(),
                "5 kHz tone should not flip the detector"
            );
            assert_eq!(detector.depth(), 0);
        }
    }

    /// Pin the infrasound release default to its four target depths via
    /// explicit literal frequencies (not `BUCKETS[..]`), so renaming or
    /// resizing `BUCKETS` cannot silently break the v0.4.0 contract.
    /// Skipped under `audible-test` because that feature deliberately swaps
    /// the compile-time default band.
    #[cfg(not(feature = "audible-test"))]
    #[test]
    fn detector_release_default_triggers_on_infrasound_inject() {
        for &(hz, expected_depth) in &[(1.0f32, 1u8), (3.0, 2), (5.0, 3), (10.0, 4)] {
            let mut detector = Detector::with_source(SineSource::new(hz, 0.5));
            for _ in 0..12 {
                detector.poll();
            }
            assert_eq!(
                detector.depth(),
                expected_depth,
                "{hz} Hz should reach depth {expected_depth} under release_default \
                 (got depth={}, peak_hz={:.2}, peak_db={:.1})",
                detector.depth(),
                detector.peak_hz(),
                detector.peak_db()
            );
            assert!(detector.is_compromised());
        }
    }

    #[test]
    fn detector_does_not_flip_just_below_streak_threshold() {
        let fft_size = base_default().fft_size;
        let mut samples = sine_window(BUCKETS[0].0, 0.5, 48_000, fft_size * 2);
        samples.extend(vec![0.0f32; fft_size * 10]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..12 {
            detector.poll();
            assert_eq!(
                detector.depth(),
                0,
                "2-window burst (streak=2 < 3) should not flip the detector"
            );
        }
    }

    #[test]
    fn detector_recovers_after_tone_stops() {
        let fft_size = base_default().fft_size;
        let mut samples = sine_window(BUCKETS[0].0, 0.5, 48_000, fft_size * 5);
        samples.extend(vec![0.0f32; fft_size * 5]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..5 {
            detector.poll();
        }
        assert_eq!(
            detector.depth(),
            1,
            "tone phase should raise depth to 1 (first bucket)"
        );
        for _ in 0..5 {
            detector.poll();
        }
        assert_eq!(
            detector.depth(),
            0,
            "silence phase should drop depth back to 0"
        );
    }

    #[test]
    fn sine_source_zero_crossings_match_frequency() {
        let mut source = SineSource::with_sample_rate(1_000.0, 1.0, 48_000);
        let mut buf = vec![0.0f32; 48_000];
        source.read(&mut buf);
        let mut crossings = 0usize;
        for pair in buf.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if b != 0.0 && a.signum() != b.signum() {
                crossings += 1;
            }
        }
        let expected = 2_000i32;
        let tolerance = expected / 20;
        let diff = (crossings as i32 - expected).abs();
        assert!(
            diff <= tolerance,
            "expected ~{expected} zero crossings, got {crossings}"
        );
    }

    /// Helper that constructs a `DetectorConfig` for a single-bucket band at
    /// `center_hz`, sized to match the historical 100 Hz half-width and a
    /// generous peak band so tone-injection tests stay deterministic. Uses
    /// the small 2048-point FFT window since the helper is only used for
    /// kHz-scale tones in unit tests.
    fn config_for_band(center_hz: f32, depth: u8, threshold: f32) -> DetectorConfig {
        DetectorConfig {
            buckets: vec![(center_hz, depth)],
            power_threshold: threshold,
            peak_band_hz: ((center_hz - 200.0).max(0.0), center_hz + 200.0),
            sample_rate: 48_000,
            fft_size: DEFAULT_FFT_SIZE,
            bucket_half_width_hz: DEFAULT_BUCKET_HALF_WIDTH_HZ,
        }
    }

    #[test]
    fn detector_with_config_triggers_on_custom_band() {
        // Pick a sub-bass band the const-based default would never see.
        let cfg = config_for_band(200.0, 3, 100.0);
        let mut detector = Detector::with_config(SineSource::new(200.0, 0.5), cfg);
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 3);
        assert!(detector.is_compromised());
    }

    #[test]
    fn detector_with_config_ignores_out_of_band_tone() {
        // 5 kHz tone, but the detector is configured to watch 200 Hz only.
        let cfg = config_for_band(200.0, 1, 100.0);
        let mut detector = Detector::with_config(SineSource::new(5_000.0, 0.5), cfg);
        for _ in 0..24 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 0);
    }

    #[test]
    fn detector_config_release_default_round_trip() {
        let cfg = DetectorConfig::release_default();
        assert_eq!(cfg.buckets.len(), 4);
        // Infrasound 1 / 3 / 5 / 10 Hz — see release_default doc for rationale.
        assert!((cfg.buckets[0].0 - 1.0).abs() < 0.001);
        assert!((cfg.buckets[1].0 - 3.0).abs() < 0.001);
        assert!((cfg.buckets[2].0 - 5.0).abs() < 0.001);
        assert!((cfg.buckets[3].0 - 10.0).abs() < 0.001);
        assert!((cfg.power_threshold - 10_000.0).abs() < 1.0);
        assert_eq!(cfg.fft_size, RELEASE_FFT_SIZE);
        assert!((cfg.bucket_half_width_hz - 1.0).abs() < 0.001);
        // Peak band must cover all buckets with margin.
        assert!(cfg.peak_band_hz.0 < 1.0);
        assert!(cfg.peak_band_hz.1 > 10.0);
    }

    #[test]
    fn detector_config_audible_test_round_trip() {
        let cfg = DetectorConfig::audible_test();
        assert_eq!(cfg.buckets.len(), 4);
        assert!((cfg.buckets[0].0 - 1_000.0).abs() < 1.0);
        assert!((cfg.power_threshold - 100.0).abs() < 1.0);
    }

    #[test]
    fn detector_config_peak_band_from_buckets_spans_min_max() {
        let buckets = vec![(20.0, 1), (50.0, 4)];
        let (lo, hi) = DetectorConfig::peak_band_from_buckets(&buckets);
        assert!(lo < 20.0);
        assert!(hi > 50.0);
        assert!(lo >= 0.0);
    }

    #[test]
    fn detector_config_from_env_returns_none_when_no_vars_set() {
        let _g = EnvGuard::clear_all();
        assert!(DetectorConfig::from_env().is_none());
    }

    #[test]
    fn detector_config_from_env_parses_buckets_only() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_BUCKETS", "100:1,200:2,300:3");
        let cfg = DetectorConfig::from_env().expect("bucket override should yield Some");
        assert_eq!(cfg.buckets, vec![(100.0, 1), (200.0, 2), (300.0, 3)]);
        // peak_band auto-derived from the new buckets, not left at the default.
        assert!(cfg.peak_band_hz.0 < 100.0);
        assert!(cfg.peak_band_hz.1 > 300.0);
    }

    #[test]
    fn detector_config_from_env_parses_threshold_only() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_THRESHOLD", "12345");
        let cfg = DetectorConfig::from_env().expect("threshold override should yield Some");
        assert!((cfg.power_threshold - 12_345.0).abs() < 1.0);
        // Buckets fall back to the compile-time default.
        assert_eq!(cfg.buckets, base_default().buckets);
    }

    #[test]
    fn detector_config_from_env_parses_buckets_and_threshold_and_peak_band() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_BUCKETS", "20:1,30:2,40:3,50:4");
        std::env::set_var("HOBA_THRESHOLD", "8000.5");
        std::env::set_var("HOBA_PEAK_BAND", "10:60");
        let cfg = DetectorConfig::from_env().expect("any override should yield Some");
        assert_eq!(
            cfg.buckets,
            vec![(20.0, 1), (30.0, 2), (40.0, 3), (50.0, 4)]
        );
        assert!((cfg.power_threshold - 8_000.5).abs() < 0.001);
        assert_eq!(cfg.peak_band_hz, (10.0, 60.0));
    }

    #[test]
    fn detector_config_from_env_falls_back_on_garbage_buckets() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_BUCKETS", "this is not a bucket");
        let cfg = DetectorConfig::from_env().expect("malformed override still yields Some");
        // Garbage buckets fall back to the compile-time default.
        assert_eq!(cfg.buckets, base_default().buckets);
    }

    #[test]
    fn detector_config_from_env_falls_back_on_garbage_threshold() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_THRESHOLD", "not-a-number");
        let cfg = DetectorConfig::from_env().expect("malformed override still yields Some");
        assert!((cfg.power_threshold - base_default().power_threshold).abs() < 0.001);
    }

    #[test]
    fn detector_config_from_env_falls_back_on_inverted_peak_band() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_PEAK_BAND", "50:10");
        let cfg = DetectorConfig::from_env().expect("malformed override still yields Some");
        assert_eq!(cfg.peak_band_hz, base_default().peak_band_hz);
    }

    /// RAII helper that clears the three HOBA_* env vars before a test runs and
    /// restores them on drop. Tests in this module mutate process-global state,
    /// so they're serialised through this guard plus a shared mutex in the
    /// helper itself (Rust runs unit tests in parallel by default).
    struct EnvGuard {
        prior: [(String, Option<String>); 4],
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn clear_all() -> Self {
            static MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let lock = MUTEX.lock().unwrap_or_else(|p| p.into_inner());
            let names = [
                "HOBA_BUCKETS",
                "HOBA_THRESHOLD",
                "HOBA_PEAK_BAND",
                "HOBA_DEBUG",
            ];
            let mut prior: [(String, Option<String>); 4] = [
                (names[0].into(), None),
                (names[1].into(), None),
                (names[2].into(), None),
                (names[3].into(), None),
            ];
            for (i, n) in names.iter().enumerate() {
                prior[i] = ((*n).into(), std::env::var(n).ok());
                std::env::remove_var(n);
            }
            Self { prior, _lock: lock }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, prior) in &self.prior {
                match prior {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[cfg(feature = "mic")]
    mod mic_tests {
        use super::*;

        #[test]
        fn mic_source_dead_when_no_audio_or_active_consistent() {
            let mic = MicSource::new();
            // Invariant: dead sources report sample_rate == 0; live sources report > 0.
            // Subsumes "constructs without panic" since constructing is a precondition.
            assert_eq!(mic.is_active(), mic.sample_rate() > 0);
        }

        #[test]
        fn mic_source_read_returns_at_most_buf_len() {
            let mut mic = MicSource::new();
            let mut buf = [0.0f32; 64];
            let n = mic.read(&mut buf);
            assert!(n <= buf.len());
        }

        #[test]
        fn detector_accepts_mic_source() {
            let mic = MicSource::new();
            let mut detector = Detector::with_source(mic);
            detector.poll();
            let _ = detector.is_compromised();
        }
    }
}

#[cfg(feature = "mic")]
mod mic {
    use super::AudioSource;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use ringbuf::traits::{Consumer, Producer, Split};

    /// Microphone-backed audio source via cpal.
    ///
    /// Failure modes (no input device, permission denied, unsupported sample
    /// format, malformed device config) are absorbed silently — the source
    /// then yields zero samples and downstream `Detector` simply stays in
    /// its non-compromised state. Use [`MicSource::is_active`] to detect
    /// the dead-source case explicitly.
    ///
    /// Supported sample formats: F32, I16, U16. Other formats (I32, F64,
    /// etc.) silent-fail. Multi-channel input is downmixed by keeping only
    /// the first channel of each frame.
    ///
    /// `MicSource` is `!Send` and `!Sync` on every platform because cpal's
    /// `Stream` is `!Send` and `!Sync`. Construct it on the thread that
    /// will hold the `Detector`.
    pub struct MicSource {
        sample_rate: u32,
        consumer: ringbuf::HeapCons<f32>,
        // Underscore-prefixed but load-bearing: dropping the stream stops the cpal callback,
        // so this field is kept alive for the entire `MicSource` lifetime.
        _stream: Option<cpal::Stream>,
    }

    impl MicSource {
        /// Opens the default input device, falling back to a silent dead source on any failure.
        pub fn new() -> Self {
            Self::try_open().unwrap_or_else(Self::dead)
        }

        /// Returns `true` if the underlying input stream opened successfully.
        pub fn is_active(&self) -> bool {
            self._stream.is_some()
        }

        fn try_open() -> Option<Self> {
            use cpal::SampleFormat;
            let host = cpal::default_host();
            let device = host.default_input_device()?;
            let config = device.default_input_config().ok()?;
            let sample_rate = config.sample_rate().0;
            let channels = usize::from(config.channels());
            if sample_rate == 0 || channels == 0 {
                return None;
            }

            let rb = ringbuf::HeapRb::<f32>::new(sample_rate as usize);
            let (mut producer, consumer) = rb.split();
            let sample_format = config.sample_format();
            let stream_config: cpal::StreamConfig = config.into();
            let err_fn = |_err| {};

            let stream = match sample_format {
                SampleFormat::F32 => device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[f32], _: &_| {
                            for (i, &s) in data.iter().enumerate() {
                                if i % channels == 0 {
                                    let _ = producer.try_push(s);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                    .ok()?,
                SampleFormat::I16 => device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[i16], _: &_| {
                            for (i, &s) in data.iter().enumerate() {
                                if i % channels == 0 {
                                    let _ = producer.try_push(s as f32 / i16::MAX as f32);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                    .ok()?,
                SampleFormat::U16 => device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _: &_| {
                            for (i, &s) in data.iter().enumerate() {
                                if i % channels == 0 {
                                    // 32_768.0 is the midpoint of the u16 range; map [0, 65535] → [-1.0, +1.0).
                                    let _ = producer.try_push((s as f32 - 32_768.0) / 32_768.0);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                    .ok()?,
                _ => return None,
            };

            stream.play().ok()?;

            Some(Self {
                sample_rate,
                consumer,
                _stream: Some(stream),
            })
        }

        fn dead() -> Self {
            let rb = ringbuf::HeapRb::<f32>::new(1);
            let (_p, consumer) = rb.split();
            Self {
                sample_rate: 0,
                consumer,
                _stream: None,
            }
        }
    }

    impl Default for MicSource {
        fn default() -> Self {
            Self::new()
        }
    }

    impl AudioSource for MicSource {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }

        fn read(&mut self, buf: &mut [f32]) -> usize {
            self.consumer.pop_slice(buf)
        }
    }
}

#[cfg(feature = "mic")]
pub use mic::MicSource;
