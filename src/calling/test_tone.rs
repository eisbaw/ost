//! Test tone generation and echo detection for call testing.
//!
//! Generates a 1kHz sine wave as PCMU audio frames, and detects echo
//! in recorded audio by matched-filter cross-correlation.

/// Generates 1kHz sine wave frames for test audio.
pub struct ToneGenerator {
    phase: f64,
    frequency: f64,
    sample_rate: f64,
    amplitude: f64,
}

impl ToneGenerator {
    pub fn new() -> Self {
        Self {
            phase: 0.0,
            frequency: 1000.0,
            sample_rate: 8000.0,
            amplitude: 0.8,
        }
    }

    /// Generate the next 160-sample frame (20ms at 8kHz).
    pub fn next_frame(&mut self) -> Vec<i16> {
        let mut samples = Vec::with_capacity(160);
        let phase_inc = 2.0 * std::f64::consts::PI * self.frequency / self.sample_rate;
        for _ in 0..160 {
            let val = (self.phase.sin() * self.amplitude * 32767.0) as i16;
            samples.push(val);
            self.phase += phase_inc;
            if self.phase > 2.0 * std::f64::consts::PI {
                self.phase -= 2.0 * std::f64::consts::PI;
            }
        }
        samples
    }
}

/// Records received audio samples for later analysis.
pub struct AudioRecorder {
    samples: Vec<i16>,
    max_samples: usize,
}

impl AudioRecorder {
    pub fn new(max_samples: usize) -> Self {
        Self {
            samples: Vec::with_capacity(max_samples),
            max_samples,
        }
    }

    pub fn push_frame(&mut self, frame: &[i16]) {
        let remaining = self.max_samples.saturating_sub(self.samples.len());
        let take = frame.len().min(remaining);
        self.samples.extend_from_slice(&frame[..take]);
    }

    pub fn samples(&self) -> &[i16] {
        &self.samples
    }
}

/// Result of echo detection analysis.
#[derive(Debug)]
pub struct EchoResult {
    pub detected: bool,
    pub delay_ms: f64,
    pub correlation_peak: f64,
}

/// Detect echo of a tone in recorded audio using matched filter.
///
/// Slides a one-period reference sine over `received` and computes
/// normalized cross-correlation. A peak above threshold means the
/// tone was echoed back.
pub fn detect_echo(received: &[i16], tone_freq: f64, sample_rate: f64) -> EchoResult {
    let period_samples = (sample_rate / tone_freq) as usize;
    if received.len() < period_samples * 2 {
        return EchoResult {
            detected: false,
            delay_ms: 0.0,
            correlation_peak: 0.0,
        };
    }

    // Generate one period of reference sine
    let mut reference = Vec::with_capacity(period_samples);
    for i in 0..period_samples {
        let t = i as f64 / sample_rate;
        reference.push((2.0 * std::f64::consts::PI * tone_freq * t).sin());
    }

    // Compute reference energy
    let ref_energy: f64 = reference.iter().map(|x| x * x).sum();

    let mut best_corr = 0.0f64;
    let mut best_offset = 0usize;

    // Slide reference over received signal
    let max_offset = received.len() - period_samples;
    for offset in 0..max_offset {
        let mut cross = 0.0f64;
        let mut sig_energy = 0.0f64;
        for (i, &r) in reference.iter().enumerate() {
            let s = received[offset + i] as f64 / 32768.0;
            cross += r * s;
            sig_energy += s * s;
        }

        let denom = (ref_energy * sig_energy).sqrt();
        let corr = if denom > 1e-10 { cross / denom } else { 0.0 };

        if corr.abs() > best_corr.abs() {
            best_corr = corr;
            best_offset = offset;
        }
    }

    let threshold = 0.3;
    let delay_ms = best_offset as f64 / sample_rate * 1000.0;

    EchoResult {
        detected: best_corr.abs() > threshold,
        delay_ms,
        correlation_peak: best_corr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tone_generator_output() {
        let mut gen = ToneGenerator::new();
        let frame = gen.next_frame();
        assert_eq!(frame.len(), 160);
        // Should have positive and negative values (sine wave)
        assert!(frame.iter().any(|&s| s > 1000));
        assert!(frame.iter().any(|&s| s < -1000));
    }

    #[test]
    fn test_recorder() {
        let mut rec = AudioRecorder::new(320);
        rec.push_frame(&vec![100i16; 160]);
        rec.push_frame(&vec![200i16; 160]);
        assert_eq!(rec.samples().len(), 320);
        // Should not exceed max
        rec.push_frame(&vec![300i16; 160]);
        assert_eq!(rec.samples().len(), 320);
    }

    #[test]
    fn test_detect_echo_with_tone() {
        // Generate a 1kHz tone and feed it as "received" audio
        let mut gen = ToneGenerator::new();
        let mut samples = Vec::new();
        // Add some silence then the tone
        samples.extend_from_slice(&vec![0i16; 400]); // 50ms silence
        for _ in 0..25 {
            samples.extend_from_slice(&gen.next_frame());
        }
        let result = detect_echo(&samples, 1000.0, 8000.0);
        assert!(
            result.detected,
            "Should detect echo, peak={}",
            result.correlation_peak
        );
        assert!(result.delay_ms > 0.0);
    }

    #[test]
    fn test_detect_echo_silence() {
        let samples = vec![0i16; 8000];
        let result = detect_echo(&samples, 1000.0, 8000.0);
        assert!(!result.detected);
    }
}
