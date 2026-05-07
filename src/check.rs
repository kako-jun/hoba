//! Per-device frequency-response self-test.
//!
//! Backs the `hoba check` CLI subcommand. Sweeps a list of target bands,
//! optionally emitting a pure sine on each, measures `peak_db` near the
//! target with a `Detector`, and prints a verdict per band plus an overall
//! summary.
//!
//! The pure helpers (`format_results`, `decide_verdict`, `BandResult`) are
//! split out from the cpal/IO plumbing so they can be unit-tested without
//! touching real audio devices.

use crate::audio::SILENCE_DB;

/// Default ultrasonic bands probed when the user does not supply `--bands`.
/// Kept as a high-frequency sweep (19–20.5 kHz) because that's a band
/// consumer speakers actually try to reproduce, even badly. The release
/// default's 1–10 Hz infrasound band is *not* meaningful to self-test on
/// commodity hardware — see `hoba check`'s README section.
pub const DEFAULT_BANDS_HZ: &[f32] = &[19_000.0, 19_500.0, 20_000.0, 20_500.0];

/// Half-width of the search window around each target band, in Hz. Wide
/// enough to absorb the bin-quantisation of a 2048-point FFT (~23 Hz at
/// 48 kHz) and small frequency-response shifts in cheap microphones,
/// narrow enough that two adjacent default bands (500 Hz apart) do not
/// pollute each other.
pub const SEARCH_HALF_WIDTH_HZ: f32 = 200.0;

/// Default measurement duration in seconds.
pub const DEFAULT_DURATION_SECS: u64 = 5;

/// Default SNR threshold (dB) the per-band verdict applies when the caller
/// does not supply `--snr`. Mirrors
/// [`crate::audio::DEFAULT_SNR_THRESHOLD_DB`]: 6 dB ≈ 2× signal over
/// noise floor. A `hoba check` PASS therefore means "the band cleared the
/// same SNR threshold the production detector uses," not just "the FFT saw
/// something."
pub const DEFAULT_SNR_THRESHOLD_DB: f32 = crate::audio::DEFAULT_SNR_THRESHOLD_DB;

/// Outcome for a single band.
#[derive(Debug, Clone)]
pub struct BandResult {
    /// 1-based index for human-friendly display.
    pub index: usize,
    /// Target frequency in Hz the user asked for.
    pub target_hz: f32,
    /// Frequency in Hz of the strongest bin observed inside the search window.
    /// `None` if no measurement window completed (e.g. the input stream never
    /// produced samples).
    pub detected_hz: Option<f32>,
    /// Median `peak_db` across all measurement windows, in dBFS.
    /// `None` if no measurement window completed.
    pub peak_db: Option<f32>,
    /// Median `noise_floor_db` across all measurement windows, in dBFS.
    /// `None` if no measurement window completed.
    pub noise_floor_db: Option<f32>,
    /// dB SNR threshold applied to (`peak_db − noise_floor_db`) for the
    /// verdict. Same units as [`crate::audio::DetectorConfig::snr_threshold_db`].
    pub snr_threshold_db: f32,
}

impl BandResult {
    /// Returns the per-band SNR (peak_db − noise_floor_db), or `None` if
    /// either measurement is missing. Floors at 0 dB to match the
    /// detector's `snr_db()` accessor — a negative SNR is information-free
    /// for trigger purposes.
    pub fn snr_db(&self) -> Option<f32> {
        match (self.peak_db, self.noise_floor_db) {
            (Some(p), Some(f)) => Some((p - f).max(0.0)),
            _ => None,
        }
    }

    /// PASS only when we got at least one measurement *and* its SNR is at
    /// or above [`Self::snr_threshold_db`]. A missing measurement is a
    /// FAIL — silent input is exactly the failure mode we want to surface.
    pub fn passed(&self) -> bool {
        match self.snr_db() {
            Some(snr) => snr >= self.snr_threshold_db,
            None => false,
        }
    }
}

/// Renders the result table that goes to stdout. Pure function: deterministic
/// output for fixed inputs, no clocks, no devices, no formatting locale.
///
/// Columns: `peak_db` (signal level), `noise_db` (median floor outside the
/// bucket), `snr_db` (the difference, the actual quantity the verdict checks
/// against `snr_threshold_db`). The "PASS at -49 dB / FAIL at -50 dB" v0.3.x
/// output was fundamentally unreproducible across hosts: a -50 dB peak in a
/// -90 dB quiet room is an obvious signal, the same -50 dB peak in a -45 dB
/// noisy room is meaningless.
pub fn format_results(results: &[BandResult]) -> String {
    let mut out = String::new();
    out.push_str("band  target_hz  detected_hz  peak_db    noise_db    snr_db   verdict\n");
    for r in results {
        let detected = match r.detected_hz {
            Some(hz) => format!("{hz:>11.1}"),
            None => format!("{:>11}", "(no input)"),
        };
        let peak = match r.peak_db {
            Some(db) => format!("{db:>6.1} dB"),
            None => format!("{:>9}", "(silent)"),
        };
        let noise = match r.noise_floor_db {
            Some(db) => format!("{db:>6.1} dB"),
            None => format!("{:>9}", "(silent)"),
        };
        let snr = match r.snr_db() {
            Some(s) => format!("{s:>5.1} dB"),
            None => format!("{:>8}", "—"),
        };
        let verdict = if r.passed() { "PASS" } else { "FAIL" };
        out.push_str(&format!(
            "{:>4}  {:>9.1}  {}  {}  {}  {}  {}\n",
            r.index, r.target_hz, detected, peak, noise, snr, verdict
        ));
    }
    let passed = results.iter().filter(|r| r.passed()).count();
    out.push_str(&format!(
        "\noverall verdict: {}/{} bands usable (SNR threshold {:.1} dB)\n",
        passed,
        results.len(),
        results
            .first()
            .map(|r| r.snr_threshold_db)
            .unwrap_or(DEFAULT_SNR_THRESHOLD_DB),
    ));
    out
}

/// Returns the median of `samples`, or `None` if empty. Sorts a copy in
/// ascending order. NaN samples are filtered out — `peak_db` returns
/// [`SILENCE_DB`] (`-100.0`) for empty windows, never NaN, but the filter
/// keeps the median well-defined regardless.
pub fn median_db(samples: &[f32]) -> Option<f32> {
    let mut clean: Vec<f32> = samples.iter().copied().filter(|x| !x.is_nan()).collect();
    if clean.is_empty() {
        return None;
    }
    clean.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = clean.len();
    Some(if n % 2 == 1 {
        clean[n / 2]
    } else {
        (clean[n / 2 - 1] + clean[n / 2]) / 2.0
    })
}

/// True when *every* result is PASS and the slice is non-empty. Empty
/// `results` is treated as failure: `hoba check` with zero bands has nothing
/// to certify.
pub fn decide_verdict(results: &[BandResult]) -> bool {
    !results.is_empty() && results.iter().all(|r| r.passed())
}

/// Parses the `--bands` argument: a comma-separated list of positive Hz
/// values. For back-compat with v0.4.x, each item may optionally be
/// suffixed with `:<depth>` — the `:depth` portion is silently dropped
/// because the graded depth concept was removed in v0.5.0. Whitespace
/// around items is tolerated; empty list rejected so the CLI never
/// silently runs zero bands.
pub fn parse_bands(s: &str) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    let mut saw_legacy_depth = false;
    for part in s.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Accept the v0.4.x "<hz>:<depth>" form by ignoring the depth.
        let hz_str = match trimmed.split_once(':') {
            Some((hz, _)) => {
                saw_legacy_depth = true;
                hz.trim()
            }
            None => trimmed,
        };
        let hz: f32 = hz_str
            .parse()
            .map_err(|e| format!("invalid frequency '{hz_str}': {e}"))?;
        if !hz.is_finite() || hz <= 0.0 {
            return Err(format!("frequency must be positive and finite: {hz_str}"));
        }
        out.push(hz);
    }
    if out.is_empty() {
        return Err("--bands needs at least one frequency".into());
    }
    if saw_legacy_depth && std::env::var("HOBA_DEBUG").as_deref() == Ok("1") {
        eprintln!(
            "hoba: --bands legacy 'hz:depth' form detected; ':depth' ignored — \
             the graded depth concept was removed in v0.5.0."
        );
    }
    Ok(out)
}

#[cfg(feature = "mic")]
pub use runner::{run_check, CheckOptions};

#[cfg(feature = "mic")]
mod runner {
    use super::*;
    use crate::audio::{AudioSource, Detector};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use ringbuf::traits::{Consumer, Producer, Split};
    use std::time::{Duration, Instant};

    /// Window length used inside the band-by-band measurement loop. ~50 ms at
    /// 48 kHz; the FFT itself runs on 2048 samples (~43 ms) per `Detector::poll`.
    const MEASUREMENT_WINDOW: Duration = Duration::from_millis(50);

    /// Runtime knobs for `run_check`.
    pub struct CheckOptions {
        pub bands: Vec<f32>,
        pub duration: Duration,
        pub listen_only: bool,
        pub input_device: Option<String>,
        pub output_device: Option<String>,
        /// Optional override for the underlying [`Detector`]'s
        /// `snr_threshold_db`, in dB. `None` keeps the compile-time default
        /// ([`crate::audio::DEFAULT_SNR_THRESHOLD_DB`]). Wired to the
        /// `--snr` (alias `--threshold`) CLI flag and to library callers
        /// who need to tighten or loosen the trigger sensitivity per
        /// device.
        pub snr_threshold_db: Option<f32>,
    }

    /// Runs the full sweep and returns one `BandResult` per band.
    pub fn run_check(opts: &CheckOptions) -> Result<Vec<BandResult>, String> {
        let host = cpal::default_host();
        let input_device = pick_input_device(&host, opts.input_device.as_deref())?;
        let output_device = if opts.listen_only {
            None
        } else {
            Some(pick_output_device(&host, opts.output_device.as_deref())?)
        };

        let mut results = Vec::with_capacity(opts.bands.len());
        for (i, &target_hz) in opts.bands.iter().enumerate() {
            let _emitter = match &output_device {
                Some(dev) => Some(start_emitter(dev, target_hz)?),
                None => None,
            };
            let result = measure_band(
                &input_device,
                target_hz,
                opts.duration,
                i + 1,
                opts.snr_threshold_db,
            )?;
            results.push(result);
            // _emitter dropped here, stopping the tone before the next band starts.
        }
        Ok(results)
    }

    fn pick_input_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
        match name {
            Some(want) => host
                .input_devices()
                .map_err(|e| format!("input enumeration failed: {e}"))?
                .find(|d| d.name().map(|n| n == want).unwrap_or(false))
                .ok_or_else(|| format!("input device not found: {want:?}")),
            None => host
                .default_input_device()
                .ok_or_else(|| "no default input device".to_string()),
        }
    }

    fn pick_output_device(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device, String> {
        match name {
            Some(want) => host
                .output_devices()
                .map_err(|e| format!("output enumeration failed: {e}"))?
                .find(|d| d.name().map(|n| n == want).unwrap_or(false))
                .ok_or_else(|| format!("output device not found: {want:?}")),
            None => host
                .default_output_device()
                .ok_or_else(|| "no default output device".to_string()),
        }
    }

    /// Audio source that pulls from a dedicated cpal input stream attached to
    /// `device`. Mirrors the design of `audio::MicSource` but lets the caller
    /// pick the device explicitly (rather than always using the default).
    struct DeviceMicSource {
        sample_rate: u32,
        consumer: ringbuf::HeapCons<f32>,
        _stream: cpal::Stream,
    }

    impl DeviceMicSource {
        fn open(device: &cpal::Device) -> Result<Self, String> {
            use cpal::SampleFormat;
            let config = device
                .default_input_config()
                .map_err(|e| format!("default_input_config: {e}"))?;
            let sample_rate = config.sample_rate().0;
            let channels = usize::from(config.channels());
            if sample_rate == 0 || channels == 0 {
                return Err("input device reported zero rate or channels".into());
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
                    .map_err(|e| format!("build_input_stream (F32): {e}"))?,
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
                    .map_err(|e| format!("build_input_stream (I16): {e}"))?,
                SampleFormat::U16 => device
                    .build_input_stream(
                        &stream_config,
                        move |data: &[u16], _: &_| {
                            for (i, &s) in data.iter().enumerate() {
                                if i % channels == 0 {
                                    let _ = producer.try_push((s as f32 - 32_768.0) / 32_768.0);
                                }
                            }
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| format!("build_input_stream (U16): {e}"))?,
                other => return Err(format!("unsupported input sample format: {other:?}")),
            };
            stream
                .play()
                .map_err(|e| format!("input stream.play: {e}"))?;
            Ok(Self {
                sample_rate,
                consumer,
                _stream: stream,
            })
        }
    }

    impl AudioSource for DeviceMicSource {
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }
        fn read(&mut self, buf: &mut [f32]) -> usize {
            self.consumer.pop_slice(buf)
        }
    }

    /// RAII guard for the output stream. Dropping it stops the tone.
    struct EmitterGuard {
        _stream: cpal::Stream,
    }

    fn start_emitter(device: &cpal::Device, freq_hz: f32) -> Result<EmitterGuard, String> {
        let config = device
            .default_output_config()
            .map_err(|e| format!("default_output_config: {e}"))?;
        let sample_rate = config.sample_rate().0 as f32;
        let channels = config.channels() as usize;
        let mut phase = 0.0f32;
        let step = std::f32::consts::TAU * freq_hz / sample_rate;
        let amplitude = 0.5f32;
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    &config.into(),
                    move |buf: &mut [f32], _| {
                        for frame in buf.chunks_mut(channels) {
                            let s = amplitude * phase.sin();
                            phase += step;
                            if phase > std::f32::consts::TAU {
                                phase -= std::f32::consts::TAU;
                            }
                            for slot in frame.iter_mut() {
                                *slot = s;
                            }
                        }
                    },
                    |err| eprintln!("output stream error: {err}"),
                    None,
                )
                .map_err(|e| format!("build_output_stream: {e}"))?,
            other => return Err(format!("unsupported output sample format: {other:?}")),
        };
        stream
            .play()
            .map_err(|e| format!("output stream.play: {e}"))?;
        Ok(EmitterGuard { _stream: stream })
    }

    fn measure_band(
        input_device: &cpal::Device,
        target_hz: f32,
        duration: Duration,
        index: usize,
        snr_threshold_db: Option<f32>,
    ) -> Result<BandResult, String> {
        let mic = DeviceMicSource::open(input_device)?;
        // Build a single-bucket config centered on `target_hz` so the detector's
        // peak-band reporting tracks the band under test, and let the CLI caller
        // tighten or loosen the SNR threshold per device. The peak band extends
        // beyond the bucket window itself so the noise-floor estimate has bins
        // outside the bucket to draw on.
        let mut config = crate::audio::DetectorConfig::release_default();
        config.buckets = vec![target_hz];
        let lo = (target_hz - SEARCH_HALF_WIDTH_HZ).max(0.0);
        let hi = target_hz + SEARCH_HALF_WIDTH_HZ;
        // bucket_half_width_hz controls both the SNR bucket window and which
        // bins get excluded from the noise floor; a narrow value relative to
        // peak_band_hz (≈ 1/4 of the search half-width) leaves enough out-of-
        // bucket bins inside peak_band_hz to compute a meaningful median.
        config.bucket_half_width_hz = SEARCH_HALF_WIDTH_HZ * 0.25;
        let peak_lo = (target_hz - SEARCH_HALF_WIDTH_HZ * 2.0).max(0.0);
        let peak_hi = target_hz + SEARCH_HALF_WIDTH_HZ * 2.0;
        config.peak_band_hz = (peak_lo, peak_hi);
        if let Some(t) = snr_threshold_db {
            config.snr_threshold_db = t;
        }
        let configured_snr = config.snr_threshold_db;
        let mut detector = Detector::with_config(mic, config);

        let start = Instant::now();
        let mut peak_samples_db: Vec<f32> = Vec::new();
        let mut noise_samples_db: Vec<f32> = Vec::new();
        let mut detected_hzs: Vec<f32> = Vec::new();
        let mut next_window = start + MEASUREMENT_WINDOW;
        while Instant::now() < start + duration {
            detector.poll();
            let peak_hz = detector.peak_hz();
            // Only count windows where the peak fell inside the target window;
            // out-of-band noise is uninformative for "is this band usable?"
            if peak_hz >= lo && peak_hz <= hi {
                let pdb = detector.peak_db();
                let ndb = detector.noise_floor_db();
                if pdb > SILENCE_DB {
                    peak_samples_db.push(pdb);
                    detected_hzs.push(peak_hz);
                }
                if ndb > SILENCE_DB {
                    noise_samples_db.push(ndb);
                }
            }
            let now = Instant::now();
            if now < next_window {
                std::thread::sleep(next_window - now);
            }
            next_window += MEASUREMENT_WINDOW;
        }

        let peak_db = median_db(&peak_samples_db);
        let noise_floor_db = median_db(&noise_samples_db);
        let detected_hz = if detected_hzs.is_empty() {
            None
        } else {
            // Median of detected frequencies, mirroring how peak_db is reduced.
            let mut copy = detected_hzs.clone();
            copy.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            Some(copy[copy.len() / 2])
        };

        Ok(BandResult {
            index,
            target_hz,
            detected_hz,
            peak_db,
            noise_floor_db,
            snr_threshold_db: configured_snr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bands_rejects_empty() {
        assert!(parse_bands("").is_err());
        assert!(parse_bands(",").is_err());
        assert!(parse_bands(" , , ").is_err());
    }

    #[test]
    fn parse_bands_accepts_singleton() {
        let v = parse_bands("19000").unwrap();
        assert_eq!(v, vec![19_000.0]);
    }

    #[test]
    fn parse_bands_accepts_list_with_whitespace() {
        let v = parse_bands(" 19000 , 19500,20000 ").unwrap();
        assert_eq!(v, vec![19_000.0, 19_500.0, 20_000.0]);
    }

    #[test]
    fn parse_bands_rejects_negative_or_zero() {
        assert!(parse_bands("0").is_err());
        assert!(parse_bands("-1").is_err());
        assert!(parse_bands("1000,-1").is_err());
    }

    #[test]
    fn parse_bands_rejects_garbage() {
        assert!(parse_bands("abc").is_err());
        assert!(parse_bands("19000,xyz").is_err());
    }

    /// Back-compat with the v0.4.x `hz:depth` form: the depth must be
    /// silently dropped, leaving a plain frequency list.
    #[test]
    fn parse_bands_accepts_legacy_hz_depth_pairs_dropping_depth() {
        let v = parse_bands("19000:1,19500:2,20000:3").unwrap();
        assert_eq!(v, vec![19_000.0, 19_500.0, 20_000.0]);
    }

    #[test]
    fn median_db_handles_empty() {
        let empty: [f32; 0] = [];
        assert!(median_db(&empty).is_none());
    }

    #[test]
    fn median_db_odd_count() {
        let v = [-50.0, -30.0, -40.0];
        assert_eq!(median_db(&v), Some(-40.0));
    }

    #[test]
    fn median_db_even_count() {
        let v = [-50.0, -30.0, -40.0, -20.0];
        assert_eq!(median_db(&v), Some(-35.0));
    }

    #[test]
    fn median_db_filters_nan() {
        let v = [f32::NAN, -40.0, -30.0];
        assert_eq!(median_db(&v), Some(-35.0));
    }

    fn band(
        idx: usize,
        target: f32,
        detected: Option<f32>,
        peak: Option<f32>,
        noise: Option<f32>,
    ) -> BandResult {
        BandResult {
            index: idx,
            target_hz: target,
            detected_hz: detected,
            peak_db: peak,
            noise_floor_db: noise,
            snr_threshold_db: DEFAULT_SNR_THRESHOLD_DB,
        }
    }

    #[test]
    fn band_result_passed_snr_logic() {
        // Comfortable PASS — 43.5 dB SNR (matches the README's worked
        // baseline-amp data point: 150 Hz peak −44.9 dB on a −88.4 dB
        // floor).
        assert!(band(1, 150.0, Some(149.8), Some(-44.9), Some(-88.4)).passed());
        // Exactly at threshold → PASS (boundary inclusive).
        assert!(band(
            1,
            19_000.0,
            Some(19_010.0),
            Some(-50.0),
            Some(-50.0 - DEFAULT_SNR_THRESHOLD_DB)
        )
        .passed());
        // Below → FAIL (3 dB SNR, half the threshold).
        assert!(!band(1, 19_500.0, Some(19_500.0), Some(-87.0), Some(-90.0)).passed());
        // Missing peak → FAIL.
        assert!(!band(1, 19_000.0, None, None, Some(-90.0)).passed());
        // Missing noise → FAIL (cannot compute SNR).
        assert!(!band(1, 19_000.0, Some(19_010.0), Some(-37.4), None).passed());
    }

    #[test]
    fn band_result_snr_db_floors_at_zero() {
        // Negative SNR (peak below floor) reports as 0 dB to mirror
        // Detector::snr_db.
        assert_eq!(
            band(1, 19_000.0, Some(19_010.0), Some(-90.0), Some(-80.0)).snr_db(),
            Some(0.0)
        );
    }

    #[test]
    fn decide_verdict_all_pass() {
        let results = vec![
            band(1, 19_000.0, Some(19_010.0), Some(-37.4), Some(-90.0)),
            band(2, 19_500.0, Some(19_490.0), Some(-40.0), Some(-90.0)),
        ];
        assert!(decide_verdict(&results));
    }

    #[test]
    fn decide_verdict_one_fail_fails_overall() {
        let results = vec![
            band(1, 19_000.0, Some(19_010.0), Some(-37.4), Some(-90.0)),
            band(2, 19_500.0, Some(19_490.0), Some(-91.2), Some(-90.0)),
        ];
        assert!(!decide_verdict(&results));
    }

    #[test]
    fn decide_verdict_empty_fails() {
        assert!(!decide_verdict(&[]));
    }

    #[test]
    fn format_results_renders_pass_and_fail_rows() {
        let results = vec![
            band(1, 19_000.0, Some(19_014.2), Some(-37.4), Some(-90.0)),
            band(2, 19_500.0, Some(19_478.1), Some(-91.2), Some(-90.0)),
            band(3, 20_000.0, None, None, None),
        ];
        let s = format_results(&results);
        assert!(s.contains("band"));
        assert!(s.contains("snr_db"));
        assert!(s.contains("noise_db"));
        assert!(s.contains("19000.0"));
        assert!(s.contains("19014.2"));
        assert!(s.contains("PASS"));
        assert!(s.contains("FAIL"));
        assert!(s.contains("(silent)"));
        assert!(s.contains("(no input)"));
        assert!(s.contains("1/3 bands usable"));
        assert!(s.contains("SNR threshold"));
    }

    #[test]
    fn format_results_handles_empty_input() {
        let s = format_results(&[]);
        assert!(s.contains("0/0 bands usable"));
    }
}
