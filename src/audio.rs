//! Audio input abstraction and a sine-wave test fixture.
//!
//! ```
//! use hoba::audio::{Detector, SineSource};
//!
//! let mut detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
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

/// 2048-point FFT: bin width 23.4 Hz at 48 kHz, ~810 cycles of a 19 kHz tone per window.
const FFT_SIZE: usize = 2048;
/// Per-bucket center frequency (Hz) and corresponding mask depth (1–4 low bits cleared).
/// The dominant bucket within 19–21 kHz dictates `Detector::depth`.
const BUCKETS: [(f32, u8); 4] = [(19_000.0, 1), (19_500.0, 2), (20_000.0, 3), (20_500.0, 4)];
/// Half-width of each bucket window in Hz; band power is summed over `center ± this`.
const BUCKET_HALF_WIDTH_HZ: f32 = 100.0;
/// Shared threshold across all four buckets. Calibrated so that pure tones at any
/// bucket center @ amp 0.5 produce band power ~262_144 (verified by the per-bucket
/// tests); threshold sits ~26x below so amplitude swings have headroom.
const POWER_THRESHOLD: f32 = 10_000.0;
/// ~128 ms at 48 kHz with 2048-sample windows. Boundary case (2 windows) covered by tests.
const STREAK_TO_FLIP: u32 = 3;

/// Pulls samples from an [`AudioSource`] and detects ultrasonic trigger tones.
///
/// Note: `Debug` is hand-implemented because the boxed FFT trait object is
/// not itself `Debug`. `Clone` is not implemented because the FFT planner
/// state is not naturally cloneable; sources that hold non-cloneable
/// resources (e.g. a future cpal stream handle in #3) compose cleanly with
/// this.
///
/// The trigger band is approximate. Rectangular-window leakage means tones
/// just outside 19.0–20.5 kHz (within ~50 Hz) still flip the flag. For an
/// "is the environment ultrasonic" check this is desirable; for narrowband
/// classification, post-process the FFT separately.
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
    /// Constructs a detector that reads from `source`.
    pub fn with_source(source: S) -> Self {
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        Self {
            source,
            fft,
            scratch: vec![rustfft::num_complex::Complex::new(0.0, 0.0); FFT_SIZE],
            pull_buf: vec![0.0f32; FFT_SIZE],
            high_streak: 0,
            low_streak: 0,
            depth: 0,
            last_peak_hz: 0.0,
            last_peak_db: -100.0,
        }
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
        for (slot, sample) in self.scratch.iter_mut().zip(self.pull_buf.iter()) {
            *slot = rustfft::num_complex::Complex::new(*sample, 0.0);
        }
        if n < FFT_SIZE {
            for slot in &mut self.scratch[n..] {
                *slot = rustfft::num_complex::Complex::new(0.0, 0.0);
            }
        }
        self.fft.process(&mut self.scratch);

        // Cache peak metrics across the trigger band (covers all four 0.5 kHz buckets
        // plus a small margin for spectral leakage).
        let (peak_bin, peak_norm_sqr) = self.peak_in_band(18_900.0, 20_600.0);
        let bin_hz = f64::from(self.source.sample_rate()) / FFT_SIZE as f64;
        self.last_peak_hz = (peak_bin as f64 * bin_hz) as f32;
        let peak_magnitude = peak_norm_sqr.sqrt();
        let max_magnitude = (FFT_SIZE / 2) as f32;
        self.last_peak_db = if peak_magnitude > 0.0 {
            20.0 * (peak_magnitude / max_magnitude).log10()
        } else {
            -100.0
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
    /// [`poll`](Self::poll). Returns -100.0 if no signal was present.
    pub fn peak_db(&self) -> f32 {
        self.last_peak_db
    }

    fn peak_in_band(&self, lo_hz: f32, hi_hz: f32) -> (usize, f32) {
        let nyquist_bin = FFT_SIZE / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / FFT_SIZE as f64;
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
        let mut best: Option<(f32, u8)> = None;
        for &(center, depth) in &BUCKETS {
            let p = self
                .band_power_between(center - BUCKET_HALF_WIDTH_HZ, center + BUCKET_HALF_WIDTH_HZ);
            if best.map_or(true, |(bp, _)| p > bp) {
                best = Some((p, depth));
            }
        }
        best.and_then(|(p, d)| if p >= POWER_THRESHOLD { Some(d) } else { None })
    }

    fn band_power_between(&self, lo_hz: f32, hi_hz: f32) -> f32 {
        let nyquist_bin = FFT_SIZE / 2;
        let bin_hz = f64::from(self.source.sample_rate()) / FFT_SIZE as f64;
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
    fn detector_depth_19khz_tone_is_1() {
        let mut detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 1);
        assert!(detector.is_compromised());
    }

    #[test]
    fn detector_depth_19_5khz_tone_is_2() {
        let mut detector = Detector::with_source(SineSource::new(19_500.0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 2);
    }

    #[test]
    fn detector_depth_20khz_tone_is_3() {
        let mut detector = Detector::with_source(SineSource::new(20_000.0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 3);
    }

    #[test]
    fn detector_depth_20_5khz_tone_is_4() {
        let mut detector = Detector::with_source(SineSource::new(20_500.0, 0.5));
        for _ in 0..12 {
            detector.poll();
        }
        assert_eq!(detector.depth(), 4);
    }

    #[test]
    fn detector_stays_uncompromised_under_1khz_tone() {
        let mut detector = Detector::with_source(SineSource::new(1_000.0, 0.5));
        for _ in 0..24 {
            detector.poll();
            assert!(
                !detector.is_compromised(),
                "1 kHz tone should not flip the detector"
            );
            assert_eq!(detector.depth(), 0);
        }
    }

    #[test]
    fn detector_does_not_flip_just_below_streak_threshold() {
        let mut samples = sine_window(19_000.0, 0.5, 48_000, FFT_SIZE * 2);
        samples.extend(vec![0.0f32; FFT_SIZE * 10]);
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
        let mut samples = sine_window(19_000.0, 0.5, 48_000, FFT_SIZE * 5);
        samples.extend(vec![0.0f32; FFT_SIZE * 5]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..5 {
            detector.poll();
        }
        assert_eq!(
            detector.depth(),
            1,
            "tone phase should raise depth to 1 (19 kHz bucket)"
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
