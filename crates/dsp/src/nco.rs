use std::f32::consts::PI;

use num_complex::Complex;

#[derive(Clone, Debug)]
pub struct Nco {
    phase: f32,
    step: f32,
}

impl Nco {
    #[must_use]
    pub fn new(freq_hz: f32, sample_rate: f32) -> Self {
        Self {
            phase: 0.0,
            step: wrap_pi(2.0 * PI * freq_hz / sample_rate),
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    pub fn set_freq(&mut self, freq_hz: f32, sample_rate: f32) {
        self.step = wrap_pi(2.0 * PI * freq_hz / sample_rate);
    }

    #[must_use]
    pub fn next_sample(&mut self) -> Complex<f32> {
        let c = Complex::from_polar(1.0, self.phase);
        self.phase += self.step;
        if self.phase >= PI {
            self.phase -= 2.0 * PI;
        } else if self.phase < -PI {
            self.phase += 2.0 * PI;
        }
        c
    }

    pub fn mix_into(&mut self, input: &[Complex<f32>], out: &mut [Complex<f32>]) {
        debug_assert_eq!(input.len(), out.len());
        for (i, o) in input.iter().zip(out.iter_mut()) {
            *o = *i * self.next_sample();
        }
    }

    pub fn mix(&mut self, samples: &mut [Complex<f32>]) {
        for s in samples {
            *s *= self.next_sample();
        }
    }
}

fn wrap_pi(x: f32) -> f32 {
    (x + PI).rem_euclid(2.0 * PI) - PI
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_a_tone_to_baseband_yields_dc() {
        let fs = 48_000.0;
        let f = 6_000.0;
        let mut src = Nco::new(f, fs);
        let tone: Vec<Complex<f32>> = (0..1024).map(|_| src.next_sample()).collect();

        let mut mixer = Nco::new(-f, fs);
        let mut out = vec![Complex::new(0.0, 0.0); tone.len()];
        mixer.mix_into(&tone, &mut out);

        let mean: Complex<f32> = out.iter().sum::<Complex<f32>>() / out.len() as f32;
        assert!(
            (mean.norm() - 1.0).abs() < 1e-3,
            "mean norm {}",
            mean.norm()
        );
    }

    #[test]
    fn mixing_in_place_matches_mixing_into_a_buffer() {
        let fs = 48_000.0;
        let mut src = Nco::new(3_000.0, fs);
        let tone: Vec<Complex<f32>> = (0..4_096).map(|_| src.next_sample()).collect();

        let mut copied = vec![Complex::new(0.0, 0.0); tone.len()];
        Nco::new(-1_000.0, fs).mix_into(&tone, &mut copied);
        let mut in_place = tone.clone();
        Nco::new(-1_000.0, fs).mix(&mut in_place);

        let worst = copied
            .iter()
            .zip(&in_place)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "in-place mix diverged: {worst}");
    }

    #[test]
    fn out_of_range_frequency_aliases_and_stays_bounded() {
        let fs = 48_000.0;
        let mut aliased = Nco::new(1.5 * fs, fs);
        let mut reference = Nco::new(-0.5 * fs, fs);
        let mut max_err = 0.0f32;
        for _ in 0..100_000 {
            let a = aliased.next_sample();
            let r = reference.next_sample();
            max_err = max_err.max((a - r).norm());
        }
        assert!(
            max_err < 1e-2,
            "aliased phasor diverged from reference: {max_err}"
        );
    }
}
