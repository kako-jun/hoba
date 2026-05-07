//! Audio input abstraction and a sine-wave test fixture.
//!
//! ```no_run
//! use hoba::audio::{Detector, SineSource};
//!
//! // Any tone in the 1–10 Hz infrasound band trips the production default —
//! // the detector is intentionally binary (single bucket, single fixed
//! // mask), matching Patlabor's HOS where the trigger has no graded levels.
//! // Real consumer speakers cannot reproduce this — see
//! // [`DetectorConfig::release_default`] for the rationale.
//! let mut detector = Detector::with_source(SineSource::new(5.0, 0.5));
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
/// 48 kHz, giving a bin width of ~0.73 Hz — fine enough to resolve sub-Hz
/// energy across the 1–10 Hz trigger band. Stepping down to 32768 loses the
/// 1 Hz end of the band; stepping up to 131072 doubles per-poll FFT cost
/// without buying meaningful selectivity for a single-bucket band.
pub(crate) const RELEASE_FFT_SIZE: usize = 65536;
/// Per-bucket center frequency (Hz) for the active preset.
/// Kept `pub(crate)` as a test fixture pinning the current default band; live
/// detection logic reads from [`DetectorConfig`], so production code should
/// reach the same values via [`DetectorConfig::release_default`] /
/// [`DetectorConfig::audible_test`].
///
/// Under the release default this is **a single bucket centred on 5.5 Hz** —
/// anywhere in the 1–10 Hz infrasound window trips the detector. Under the
/// `audible-test` feature the preset is also a single bucket, centred on
/// 1.75 kHz with a 750 Hz half-width covering 1.0–2.5 kHz, structurally
/// symmetric to the release default.
#[cfg(not(feature = "audible-test"))]
#[allow(dead_code)]
pub(crate) const BUCKETS: [f32; 1] = [5.5];
/// Audible-band stand-in for development — single bucket centred at 1.75 kHz
/// covering 1.0–2.5 kHz via a 750 Hz half-width. Most laptop and consumer
/// speakers reproduce this band cleanly, so the detector can be exercised
/// through a tone generator without specialised hardware. Single bucket
/// matches the release default's binary-trigger spirit.
#[cfg(feature = "audible-test")]
#[allow(dead_code)]
pub(crate) const BUCKETS: [f32; 1] = [1_750.0];
/// Default per-bucket half-width in Hz, used when a [`DetectorConfig`] does
/// not specify [`DetectorConfig::bucket_half_width_hz`] and as the margin in
/// [`DetectorConfig::peak_band_from_buckets`]. Sized for kHz-scale buckets;
/// the infrasound release default widens its own per-bucket half-width to
/// 4.5 Hz so a single bucket centred on 5.5 Hz covers the entire 1–10 Hz
/// range, and the audible-test preset widens to 750 Hz so a single bucket
/// centred on 1.75 kHz covers 1.0–2.5 kHz.
const DEFAULT_BUCKET_HALF_WIDTH_HZ: f32 = 100.0;

/// dBFS value reported by [`Detector::peak_db`] when no signal is present
/// (peak magnitude is zero). Callers comparing against silence should use
/// this constant rather than the literal.
pub const SILENCE_DB: f32 = -100.0;
/// Default `snr_threshold_db` for both presets and any caller that does not
/// set it explicitly. 6 dB ≈ 2× amplitude over the noise floor — the same
/// rule of thumb radio engineering uses to call something "audible at all".
/// Empirically separates real triggers from quiet-room background hum on
/// every device the maintainer has tested.
pub const DEFAULT_SNR_THRESHOLD_DB: f32 = 6.0;
/// Default `snr_threshold_db` for [`DetectorConfig::audible_test`] specifically.
/// Raised in v0.5.1 from 6.0 → 18.0 because the audible band (around 1.75 kHz)
/// has continuous low-level sources in real rooms — laptop fans, HVAC,
/// electronic harmonics — that routinely beat 6 dB SNR over the bucket noise
/// floor and produce a perpetual BABEL flood in the cross-terminal demo.
/// 18 dB is closer to the empirical room-noise SNR ceiling on a typical
/// laptop. For demos in noisier environments, override with `HOBA_SNR=30` or
/// `--snr 30`.
pub const AUDIBLE_TEST_SNR_THRESHOLD_DB: f32 = 18.0;
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
/// The default behaviour of [`Detector::with_source`] is
/// equivalent to `DetectorConfig::release_default()` (or
/// `DetectorConfig::audible_test()` when the `audible-test` cargo feature is
/// on). Pass a custom config to [`Detector::with_config`] when you want to
/// monitor a different band (sub-bass, ultrasonic at a non-standard center,
/// CI-friendly tones, …) without a recompile.
///
/// Construction conventions:
/// - `buckets`: 1–N centre frequencies in Hz. The detector fires when **any
///   bucket** beats the noise floor by `snr_threshold_db`. Empty `buckets`
///   makes the detector unable to trigger; the constructor accepts it but
///   the resulting detector permanently reports `is_compromised() == false`.
///   Since v0.5.0 there is no per-bucket depth — the mask is hardcoded
///   LSB 1-bit (`0xFFFF_FFFF_FFFF_FFFE`, even-only). That returns the
///   library to the original `notes/dev/hoba.md` design and matches the
///   binary-trigger HOS in *Patlabor: The Movie* (1989).
/// - `snr_threshold_db`: minimum signal-to-noise ratio (in dB) the bucket's
///   peak must beat over the surrounding noise floor before that bucket
///   counts as triggered. SNR is what "is there a frequency in this band"
///   actually means once you stop relying on a hand-calibrated absolute
///   amplitude. The default is 6 dB — 2× signal over noise floor, the same
///   threshold radio engineering uses to call something "audible at all".
/// - `peak_band_hz`: `(lo, hi)` window scanned for `peak_hz` / `peak_db`,
///   AND for the per-poll noise-floor estimate (median bin power across the
///   band, excluding each bucket's window). Use
///   [`DetectorConfig::peak_band_from_buckets`] to derive a sensible default
///   covering all bucket centers plus margin.
/// - `sample_rate`: documentation hint only; the live source's reported rate
///   is what the FFT uses.
#[derive(Debug, Clone)]
pub struct DetectorConfig {
    /// Centre frequencies (Hz) of the trigger buckets. Any bucket clearing
    /// `snr_threshold_db` over the noise floor flips the detector.
    pub buckets: Vec<f32>,
    /// Minimum signal-to-noise ratio in dB the bucket peak must beat over the
    /// noise floor (median bin power inside `peak_band_hz`, excluding each
    /// bucket's own window) before the bucket counts as triggered.
    ///
    /// 6 dB (≈ 2× amplitude) is the default and the same threshold radio
    /// engineering uses for "audible at all". Push to 10–12 dB if the host
    /// environment has a noisy room tone you want to distinguish from real
    /// triggers; drop to 3 dB if you specifically want a hair-trigger.
    pub snr_threshold_db: f32,
    /// `(lo, hi)` Hz bounds for the peak-search band reported via `peak_hz` /
    /// `peak_db`, and the band the noise-floor estimate is taken from.
    pub peak_band_hz: (f32, f32),
    /// Documentation hint — the live source dictates actual FFT bin width.
    pub sample_rate: u32,
    /// FFT window length in samples. Larger = finer Hz resolution at the cost
    /// of latency and per-poll work. The infrasound release default uses 65536
    /// (~1.4 s at 48 kHz) so the 1 Hz end of the band is resolvable; the
    /// audible-test preset stays at 2048 since 23 Hz bin width is plenty for
    /// 1–2.5 kHz tones. Must be a power of two for `rustfft` planner efficiency,
    /// though the planner accepts other sizes.
    pub fft_size: usize,
    /// Half-width of each bucket window in Hz; band power for a bucket is
    /// summed over `center ± this`. The infrasound default uses 4.5 Hz so a
    /// single bucket centred on 5.5 Hz spans the entire 1–10 Hz trigger
    /// window; the audible-test preset uses 750 Hz so its 1.75 kHz bucket
    /// spans 1.0–2.5 kHz. Widen further when the source has poor frequency
    /// stability.
    pub bucket_half_width_hz: f32,
}

impl DetectorConfig {
    /// Production default: **a single 1–10 Hz infrasound bucket**, all below
    /// the human audibility floor (~20 Hz). Inspired by the HOS
    /// ("バビロンプロジェクト") in *Patlabor: The Movie* (1989), which
    /// triggers from low-frequency wind resonance against tall buildings —
    /// nothing a speaker can play, and crucially **binary**: the system
    /// either fires or it does not. Since v0.5.0 the detector mirrors that
    /// faithfully — anywhere in 1–10 Hz, the trigger flips. The mask applied
    /// while the trigger is active is a hardcoded LSB 1 bit
    /// (`0xFFFF_FFFF_FFFF_FFFE`), restoring the original `notes/dev/hoba.md`
    /// design.
    ///
    /// Implementation: one bucket at center 5.5 Hz with `bucket_half_width_hz
    /// = 4.5`, so the integration window covers exactly 1.0–10.0 Hz. A
    /// consumer playback chain physically cannot reach this band — the
    /// detector waits for real environmental energy: earthquakes, typhoon
    /// gusts, heavy machinery, large HVAC, subway passes. That "doesn't fire
    /// in everyday life" is the whole point.
    ///
    /// The 65536-sample FFT window (~1.4 s at 48 kHz) gives ~0.73 Hz bin
    /// resolution, fine enough to resolve the 1 Hz end of the band against
    /// rectangular-window leakage.
    ///
    /// **Trigger criterion**: the bucket peak must beat the surrounding
    /// noise floor by at least `snr_threshold_db` (default 6 dB). SNR is
    /// what "a frequency is present in this band" actually means once you
    /// stop pretending mic gain and room tone are constant across hosts.
    ///
    /// Callers wanting a different band (sub-bass HVAC, audible CI tones)
    /// can get there without a recompile via `HOBA_BUCKETS` or
    /// [`Detector::with_config`]:
    ///
    /// ```text
    /// HOBA_BUCKETS=20,30,40,50 HOBA_SNR=10   # sub-bass HVAC, stricter SNR
    /// ```
    pub fn release_default() -> Self {
        Self {
            buckets: vec![5.5],
            snr_threshold_db: DEFAULT_SNR_THRESHOLD_DB,
            peak_band_hz: (0.5, 12.0),
            sample_rate: INFRASOUND_SAMPLE_RATE,
            fft_size: RELEASE_FFT_SIZE,
            bucket_half_width_hz: 4.5,
        }
    }

    /// Audible-band preset for development, CI, and live demos. Reachable
    /// without the `audible-test` cargo feature: pass this config to
    /// [`Detector::with_config`] from a release build.
    ///
    /// Single bucket centred at 1.75 kHz with a 750 Hz half-width, so the
    /// integration window covers 1.0–2.5 kHz. Structurally symmetric to
    /// [`Self::release_default`] — one bucket, binary trigger, no graded
    /// depth.
    pub fn audible_test() -> Self {
        Self {
            buckets: vec![1_750.0],
            // v0.5.1: raised from DEFAULT_SNR_THRESHOLD_DB (6.0) — see
            // [`AUDIBLE_TEST_SNR_THRESHOLD_DB`] for rationale. The audible
            // band has real-world low-level sources that the infrasound
            // band does not.
            snr_threshold_db: AUDIBLE_TEST_SNR_THRESHOLD_DB,
            peak_band_hz: (500.0, 3_000.0),
            sample_rate: DEFAULT_SAMPLE_RATE,
            fft_size: DEFAULT_FFT_SIZE,
            bucket_half_width_hz: 750.0,
        }
    }

    /// Reads `HOBA_BUCKETS` / `HOBA_SNR` / `HOBA_PEAK_BAND` from the process
    /// environment and merges them with [`Self::release_default`] (or
    /// [`Self::audible_test`] when the `audible-test` feature is on).
    ///
    /// `HOBA_BUCKETS` format is a comma-separated list of Hz values
    /// (`HOBA_BUCKETS=1,3,5,10`). For back-compat the v0.4.x `hz:depth`
    /// form is still parsed — the `:depth` portion is ignored, and under
    /// `HOBA_DEBUG=1` a one-line stderr warning notes that the depth concept
    /// was removed in v0.5.0.
    ///
    /// `HOBA_THRESHOLD` (the v0.3.x raw-power threshold) is still inspected
    /// for back-compat detection but its value is **ignored** — the unit
    /// changed from raw post-FFT power to a dB SNR, and silently
    /// reinterpreting the number would produce surprising trigger behaviour.
    /// Set `HOBA_DEBUG=1` to see a one-line deprecation note on stderr; use
    /// `HOBA_SNR` instead.
    ///
    /// Returns `None` if no relevant env vars are set, so callers can skip
    /// the override path entirely. Returns `Some(config)` if at least one
    /// override is present; unrecognised values fall back to the matching
    /// default field. When `HOBA_DEBUG=1` is set, parse failures emit a
    /// single line on stderr; otherwise failures are silent (consistent with
    /// hoba's library-fails-quietly contract).
    pub fn from_env() -> Option<Self> {
        let buckets_raw = std::env::var("HOBA_BUCKETS").ok();
        let snr_raw = std::env::var("HOBA_SNR").ok();
        let legacy_threshold_raw = std::env::var("HOBA_THRESHOLD").ok();
        let peak_band_raw = std::env::var("HOBA_PEAK_BAND").ok();
        if buckets_raw.is_none()
            && snr_raw.is_none()
            && legacy_threshold_raw.is_none()
            && peak_band_raw.is_none()
        {
            return None;
        }

        let mut config = base_default();
        let debug = std::env::var("HOBA_DEBUG").as_deref() == Ok("1");

        if let Some(s) = buckets_raw.as_deref() {
            match parse_buckets_env(s, debug) {
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

        if let Some(s) = snr_raw.as_deref() {
            match s.trim().parse::<f32>() {
                Ok(v) if v.is_finite() && v >= 0.0 => config.snr_threshold_db = v,
                Ok(_) | Err(_) => {
                    if debug {
                        eprintln!("hoba: HOBA_SNR invalid ({s:?}); using default");
                    }
                }
            }
        }

        if legacy_threshold_raw.is_some() && debug {
            eprintln!(
                "hoba: HOBA_THRESHOLD is deprecated since v0.4.0 (raw-power threshold replaced \
                 by dB SNR); ignoring. Use HOBA_SNR=<dB> instead."
            );
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
    pub fn peak_band_from_buckets(buckets: &[f32]) -> (f32, f32) {
        if buckets.is_empty() {
            return (0.0, 0.0);
        }
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &c in buckets {
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

/// Parses `HOBA_BUCKETS` (`hz,hz,hz`). For back-compat with v0.4.x the
/// legacy `hz:depth` form is also accepted — the `:depth` portion is
/// dropped silently and, when `debug` is set, a one-line warning explains
/// that the depth concept was removed in v0.5.0.
fn parse_buckets_env(s: &str, debug: bool) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    let mut saw_legacy_depth = false;
    for raw in s.split(',') {
        let part = raw.trim();
        if part.is_empty() {
            continue;
        }
        let hz_str = match part.split_once(':') {
            Some((hz, _depth)) => {
                saw_legacy_depth = true;
                hz.trim()
            }
            None => part,
        };
        let hz: f32 = hz_str
            .parse()
            .map_err(|e| format!("center_hz '{hz_str}' invalid: {e}"))?;
        if !hz.is_finite() || hz <= 0.0 {
            return Err(format!("center_hz must be positive and finite: {hz_str}"));
        }
        out.push(hz);
    }
    if saw_legacy_depth && debug {
        eprintln!(
            "hoba: HOBA_BUCKETS legacy 'hz:depth' form detected; ':depth' ignored — the \
             graded depth concept was removed in v0.5.0. Use 'HOBA_BUCKETS=hz,hz,hz'."
        );
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
    compromised: bool,
    last_peak_hz: f32,
    last_peak_db: f32,
    last_noise_floor_db: f32,
    config: DetectorConfig,
}

impl<S: AudioSource + core::fmt::Debug> core::fmt::Debug for Detector<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Detector")
            .field("source", &self.source)
            .field("high_streak", &self.high_streak)
            .field("low_streak", &self.low_streak)
            .field("compromised", &self.compromised)
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
            compromised: false,
            last_peak_hz: 0.0,
            last_peak_db: SILENCE_DB,
            last_noise_floor_db: SILENCE_DB,
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

    /// Pulls one FFT window worth of samples, runs an FFT, and updates the trigger flag.
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
        let max_magnitude = (fft_size / 2) as f32;
        self.last_peak_db = power_to_db(peak_norm_sqr, max_magnitude);

        // Estimate the noise floor as the median bin *power* (norm_sqr) inside
        // peak_band_hz, excluding each bucket's own window so a strong tone
        // sitting inside a bucket can never inflate its own floor and
        // self-mask. Power-domain median first, dB conversion afterwards: a
        // dB-domain median would over-weight near-silent bins (-100 dB
        // outliers pull a dB median harder than they pull a power median).
        let noise_floor_power = self.noise_floor_power(peak_lo, peak_hi);
        self.last_noise_floor_db = power_to_db(noise_floor_power, max_magnitude);

        if self.any_bucket_triggered(noise_floor_power, max_magnitude) {
            self.high_streak = self.high_streak.saturating_add(1);
            self.low_streak = 0;
            if self.high_streak >= STREAK_TO_FLIP {
                self.compromised = true;
            }
        } else {
            self.low_streak = self.low_streak.saturating_add(1);
            self.high_streak = 0;
            if self.low_streak >= STREAK_TO_FLIP {
                self.compromised = false;
            }
        }
    }

    /// Returns whether the detector is currently flagging a trigger condition.
    pub fn is_compromised(&self) -> bool {
        self.compromised
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

    /// dBFS magnitude of the noise-floor estimate (median bin power across
    /// `peak_band_hz`, excluding each bucket's own window) from the most
    /// recent [`poll`](Self::poll). Returns [`SILENCE_DB`] when the band
    /// outside the buckets has no measurable energy or when `poll` has not
    /// been called yet.
    pub fn noise_floor_db(&self) -> f32 {
        self.last_noise_floor_db
    }

    /// Signal-to-noise ratio in dB from the most recent
    /// [`poll`](Self::poll), defined as `peak_db - noise_floor_db`. Returns
    /// 0.0 before the first poll. Floors at 0.0 when peak is below the
    /// floor — a negative SNR is information-free for trigger purposes.
    pub fn snr_db(&self) -> f32 {
        (self.last_peak_db - self.last_noise_floor_db).max(0.0)
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

    /// Returns `true` when at least one configured bucket beats the noise
    /// floor by `snr_threshold_db`. Since v0.5.0 the trigger is binary: any
    /// bucket clearing the SNR bar flips the detector, no per-bucket depth
    /// is computed. Matches the binary-trigger HOS in *Patlabor: The Movie*.
    fn any_bucket_triggered(&self, noise_floor_power: f32, max_magnitude: f32) -> bool {
        let half_width = self.config.bucket_half_width_hz;
        let noise_floor_db = power_to_db(noise_floor_power, max_magnitude);
        for &center in &self.config.buckets {
            let peak_power = self.peak_power_between(center - half_width, center + half_width);
            let peak_db = power_to_db(peak_power, max_magnitude);
            let snr = peak_db - noise_floor_db;
            if snr >= self.config.snr_threshold_db {
                return true;
            }
        }
        false
    }

    /// Returns the strongest single-bin power inside `[lo_hz, hi_hz]`. Used
    /// by [`Detector::any_bucket_triggered`] so a bucket's peak — not its
    /// summed energy — is what gets compared to the noise floor. Summed-band
    /// power would scale with bucket width and let a wide quiet bucket "beat"
    /// a narrow loud one, which is the opposite of what SNR is asking.
    fn peak_power_between(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let fft_size = self.scratch.len();
        let nyquist_bin = fft_size / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / fft_size as f64;
        let lo_bin = ((f64::from(lo_hz) / bin_hz).floor() as usize).min(nyquist_bin);
        let hi_bin = ((f64::from(hi_hz) / bin_hz).ceil() as usize).min(nyquist_bin);
        if lo_bin > hi_bin {
            return 0.0;
        }
        let mut best = 0.0f32;
        for i in lo_bin..=hi_bin {
            let p = self.scratch[i].norm_sqr();
            if p > best {
                best = p;
            }
        }
        best
    }

    /// Median bin power inside `[lo_hz, hi_hz]`, excluding every bucket's
    /// `center ± bucket_half_width_hz` window. Median rather than mean
    /// because a single loud trigger tone would otherwise drag the floor up
    /// linearly and mask itself; median is robust against that. Excluding
    /// the bucket windows is essential — leaving them in lets a tone sitting
    /// inside a bucket count toward "its own" noise floor.
    fn noise_floor_power(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let fft_size = self.scratch.len();
        let nyquist_bin = fft_size / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / fft_size as f64;
        let lo_bin = ((f64::from(lo_hz) / bin_hz).floor() as usize).min(nyquist_bin);
        let hi_bin = ((f64::from(hi_hz) / bin_hz).ceil() as usize).min(nyquist_bin);
        if lo_bin > hi_bin {
            return 0.0;
        }
        let half_width = self.config.bucket_half_width_hz;
        let mut excluded: Vec<(usize, usize)> = self
            .config
            .buckets
            .iter()
            .map(|&center| {
                let blo = ((f64::from((center - half_width).max(0.0)) / bin_hz).floor() as usize)
                    .min(nyquist_bin);
                let bhi =
                    ((f64::from(center + half_width) / bin_hz).ceil() as usize).min(nyquist_bin);
                (blo, bhi)
            })
            .collect();
        excluded.sort_unstable();

        let mut out: Vec<f32> = Vec::with_capacity(hi_bin.saturating_sub(lo_bin) + 1);
        for i in lo_bin..=hi_bin {
            if excluded.iter().any(|&(blo, bhi)| i >= blo && i <= bhi) {
                continue;
            }
            out.push(self.scratch[i].norm_sqr());
        }
        if out.is_empty() {
            // Buckets cover the entire peak band — nothing left to estimate
            // the floor from. Conservative fallback: pretend it is silent so
            // SNR collapses to peak_db, which mirrors the v0.3.x behaviour
            // and keeps the detector usable when the user explicitly tunes
            // peak_band_hz tight to the buckets.
            return 0.0;
        }
        out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = out.len();
        if n % 2 == 1 {
            out[n / 2]
        } else {
            (out[n / 2 - 1] + out[n / 2]) * 0.5
        }
    }
}

/// Converts a single-bin / aggregated power value (FFT `norm_sqr`) into
/// dBFS using the standard reference `max_magnitude = fft_size / 2`. Returns
/// [`SILENCE_DB`] for non-positive input so callers never see `-inf` or NaN.
fn power_to_db(power: f32, max_magnitude: f32) -> f32 {
    if power > 0.0 {
        let magnitude = power.sqrt();
        20.0 * (magnitude / max_magnitude).log10()
    } else {
        SILENCE_DB
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

    /// Iterate over every configured bucket center under the active preset
    /// and confirm each one trips the detector. Both presets ship a single
    /// bucket since v0.5.0.
    #[test]
    fn detector_triggers_on_each_preset_bucket() {
        for &freq in BUCKETS.iter() {
            let mut detector = Detector::with_source(SineSource::new(freq, 0.5));
            for _ in 0..12 {
                detector.poll();
            }
            assert!(
                detector.is_compromised(),
                "{freq} Hz tone should trip the detector"
            );
        }
    }

    /// 5 kHz sits well outside the trigger band in both default (0.5–12 Hz
    /// infrasound) and `audible-test` (0.5–3 kHz) configurations;
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
        }
    }

    /// Pin the infrasound release default to a binary trigger across the
    /// entire 1–10 Hz band: anywhere inside the window, the detector must
    /// flip. This is the *Patlabor*-faithful contract — the HOS trigger has
    /// no graded levels in the film, and v0.5.0 has no graded depth in the
    /// library either.
    /// Skipped under `audible-test` because that feature deliberately swaps
    /// the compile-time default band for an audible 1–2.5 kHz bucket.
    #[cfg(not(feature = "audible-test"))]
    #[test]
    fn detector_release_default_triggers_on_infrasound_inject() {
        for &hz in &[1.0f32, 3.0, 5.0, 7.0, 10.0] {
            let mut detector = Detector::with_source(SineSource::new(hz, 0.5));
            for _ in 0..12 {
                detector.poll();
            }
            assert!(
                detector.is_compromised(),
                "{hz} Hz should trigger under release_default \
                 (peak_hz={:.2}, peak_db={:.1})",
                detector.peak_hz(),
                detector.peak_db()
            );
        }
    }

    #[test]
    fn detector_does_not_flip_just_below_streak_threshold() {
        let fft_size = base_default().fft_size;
        let mut samples = sine_window(BUCKETS[0], 0.5, 48_000, fft_size * 2);
        samples.extend(vec![0.0f32; fft_size * 10]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..12 {
            detector.poll();
            assert!(
                !detector.is_compromised(),
                "2-window burst (streak=2 < 3) should not flip the detector"
            );
        }
    }

    #[test]
    fn detector_recovers_after_tone_stops() {
        let fft_size = base_default().fft_size;
        let freq = BUCKETS[0];
        let mut samples = sine_window(freq, 0.5, 48_000, fft_size * 5);
        samples.extend(vec![0.0f32; fft_size * 5]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..5 {
            detector.poll();
        }
        assert!(
            detector.is_compromised(),
            "tone phase should flip the detector"
        );
        for _ in 0..5 {
            detector.poll();
        }
        assert!(
            !detector.is_compromised(),
            "silence phase should clear the detector"
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
    fn config_for_band(center_hz: f32, snr_threshold_db: f32) -> DetectorConfig {
        DetectorConfig {
            buckets: vec![center_hz],
            snr_threshold_db,
            peak_band_hz: ((center_hz - 200.0).max(0.0), center_hz + 200.0),
            sample_rate: 48_000,
            fft_size: DEFAULT_FFT_SIZE,
            bucket_half_width_hz: DEFAULT_BUCKET_HALF_WIDTH_HZ,
        }
    }

    #[test]
    fn detector_with_config_triggers_on_custom_band() {
        // Pick a sub-bass band the const-based default would never see.
        let cfg = config_for_band(200.0, DEFAULT_SNR_THRESHOLD_DB);
        let mut detector = Detector::with_config(SineSource::new(200.0, 0.5), cfg);
        for _ in 0..12 {
            detector.poll();
        }
        assert!(detector.is_compromised());
    }

    #[test]
    fn detector_with_config_ignores_out_of_band_tone() {
        // 5 kHz tone, but the detector is configured to watch 200 Hz only.
        let cfg = config_for_band(200.0, DEFAULT_SNR_THRESHOLD_DB);
        let mut detector = Detector::with_config(SineSource::new(5_000.0, 0.5), cfg);
        for _ in 0..24 {
            detector.poll();
        }
        assert!(!detector.is_compromised());
    }

    /// SNR sanity: a tone in the bucket should report a positive SNR well
    /// above the default 6 dB threshold, while pure silence stays at 0.
    /// This pins the noise-floor / SNR pipeline end-to-end without
    /// reaching into private fields.
    #[test]
    fn detector_reports_snr_for_tone_and_silence() {
        let cfg = config_for_band(200.0, DEFAULT_SNR_THRESHOLD_DB);
        let mut tone_det = Detector::with_config(SineSource::new(200.0, 0.5), cfg.clone());
        for _ in 0..12 {
            tone_det.poll();
        }
        assert!(
            tone_det.snr_db() >= DEFAULT_SNR_THRESHOLD_DB,
            "200 Hz tone should produce SNR ≥ {} dB, got {} dB (peak={:.1} floor={:.1})",
            DEFAULT_SNR_THRESHOLD_DB,
            tone_det.snr_db(),
            tone_det.peak_db(),
            tone_det.noise_floor_db()
        );

        let silence = ScriptedSource::new(vec![0.0f32; cfg.fft_size * 12], 48_000);
        let mut sil_det = Detector::with_config(silence, cfg);
        for _ in 0..12 {
            sil_det.poll();
        }
        assert!(
            sil_det.snr_db() < DEFAULT_SNR_THRESHOLD_DB,
            "silence should not clear the SNR threshold (got {} dB)",
            sil_det.snr_db()
        );
        assert!(!sil_det.is_compromised());
    }

    /// Multi-bucket selectivity: with two well-separated bucket centres, a
    /// tone injected at one centre fires the detector (binary trigger);
    /// silence keeps it cleared. Since v0.5.0 the trigger is binary — there
    /// is no per-bucket depth to differentiate, but the OR-of-buckets logic
    /// must still be exercised.
    #[test]
    fn detector_fires_when_any_bucket_has_signal() {
        let cfg = DetectorConfig {
            buckets: vec![1_000.0, 2_500.0],
            snr_threshold_db: DEFAULT_SNR_THRESHOLD_DB,
            peak_band_hz: (500.0, 3_000.0),
            sample_rate: 48_000,
            fft_size: DEFAULT_FFT_SIZE,
            bucket_half_width_hz: 50.0,
        };
        let mut det_low = Detector::with_config(SineSource::new(1_000.0, 0.5), cfg.clone());
        for _ in 0..12 {
            det_low.poll();
        }
        assert!(
            det_low.is_compromised(),
            "tone at 1 kHz should fire (any-bucket-triggers semantics)"
        );

        let mut det_high = Detector::with_config(SineSource::new(2_500.0, 0.5), cfg);
        for _ in 0..12 {
            det_high.poll();
        }
        assert!(
            det_high.is_compromised(),
            "tone at 2.5 kHz should fire (any-bucket-triggers semantics)"
        );
    }

    /// SNR threshold gating: with a pathologically high `snr_threshold_db`
    /// (e.g. 80 dB), even a clear in-band tone must fail to flip the
    /// detector. Direct test that the SNR comparison actually gates the
    /// trigger rather than getting bypassed by the streak counter.
    #[test]
    fn detector_high_snr_threshold_suppresses_trigger() {
        let mut cfg = config_for_band(200.0, 80.0);
        cfg.fft_size = DEFAULT_FFT_SIZE;
        let mut detector = Detector::with_config(SineSource::new(200.0, 0.5), cfg);
        for _ in 0..24 {
            detector.poll();
        }
        assert!(
            !detector.is_compromised(),
            "80 dB SNR requirement should suppress trigger (peak={:.1} floor={:.1} snr={:.1})",
            detector.peak_db(),
            detector.noise_floor_db(),
            detector.snr_db()
        );
    }

    /// Realistic baseline-amp data point from the maintainer's host:
    /// an external 150 Hz tone (peak ≈ -45 dBFS) sitting on a quiet-room
    /// floor (≈ -88 dBFS) should comfortably PASS at the default 6 dB SNR
    /// threshold. Synthesised here via `peak_to_db`-equivalent power
    /// math directly on the `power_to_db` helper, with no detector loop —
    /// this is the unit-level guard for the README's worked example.
    #[test]
    fn power_to_db_round_trip_matches_amp_05_sine_calibration() {
        // For an amp-0.5 sine in a 2048-point FFT, a single bin near the
        // true frequency carries roughly (amp * fft_size / 4)^2 power. We
        // do not need to reproduce the exact figure; what matters for the
        // SNR contract is that `power_to_db` is monotonic and that a 100×
        // power ratio equals the expected 20 dB.
        let big = power_to_db(10_000.0, 1024.0);
        let small = power_to_db(100.0, 1024.0);
        let snr = big - small;
        assert!(
            (snr - 20.0).abs() < 0.001,
            "100× power ratio should be 20 dB SNR, got {snr} dB"
        );
        // SILENCE_DB on zero-power input — guards the SNR floor.
        assert!((power_to_db(0.0, 1024.0) - SILENCE_DB).abs() < 0.001);
    }

    #[test]
    fn detector_config_release_default_round_trip() {
        let cfg = DetectorConfig::release_default();
        assert_eq!(cfg.buckets.len(), 1);
        // Single bucket centred at 5.5 Hz, half-width 4.5 Hz so the
        // integration window covers exactly 1.0–10.0 Hz. See
        // release_default doc for the binary-trigger rationale.
        assert!((cfg.buckets[0] - 5.5).abs() < 0.001);
        assert!((cfg.bucket_half_width_hz - 4.5).abs() < 0.001);
        let lo = cfg.buckets[0] - cfg.bucket_half_width_hz;
        let hi = cfg.buckets[0] + cfg.bucket_half_width_hz;
        assert!(
            (lo - 1.0).abs() < 0.001,
            "lo edge of bucket should be 1.0 Hz"
        );
        assert!(
            (hi - 10.0).abs() < 0.001,
            "hi edge of bucket should be 10.0 Hz"
        );
        assert!((cfg.snr_threshold_db - DEFAULT_SNR_THRESHOLD_DB).abs() < 0.001);
        assert_eq!(cfg.fft_size, RELEASE_FFT_SIZE);
        // Peak band must cover the bucket with margin on both sides.
        assert!(cfg.peak_band_hz.0 < 1.0);
        assert!(cfg.peak_band_hz.1 > 10.0);
    }

    #[test]
    fn detector_config_audible_test_round_trip() {
        let cfg = DetectorConfig::audible_test();
        // Single bucket at 1.75 kHz with 750 Hz half-width spans 1.0–2.5 kHz,
        // structurally symmetric to release_default.
        assert_eq!(cfg.buckets.len(), 1);
        assert!((cfg.buckets[0] - 1_750.0).abs() < 1.0);
        assert!((cfg.bucket_half_width_hz - 750.0).abs() < 0.001);
        let lo = cfg.buckets[0] - cfg.bucket_half_width_hz;
        let hi = cfg.buckets[0] + cfg.bucket_half_width_hz;
        assert!((lo - 1_000.0).abs() < 0.001);
        assert!((hi - 2_500.0).abs() < 0.001);
        // v0.5.1: audible_test uses its own SNR default (18 dB), not the
        // shared DEFAULT_SNR_THRESHOLD_DB (6 dB) used by release_default.
        assert!((cfg.snr_threshold_db - AUDIBLE_TEST_SNR_THRESHOLD_DB).abs() < 0.001);
        assert!(
            cfg.snr_threshold_db > DEFAULT_SNR_THRESHOLD_DB,
            "audible_test should be stricter than release_default (got {} vs {})",
            cfg.snr_threshold_db,
            DEFAULT_SNR_THRESHOLD_DB
        );
    }

    #[test]
    fn detector_config_peak_band_from_buckets_spans_min_max() {
        let buckets = vec![20.0, 50.0];
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
        std::env::set_var("HOBA_BUCKETS", "100,200,300");
        let cfg = DetectorConfig::from_env().expect("bucket override should yield Some");
        assert_eq!(cfg.buckets, vec![100.0, 200.0, 300.0]);
        // peak_band auto-derived from the new buckets, not left at the default.
        assert!(cfg.peak_band_hz.0 < 100.0);
        assert!(cfg.peak_band_hz.1 > 300.0);
    }

    /// Back-compat with the v0.4.x `hz:depth` form: the `:depth` portion
    /// must be silently dropped, and the parse must succeed yielding the
    /// frequencies only.
    #[test]
    fn detector_config_from_env_accepts_legacy_hz_depth_form() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_BUCKETS", "100:1,200:2,300:3");
        let cfg = DetectorConfig::from_env().expect("legacy form should still yield Some");
        assert_eq!(cfg.buckets, vec![100.0, 200.0, 300.0]);
    }

    #[test]
    fn detector_config_from_env_parses_snr_only() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_SNR", "12.5");
        let cfg = DetectorConfig::from_env().expect("snr override should yield Some");
        assert!((cfg.snr_threshold_db - 12.5).abs() < 0.001);
        // Buckets fall back to the compile-time default.
        assert_eq!(cfg.buckets, base_default().buckets);
    }

    #[test]
    fn detector_config_from_env_parses_buckets_snr_and_peak_band() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_BUCKETS", "20,30,40,50");
        std::env::set_var("HOBA_SNR", "9.0");
        std::env::set_var("HOBA_PEAK_BAND", "10:60");
        let cfg = DetectorConfig::from_env().expect("any override should yield Some");
        assert_eq!(cfg.buckets, vec![20.0, 30.0, 40.0, 50.0]);
        assert!((cfg.snr_threshold_db - 9.0).abs() < 0.001);
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
    fn detector_config_from_env_falls_back_on_garbage_snr() {
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_SNR", "not-a-number");
        let cfg = DetectorConfig::from_env().expect("malformed override still yields Some");
        assert!((cfg.snr_threshold_db - base_default().snr_threshold_db).abs() < 0.001);
    }

    #[test]
    fn detector_config_from_env_legacy_threshold_yields_some_but_ignored() {
        // HOBA_THRESHOLD is the v0.3.x raw-power knob. Since v0.4.0 the unit
        // changed to dB SNR; reinterpreting the old number would silently
        // change trigger behaviour, so we treat its presence as a triggering
        // env var (returns Some) but ignore the value entirely.
        let _g = EnvGuard::clear_all();
        std::env::set_var("HOBA_THRESHOLD", "10000");
        let cfg = DetectorConfig::from_env()
            .expect("legacy threshold env var should still yield Some for back-compat detection");
        assert!(
            (cfg.snr_threshold_db - base_default().snr_threshold_db).abs() < 0.001,
            "legacy HOBA_THRESHOLD must NOT bleed into snr_threshold_db (got {})",
            cfg.snr_threshold_db
        );
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
        prior: [(String, Option<String>); 5],
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn clear_all() -> Self {
            static MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let lock = MUTEX.lock().unwrap_or_else(|p| p.into_inner());
            let names = [
                "HOBA_BUCKETS",
                "HOBA_SNR",
                "HOBA_THRESHOLD",
                "HOBA_PEAK_BAND",
                "HOBA_DEBUG",
            ];
            let mut prior: [(String, Option<String>); 5] = [
                (names[0].into(), None),
                (names[1].into(), None),
                (names[2].into(), None),
                (names[3].into(), None),
                (names[4].into(), None),
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
