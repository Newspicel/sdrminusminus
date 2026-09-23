use num_complex::Complex;

const HISTORY: usize = 3;

#[derive(Clone, Debug)]
pub struct CubicInterpolator {
    step: f64,
    t: f64,
    buf: Vec<Complex<f32>>,
}

impl CubicInterpolator {
    #[must_use]
    pub fn new(ratio: f64) -> Self {
        assert!(ratio.is_finite() && ratio > 0.0, "ratio must be positive");
        Self {
            step: ratio.recip(),
            t: 1.0,
            buf: vec![Complex::new(0.0, 0.0); HISTORY],
        }
    }

    pub fn reset(&mut self) {
        self.t = 1.0;
        self.buf.clear();
        self.buf.resize(HISTORY, Complex::new(0.0, 0.0));
    }

    pub fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        out.clear();
        self.buf.extend_from_slice(input);
        while (self.t as usize) + 2 < self.buf.len() {
            let i = self.t as usize;
            let mu = (self.t - i as f64) as f32;
            out.push(catmull_rom(&self.buf[i - 1..=i + 2], mu));
            self.t += self.step;
        }
        let drain = (self.t as usize).saturating_sub(1).min(self.buf.len());
        self.buf.drain(..drain);
        self.t -= drain as f64;
    }
}

#[derive(Clone, Debug)]
pub struct RealInterpolator {
    phases: Vec<Vec<f32>>,
    history: Vec<f32>,
}

impl RealInterpolator {
    #[must_use]
    pub fn new(taps: &[f32], factor: usize) -> Self {
        assert!(factor >= 1, "factor must be >= 1");
        assert!(taps.len() >= factor, "need at least one tap per phase");
        let per_phase = taps.len().div_ceil(factor);
        let gain = factor as f32;
        let phases = (0..factor)
            .map(|phase| {
                (0..per_phase)
                    .map(|k| taps.get(phase + k * factor).copied().unwrap_or(0.0) * gain)
                    .collect()
            })
            .collect();
        Self {
            phases,
            history: vec![0.0; per_phase],
        }
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        let len = self.history.len();
        for &sample in input {
            self.history.copy_within(1.., 0);
            self.history[len - 1] = sample;
            for phase in &self.phases {
                out.push(
                    phase
                        .iter()
                        .zip(self.history.iter().rev())
                        .map(|(h, x)| h * x)
                        .sum(),
                );
            }
        }
    }
}

fn catmull_rom(p: &[Complex<f32>], mu: f32) -> Complex<f32> {
    let c1 = (p[2] - p[0]) * 0.5;
    let c2 = p[0] - p[1] * 2.5 + p[2] * 2.0 - p[3] * 0.5;
    let c3 = (p[3] - p[0]) * 0.5 + (p[1] - p[2]) * 1.5;
    p[1] + (c1 + (c2 + c3 * mu) * mu) * mu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{complex_tone, tone_peak_and_snr};

    #[test]
    fn upsampled_tone_keeps_its_frequency_and_snr() {
        for (ratio, bin) in [
            (2_400_000.0 / 2_000_000.0, 64usize),
            (2_400_000.0 / 2_048_000.0, 64),
            (2.0, 64),
            (20.0 / 3.0, 48),
        ] {
            let mut r = CubicInterpolator::new(ratio);
            let input = complex_tone(ratio * bin as f64 / 4096.0, 8_192);
            let mut out = Vec::new();
            r.process(&input, &mut out);
            let (peak, snr) = tone_peak_and_snr(&out[16..16 + 4096]);
            assert_eq!(peak, bin, "ratio {ratio}: output frequency shifted");
            assert!(snr > 40.0, "ratio {ratio}: snr {snr} dB");
        }
    }

    #[test]
    fn a_pulse_stays_confined_to_its_neighbours() {
        let mut r = CubicInterpolator::new(2.0);
        let mut input = vec![Complex::new(0.0, 0.0); 64];
        input[32] = Complex::new(1.0, 0.0);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.norm().total_cmp(&b.1.norm()))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(out[peak].re, 1.0);
        for (i, y) in out.iter().enumerate() {
            let away = (i as i64 - peak as i64).unsigned_abs();
            assert!(
                away <= 3 || y.norm() == 0.0,
                "sample {i} ({away} from the pulse) reads {y}"
            );
        }
    }

    #[test]
    fn long_run_output_count_matches_ratio() {
        for (ratio, total_in, block) in [
            (1.2, 1_200_000usize, 7_777usize),
            (20.0 / 3.0, 90_000, 9_999),
        ] {
            let mut r = CubicInterpolator::new(ratio);
            let input = complex_tone(0.01, total_in);
            let mut out = Vec::new();
            let mut count = 0i64;
            for chunk in input.chunks(block) {
                r.process(chunk, &mut out);
                count += out.len() as i64;
            }
            let ideal = (total_in as f64 * ratio) as i64;
            assert!(
                (count - ideal).abs() <= 2 + ratio as i64,
                "ratio {ratio}: got {count}, ideal {ideal}"
            );
        }
    }

    #[test]
    fn ragged_blocks_match_one_shot() {
        let input = complex_tone(0.021, 30_000);
        let mut whole = CubicInterpolator::new(1.2);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);

        let mut ragged = CubicInterpolator::new(1.2);
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
        assert!((expected.len() as i64 - got.len() as i64).abs() <= 1);
        for (i, (a, b)) in expected.iter().zip(&got).enumerate() {
            assert!((a - b).norm() < 1e-5, "sample {i}: {a} vs {b}");
        }
    }

    #[test]
    fn reset_leaves_the_state_a_fresh_interpolator_would_have() {
        let input = complex_tone(0.037, 4_000);
        let mut fresh = CubicInterpolator::new(1.5);
        let mut expected = Vec::new();
        fresh.process(&input, &mut expected);

        let mut reused = CubicInterpolator::new(1.5);
        let mut scratch = Vec::new();
        reused.process(&complex_tone(0.11, 2_500), &mut scratch);
        reused.reset();
        let mut got = Vec::new();
        reused.process(&input, &mut got);
        assert_eq!(expected, got);
    }

    #[test]
    fn a_real_interpolator_keeps_an_in_band_tone_and_its_level() {
        let taps = crate::design_lowpass(96, 0.15);
        let mut up = RealInterpolator::new(&taps, 3);
        let input = crate::testutil::real_tone(0.05, 4_000);
        let mut out = Vec::new();
        up.process(&input, &mut out);
        assert_eq!(out.len(), 3 * input.len());
        let level = crate::testutil::rms_r(&out[600..]);
        assert!((0.68..0.73).contains(&level), "interpolated rms {level}");
    }

    #[test]
    fn a_real_interpolator_suppresses_the_images_it_makes() {
        let taps = crate::design_lowpass(96, 0.15);
        let mut up = RealInterpolator::new(&taps, 3);
        let input = crate::testutil::real_tone(0.1, 6_000);
        let mut out = Vec::new();
        up.process(&input, &mut out);
        let settled = &out[600..600 + 4_096];
        let image = settled
            .iter()
            .enumerate()
            .map(|(n, &x)| {
                let p = std::f64::consts::TAU * (1.0 / 3.0 - 0.1 / 3.0) * n as f64;
                Complex::new(x * p.cos() as f32, -x * p.sin() as f32)
            })
            .sum::<Complex<f32>>()
            .norm()
            / settled.len() as f32;
        assert!(image < 1e-3, "image leaked at {image}");
    }

    #[test]
    fn a_real_interpolator_gives_the_same_result_in_ragged_blocks() {
        let taps = crate::design_lowpass(96, 0.15);
        let input = crate::testutil::real_tone(0.07, 3_000);
        let mut whole = RealInterpolator::new(&taps, 3);
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);
        let mut ragged = RealInterpolator::new(&taps, 3);
        let mut got = Vec::new();
        let mut block = Vec::new();
        for chunk in input.chunks(97) {
            ragged.process(chunk, &mut block);
            got.extend_from_slice(&block);
        }
        assert_eq!(expected, got);
    }
}
