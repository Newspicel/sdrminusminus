use num_complex::Complex;

use crate::fir::{design_lowpass, dot};

const PHASES: usize = 128;

pub(crate) fn taps_per_phase(ratio: f64) -> usize {
    (5.5 / (0.1 * ratio.min(1.0))).ceil() as usize
}

#[derive(Clone, Debug)]
pub struct FracResampler {
    rows: Vec<f32>,
    taps_per_phase: usize,
    step: f64,
    position: usize,
    fraction: f64,
    buf: Vec<Complex<f32>>,
}

impl FracResampler {
    #[must_use]
    pub fn new(ratio: f64) -> Self {
        assert!(ratio.is_finite() && ratio > 0.0, "ratio must be positive");
        let band = 0.5 * ratio.min(1.0);
        let taps_per_phase = taps_per_phase(ratio);
        let cutoff = 0.9 * band;
        let proto = design_lowpass(PHASES * taps_per_phase + 1, cutoff / PHASES as f64);
        let mut rows = vec![0.0f32; (PHASES + 1) * taps_per_phase];
        for p in 0..=PHASES {
            let row = &mut rows[p * taps_per_phase..(p + 1) * taps_per_phase];
            for (j, slot) in row.iter_mut().enumerate() {
                *slot = proto[j * PHASES + p];
            }
            let sum: f32 = row.iter().sum();
            debug_assert!(sum > 0.0, "degenerate polyphase branch");
            for v in row.iter_mut() {
                *v /= sum;
            }
            row.reverse();
        }
        Self {
            rows,
            taps_per_phase,
            step: ratio.recip(),
            position: taps_per_phase - 1,
            fraction: 0.0,
            buf: vec![Complex::new(0.0, 0.0); taps_per_phase - 1],
        }
    }

    pub fn reset(&mut self) {
        self.position = self.taps_per_phase - 1;
        self.fraction = 0.0;
        self.buf.clear();
        self.buf
            .resize(self.taps_per_phase - 1, Complex::new(0.0, 0.0));
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        self.buf.extend_from_slice(input);
        let tpp = self.taps_per_phase;
        while self.position < self.buf.len() {
            let n = self.position;
            let phase = self.fraction * PHASES as f64;
            let p = phase as usize;
            let mu = (phase - p as f64) as f32;
            let window = &self.buf[n + 1 - tpp..=n];
            let a = dot(window, &self.rows[p * tpp..(p + 1) * tpp]);
            let b = dot(window, &self.rows[(p + 1) * tpp..(p + 2) * tpp]);
            out.push(a + (b - a) * mu);
            self.fraction += self.step;
            let advance = self.fraction as usize;
            self.position += advance;
            self.fraction -= advance as f64;
        }
        let drain = self.position.saturating_sub(tpp - 1).min(self.buf.len());
        self.buf.drain(..drain);
        self.position -= drain;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{complex_tone, rms_c, tone_peak_and_snr};

    #[test]
    fn ragged_resampling_matches_double_precision_convolution() {
        let input: Vec<_> = (0..16_381)
            .map(|index| {
                Complex::new(
                    ((index * 37) % 251) as f32 / 251.0 - 0.5,
                    ((index * 71) % 257) as f32 / 257.0 - 0.5,
                )
            })
            .collect();
        for ratio in [0.2, 0.75, 0.768, 0.96, 48_000.0 / 44_100.0, 1.2] {
            let mut resampler = FracResampler::new(ratio);
            let taps = resampler.taps_per_phase;
            let mut history = vec![Complex::new(0.0, 0.0); taps - 1];
            history.extend_from_slice(&input);
            let mut expected = Vec::new();
            let mut time = (taps - 1) as f64;
            while (time as usize) < history.len() {
                let sample = time as usize;
                let phase = (time - sample as f64) * PHASES as f64;
                let row = phase as usize;
                let fraction = phase - row as f64;
                let mut sum = Complex::new(0.0f64, 0.0);
                for (index, value) in history[sample + 1 - taps..=sample].iter().enumerate() {
                    let a = f64::from(resampler.rows[row * taps + index]);
                    let b = f64::from(resampler.rows[(row + 1) * taps + index]);
                    sum += Complex::new(f64::from(value.re), f64::from(value.im))
                        * (a + (b - a) * fraction);
                }
                expected.push(sum);
                time += ratio.recip();
            }
            let mut actual = Vec::new();
            let mut block = Vec::new();
            for input in input.chunks(997) {
                resampler.process(input, &mut block);
                actual.extend_from_slice(&block);
            }
            assert_eq!(actual.len(), expected.len(), "ratio={ratio}");
            for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                let actual = Complex::new(f64::from(actual.re), f64::from(actual.im));
                assert!(
                    (actual - expected).norm() < 1e-6,
                    "ratio={ratio} sample={index}"
                );
            }
        }
    }

    #[test]
    fn out_of_band_tone_at_5x_downsample_suppressed_over_50_db() {
        let mut r = FracResampler::new(48_000.0 / 240_000.0);
        let input = complex_tone(30_000.0 / 240_000.0, 5 * 4096);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let rms = rms_c(&out[512..]);
        assert!(rms < 3.16e-3, "alias leak rms {rms}");
    }

    #[test]
    fn downsample_5x_keeps_frequency_and_snr() {
        let mut r = FracResampler::new(48_000.0 / 240_000.0);
        let input = complex_tone(100.0 / (4096.0 * 5.0), 5 * 4096 + 1024);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let (peak, snr) = tone_peak_and_snr(&out[64..64 + 4096]);
        assert_eq!(peak, 100, "output frequency shifted");
        assert!(snr > 40.0, "snr {snr} dB");
    }

    #[test]
    fn awkward_ratio_44100_to_48000_keeps_frequency_and_snr() {
        let mut r = FracResampler::new(48_000.0 / 44_100.0);
        let input = complex_tone(1500.0 / 44_100.0, 4400);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let (peak, snr) = tone_peak_and_snr(&out[64..64 + 4096]);
        assert_eq!(peak, 128, "output frequency shifted");
        assert!(snr > 40.0, "snr {snr} dB");
    }

    #[test]
    fn long_run_output_count_matches_ratio() {
        for (ratio, total_in, ideal_out, block) in [
            (48_000.0 / 240_000.0, 1_200_000usize, 240_000i64, 7_777usize),
            (48_000.0 / 44_100.0, 441_000, 480_000, 9_999),
        ] {
            let mut r = FracResampler::new(ratio);
            let input = complex_tone(0.01, total_in);
            let mut out = Vec::new();
            let mut count = 0i64;
            for chunk in input.chunks(block) {
                r.process(chunk, &mut out);
                count += out.len() as i64;
            }
            assert!(
                (count - ideal_out).abs() <= 2,
                "ratio {ratio}: got {count}, ideal {ideal_out}"
            );
        }
    }

    #[test]
    fn ragged_blocks_match_one_shot() {
        let input = complex_tone(0.021, 30_000);
        let mut whole = FracResampler::new(48_000.0 / 44_100.0);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = FracResampler::new(48_000.0 / 44_100.0);
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
        assert_eq!(expected.len(), got.len());
        for (i, (a, b)) in expected.iter().zip(&got).enumerate() {
            assert_eq!(a, b, "sample {i}");
        }
    }

    #[test]
    fn reset_leaves_the_state_a_fresh_resampler_would_have() {
        let ratio = 48_000.0 / 8_000.0;
        let input = complex_tone(0.037, 4_000);

        let mut fresh = FracResampler::new(ratio);
        let mut expected = Vec::new();
        fresh.process(&input, &mut expected);

        let mut reused = FracResampler::new(ratio);
        let mut scratch = Vec::new();
        reused.process(&complex_tone(0.11, 2_500), &mut scratch);
        reused.reset();
        let mut got = Vec::new();
        reused.process(&input, &mut got);

        assert_eq!(expected.len(), got.len(), "reset shifted the output phase");
        for (i, (a, b)) in expected.iter().zip(&got).enumerate() {
            assert_eq!(a, b, "sample {i}: fresh {a}, reset {b}");
        }
    }
}
