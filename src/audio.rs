//! Audio input abstraction and a sine-wave test fixture.
//!
//! ```
//! use hoba::audio::{AudioSource, Detector, SineSource};
//!
//! let mut detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
//! let mut buf = [0.0f32; 1024];
//! let n = detector.read(&mut buf);
//! assert_eq!(n, buf.len());
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

const FFT_SIZE: usize = 2048;
const TRIGGER_BAND_HZ: (f32, f32) = (19_000.0, 20_500.0);
/// Pure 19 kHz @ amp 0.5 yields band power around 262_144; threshold sits well below that.
const POWER_THRESHOLD: f32 = 10_000.0;
/// ~128 ms at 48 kHz with 2048-sample windows.
const STREAK_TO_FLIP: u32 = 3;

/// Pulls samples from an [`AudioSource`] and detects ultrasonic trigger tones.
///
/// Note: `Debug` is hand-implemented because the boxed FFT trait object is
/// not itself `Debug`. `Clone` is not implemented because the FFT planner
/// state is not naturally cloneable; sources that hold non-cloneable
/// resources (e.g. a future cpal stream handle in #3) compose cleanly with
/// this.
pub struct Detector<S: AudioSource> {
    source: S,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    scratch: Vec<rustfft::num_complex::Complex<f32>>,
    high_streak: u32,
    low_streak: u32,
    compromised: bool,
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
    /// Constructs a detector that reads from `source`.
    pub fn with_source(source: S) -> Self {
        let mut planner = rustfft::FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        Self {
            source,
            fft,
            scratch: vec![rustfft::num_complex::Complex::new(0.0, 0.0); FFT_SIZE],
            high_streak: 0,
            low_streak: 0,
            compromised: false,
        }
    }

    /// Returns the underlying source's sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    /// Reads samples from the underlying source into `buf`. Does not advance FFT state — call `poll` for that.
    pub fn read(&mut self, buf: &mut [f32]) -> usize {
        self.source.read(buf)
    }

    /// Pulls one FFT window worth of samples, runs an FFT, updates the compromised flag, and returns it.
    pub fn poll(&mut self) -> bool {
        let mut buf = vec![0.0f32; FFT_SIZE];
        let n = self.source.read(&mut buf);
        for (slot, sample) in self.scratch.iter_mut().zip(buf.iter()) {
            *slot = rustfft::num_complex::Complex::new(*sample, 0.0);
        }
        if n < FFT_SIZE {
            for slot in &mut self.scratch[n..] {
                *slot = rustfft::num_complex::Complex::new(0.0, 0.0);
            }
        }
        self.fft.process(&mut self.scratch);

        let band_power = self.band_power();
        if band_power >= POWER_THRESHOLD {
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
        self.compromised
    }

    /// Returns whether the detector is currently flagging a trigger condition.
    pub fn is_compromised(&self) -> bool {
        self.compromised
    }

    fn band_power(&self) -> f32 {
        let bin_hz = self.source.sample_rate() as f32 / FFT_SIZE as f32;
        let lo_bin = (TRIGGER_BAND_HZ.0 / bin_hz).floor() as usize;
        let hi_bin = ((TRIGGER_BAND_HZ.1 / bin_hz).ceil() as usize).min(FFT_SIZE / 2);
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
    fn detector_pulls_samples_from_sine_source() {
        let mut detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
        let mut buf = [0.0f32; 1024];
        let n = detector.read(&mut buf);
        assert_eq!(n, 1024);
        assert!(buf.iter().any(|s| *s != 0.0));
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
    fn detector_flags_compromised_under_19khz_tone() {
        let mut detector = Detector::with_source(SineSource::new(19_000.0, 0.5));
        let mut flipped = false;
        for _ in 0..24 {
            if detector.poll() {
                flipped = true;
                break;
            }
        }
        assert!(flipped, "detector did not flip under sustained 19 kHz tone");
        assert!(detector.is_compromised());
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
        }
    }

    #[test]
    fn detector_does_not_flip_on_single_window_spike() {
        let mut samples = sine_window(19_000.0, 0.5, 48_000, FFT_SIZE);
        samples.extend(vec![0.0f32; FFT_SIZE * 10]);
        let source = ScriptedSource::new(samples, 48_000);
        let mut detector = Detector::with_source(source);
        for _ in 0..11 {
            detector.poll();
            assert!(
                !detector.is_compromised(),
                "single-window spike should not flip the detector"
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
        assert!(
            detector.is_compromised(),
            "tone phase should flip the detector to true"
        );
        for _ in 0..5 {
            detector.poll();
        }
        assert!(
            !detector.is_compromised(),
            "silence phase should flip the detector back to false"
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
}
