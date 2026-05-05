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

/// Pulls samples from an [`AudioSource`] for downstream analysis.
#[derive(Debug, Clone)]
pub struct Detector<S: AudioSource> {
    source: S,
}

impl<S: AudioSource> Detector<S> {
    /// Constructs a detector that reads from `source`.
    pub fn with_source(source: S) -> Self {
        Self { source }
    }

    /// Returns the underlying source's sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    /// Reads samples from the underlying source into `buf`.
    pub fn read(&mut self, buf: &mut [f32]) -> usize {
        self.source.read(buf)
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
            assert!((a[i] - combined[i]).abs() < 1e-6);
            assert!((b[i] - combined[i + 256]).abs() < 1e-6);
        }
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
        let tolerance = (expected as f32 * 0.05) as i32;
        let diff = (crossings as i32 - expected).abs();
        assert!(
            diff <= tolerance,
            "expected ~{expected} zero crossings, got {crossings}"
        );
    }
}
