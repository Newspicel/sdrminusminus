//! Integer-factor polyphase FIR decimation (). Only fully-computed output samples are
//! emitted; the inter-block remainder is carried so arbitrary block sizes stream exactly.

use num_complex::Complex;

use crate::fir::StreamFir;

/// Decimator for complex IQ.
#[derive(Clone, Debug)]
pub struct Decimator {
    core: StreamFir<Complex<f32>, f32>,
}

impl Decimator {
    /// `taps` must be an anti-alias lowpass for `factor` (see [`crate::fir::design_lowpass`])
    /// with at least `factor` taps.
    #[must_use]
    pub fn new(taps: &[f32], factor: usize) -> Self {
        Self {
            core: StreamFir::new(taps, factor),
        }
    }

    /// Replaces `out` with the output samples fully computable from history + `input`.
    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.core.process(input, out);
    }
}

/// Decimator for real samples (audio-rate paths).
#[derive(Clone, Debug)]
pub struct RealDecimator {
    core: StreamFir<f32, f32>,
}

impl RealDecimator {
    /// `taps` must be an anti-alias lowpass for `factor` (see [`crate::fir::design_lowpass`])
    /// with at least `factor` taps.
    #[must_use]
    pub fn new(taps: &[f32], factor: usize) -> Self {
        Self {
            core: StreamFir::new(taps, factor),
        }
    }

    /// Replaces `out` with the output samples fully computable from history + `input`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.core.process(input, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        design_lowpass,
        testutil::{complex_tone, real_tone, rms_c, rms_r},
    };

    const FACTOR: usize = 4;

    fn taps() -> Vec<f32> {
        // −6 dB at 0.11: passband flat to ~0.088, stopband from ~0.132 (2.75/127 half-width).
        design_lowpass(127, 0.11)
    }

    #[test]
    fn inband_complex_tone_survives_within_1_db() {
        let mut d = Decimator::new(&taps(), FACTOR);
        let input = complex_tone(0.05, 4 * 4096);
        let mut out = Vec::new();
        d.process(&input, &mut out);
        let rms = rms_c(&out[64..]);
        assert!((0.89..1.05).contains(&rms), "in-band rms {rms}");
    }

    #[test]
    fn aliasing_tone_suppressed_over_50_db() {
        let mut d = Decimator::new(&taps(), FACTOR);
        // 0.23 of the input rate folds to −0.02 of the output band after ÷4.
        let input = complex_tone(0.23, 4 * 4096);
        let mut out = Vec::new();
        d.process(&input, &mut out);
        let rms = rms_c(&out[64..]);
        assert!(rms < 3.16e-3, "alias leak rms {rms}");
    }

    #[test]
    fn ragged_blocks_match_one_shot_exactly() {
        let input = complex_tone(0.037, 10_000);
        let mut whole = Decimator::new(&taps(), FACTOR);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = Decimator::new(&taps(), FACTOR);
        let mut got = Vec::new();
        let mut block = Vec::new();
        let mut pos = 0;
        for len in [1usize, 7, 64, 3, 129, 1024, 17].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            ragged.process(&input[pos..end], &mut block);
            got.extend_from_slice(&block);
            pos = end;
        }
        assert_eq!(expected, got);
    }

    #[test]
    fn real_decimator_passes_band_and_rejects_alias() {
        let mut d = RealDecimator::new(&taps(), FACTOR);
        let input = real_tone(0.05, 4 * 4096);
        let mut out = Vec::new();
        d.process(&input, &mut out);
        let rms = rms_r(&out[64..]);
        // Unit real tone has RMS 1/√2.
        assert!((0.63..0.75).contains(&rms), "in-band rms {rms}");

        let mut d = RealDecimator::new(&taps(), FACTOR);
        let input = real_tone(0.23, 4 * 4096);
        d.process(&input, &mut out);
        let rms = rms_r(&out[64..]);
        assert!(rms < 3.0e-3, "alias leak rms {rms}");
    }

    #[test]
    #[should_panic(expected = "factor must not exceed the tap count")]
    fn factor_beyond_tap_count_fails_at_construction() {
        let _ = Decimator::new(&[0.25, 0.5, 0.25], 5);
    }

    #[test]
    fn factor_equal_to_tap_count_streams_without_panic() {
        let mut d = Decimator::new(&[0.25, 0.5, 0.25], 3);
        let mut out = Vec::new();
        // The block split that drove the post-emit stride past the buffer end when the
        // factor exceeded the tap count.
        d.process(&complex_tone(0.01, 6), &mut out);
        d.process(&complex_tone(0.01, 3), &mut out);
    }

    #[test]
    fn real_ragged_blocks_match_one_shot_exactly() {
        let input = real_tone(0.041, 10_000);
        let mut whole = RealDecimator::new(&taps(), FACTOR);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = RealDecimator::new(&taps(), FACTOR);
        let mut got = Vec::new();
        let mut block = Vec::new();
        let mut pos = 0;
        for len in [5usize, 1, 250, 33, 999].iter().cycle() {
            if pos >= input.len() {
                break;
            }
            let end = (pos + len).min(input.len());
            ragged.process(&input[pos..end], &mut block);
            got.extend_from_slice(&block);
            pos = end;
        }
        assert_eq!(expected, got);
    }
}
