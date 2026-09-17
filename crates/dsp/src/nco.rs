use std::{fmt, sync::LazyLock};

use num_complex::Complex;

const TABLE_BITS: u32 = 9;
const TABLE_LEN: usize = 1 << TABLE_BITS;
const FRACTION_BITS: u32 = u64::BITS - TABLE_BITS;
const FRACTION_MASK: u64 = (1 << FRACTION_BITS) - 1;
const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;

static PHASORS: LazyLock<[Complex<f32>; TABLE_LEN]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let phase = std::f64::consts::TAU * index as f64 / TABLE_LEN as f64;
        let (sin, cos) = phase.sin_cos();
        Complex::new(cos as f32, sin as f32)
    })
});

#[derive(Clone)]
pub struct Nco {
    phase: u64,
    step: u64,
    valid: bool,
    table: &'static [Complex<f32>; TABLE_LEN],
}

impl fmt::Debug for Nco {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nco")
            .field("phase", &self.phase)
            .field("step", &self.step)
            .field("valid", &self.valid)
            .finish()
    }
}

impl Nco {
    #[must_use]
    pub fn new(freq_hz: f32, sample_rate: f32) -> Self {
        let mut nco = Self {
            phase: 0,
            step: 0,
            valid: true,
            table: &PHASORS,
        };
        nco.set_freq(freq_hz, sample_rate);
        nco
    }

    pub fn reset(&mut self) {
        self.phase = 0;
    }

    pub fn set_freq(&mut self, freq_hz: f32, sample_rate: f32) {
        let mut turns = (f64::from(freq_hz) / f64::from(sample_rate)).fract();
        self.valid = turns.is_finite();
        if turns >= 0.5 {
            turns -= 1.0;
        } else if turns < -0.5 {
            turns += 1.0;
        }
        self.step = (turns * PHASE_SCALE).round() as i64 as u64;
    }

    #[must_use]
    pub fn next_sample(&mut self) -> Complex<f32> {
        if !self.valid {
            return Complex::new(f32::NAN, f32::NAN);
        }
        let index = (self.phase >> FRACTION_BITS) as usize;
        let fraction = (self.phase & FRACTION_MASK) as f32 / (1u64 << FRACTION_BITS) as f32;
        let first = self.table[index];
        self.phase = self.phase.wrapping_add(self.step);
        let delta = fraction * (std::f32::consts::TAU / TABLE_LEN as f32);
        let square = delta * delta;
        let sin = delta * (1.0 - square / 6.0);
        let cos = 1.0 - square / 2.0;
        Complex::new(
            first.re * cos - first.im * sin,
            first.im * cos + first.re * sin,
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(turns: f64) -> Complex<f32> {
        let (sin, cos) = (std::f64::consts::TAU * turns.rem_euclid(1.0)).sin_cos();
        Complex::new(cos as f32, sin as f32)
    }

    #[test]
    fn long_runs_match_double_precision_phase_without_amplitude_drift() {
        for frequency in [
            0.0,
            0.001,
            -0.001,
            187_501.25,
            -731_251.0,
            9_999_999.0,
            -10_000_000.0,
        ] {
            let rate = 20_000_000.0;
            let mut nco = Nco::new(frequency, rate);
            let turns = f64::from(frequency) / f64::from(rate);
            let mut worst = 0.0f32;
            for index in 0..2_000_000 {
                let actual = nco.next_sample();
                let expected = reference(index as f64 * turns);
                worst = worst.max((actual - expected).norm());
            }
            assert!(worst < 4e-7, "frequency {frequency}: error {worst}");
        }
    }

    #[test]
    fn lookup_interpolation_stays_below_minus_125_dbc() {
        let len = 65_536;
        for bin in [1, 341, 16_383, 32_767, 65_533] {
            let mut nco = Nco::new(bin as f32, len as f32);
            let mut samples: Vec<_> = (0..len).map(|_| nco.next_sample()).collect();
            crate::fft::FftPair::new(len).forward(&mut samples);
            let carrier = samples[bin].norm();
            let spur = samples
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != bin)
                .map(|(_, sample)| sample.norm())
                .fold(0.0f32, f32::max);
            let dbc = 20.0 * (spur / carrier).log10();
            assert!(dbc < -125.0, "bin {bin}: spur {dbc} dBc");
        }
    }

    #[test]
    fn retuning_preserves_phase_and_reset_restarts_at_unity() {
        let rate = 48_000.0;
        let mut nco = Nco::new(137.0, rate);
        let mut turns = 0.0;
        for frequency in [137.0, -913.0, 0.0, 24_000.0, 1.0] {
            nco.set_freq(frequency, rate);
            for _ in 0..4_099 {
                assert!((nco.next_sample() - reference(turns)).norm() < 4e-7);
                turns += f64::from(frequency) / f64::from(rate);
            }
        }
        nco.reset();
        assert_eq!(nco.next_sample(), Complex::new(1.0, 0.0));
    }

    #[test]
    fn ragged_mixing_matches_one_shot() {
        let mut nco = Nco::new(731.0, 48_000.0);
        let mut samples = [Complex::new(1.0, 0.5); 4_099];
        let mut reference = nco.clone();
        let mut expected = samples;
        reference.mix(&mut expected);
        for chunk in samples.chunks_mut(17) {
            nco.mix(chunk);
        }
        assert_eq!(samples, expected);
    }

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
