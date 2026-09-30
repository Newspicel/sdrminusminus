mod lanes;

use std::{fmt, sync::LazyLock};

use num_complex::Complex;

const TABLE_BITS: u32 = 9;
const TABLE_LEN: usize = 1 << TABLE_BITS;
const LOOKUP_SHIFT: u32 = u32::BITS;
const INDEX_SHIFT: u32 = u32::BITS - TABLE_BITS;
const FRACTION_MASK: u32 = (1 << INDEX_SHIFT) - 1;
const DELTA_PER_STEP: f32 = std::f32::consts::TAU / TABLE_LEN as f32 / (1u32 << INDEX_SHIFT) as f32;
const SIXTH: f32 = 1.0 / 6.0;
const PHASE_SCALE: f64 = 18_446_744_073_709_551_616.0;
const SILENT: Complex<f32> = Complex::new(f32::NAN, f32::NAN);

type Table = [Complex<f32>; TABLE_LEN];

static PHASORS: LazyLock<Table> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let phase = std::f64::consts::TAU * index as f64 / TABLE_LEN as f64;
        let (sin, cos) = phase.sin_cos();
        Complex::new(cos as f32, sin as f32)
    })
});

#[inline(always)]
fn phasor(table: &Table, phase: u64) -> Complex<f32> {
    let top = (phase >> LOOKUP_SHIFT) as u32;
    let first = table[(top >> INDEX_SHIFT) as usize];
    let delta = (top & FRACTION_MASK) as f32 * DELTA_PER_STEP;
    let square = delta * delta;
    let sin = delta * (1.0 - square * SIXTH);
    let cos = 1.0 - square * 0.5;
    Complex::new(
        first.re * cos - first.im * sin,
        first.im * cos + first.re * sin,
    )
}

#[derive(Clone)]
pub struct Nco {
    phase: u64,
    step: u64,
    valid: bool,
    table: &'static Table,
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
    pub const fn is_identity(&self) -> bool {
        self.valid && self.step == 0 && self.phase == 0
    }

    #[must_use]
    pub fn next_sample(&mut self) -> Complex<f32> {
        if !self.valid {
            return SILENT;
        }
        let sample = phasor(self.table, self.phase);
        self.phase = self.phase.wrapping_add(self.step);
        sample
    }

    #[inline(never)]
    pub fn mix_into(&mut self, input: &[Complex<f32>], out: &mut [Complex<f32>]) {
        debug_assert_eq!(input.len(), out.len());
        let len = input.len().min(out.len());
        let (input, out) = (&input[..len], &mut out[..len]);
        if !self.valid {
            out.fill(SILENT);
            return;
        }
        self.phase = lanes::mix_into(self.table, self.phase, self.step, input, out);
    }

    pub fn mix(&mut self, samples: &mut [Complex<f32>]) {
        if !self.valid {
            samples.fill(SILENT);
            return;
        }
        self.phase = lanes::mix(self.table, self.phase, self.step, samples);
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

    fn noise(len: usize) -> Vec<Complex<f32>> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u32 << 23) as f32 - 1.0
        };
        (0..len).map(|_| Complex::new(next(), next())).collect()
    }

    fn started(frequency: f32, lead: usize) -> Nco {
        let mut nco = Nco::new(frequency, 20_000_000.0);
        for _ in 0..lead {
            let _ = nco.next_sample();
        }
        nco
    }

    #[test]
    fn block_mixing_is_bit_identical_to_mixing_sample_by_sample() {
        let input = noise(37);
        for frequency in [0.0, 187_501.25, -731_251.0, 9_999_999.0, -10_000_000.0] {
            for len in 0..=37 {
                let source = &input[..len];
                let mut single = started(frequency, len * 7);
                let expected: Vec<_> = source.iter().map(|x| x * single.next_sample()).collect();
                let mut copied = started(frequency, len * 7);
                let mut out = vec![Complex::new(0.0, 0.0); len];
                copied.mix_into(source, &mut out);
                let mut in_place = started(frequency, len * 7);
                let mut samples = source.to_vec();
                in_place.mix(&mut samples);
                assert_eq!(out, expected, "{frequency} Hz, {len} samples");
                assert_eq!(samples, expected, "{frequency} Hz, {len} samples in place");
                assert_eq!(copied.phase, single.phase);
                assert_eq!(in_place.phase, single.phase);
            }
        }
    }

    #[test]
    fn block_boundaries_do_not_change_the_mix() {
        let input = noise(37 * 38 / 2 + 5);
        let mut whole = started(-731_251.0, 3);
        let mut expected = input.clone();
        whole.mix(&mut expected);
        let mut ragged = started(-731_251.0, 3);
        let mut out = vec![Complex::new(0.0, 0.0); input.len()];
        let mut start = 0;
        for len in (0..=37).chain([5]) {
            let end = start + len;
            ragged.mix_into(&input[start..end], &mut out[start..end]);
            start = end;
        }
        assert_eq!(start, input.len());
        assert_eq!(out, expected);
        assert_eq!(ragged.phase, whole.phase);
    }

    #[test]
    fn an_invalid_frequency_mixes_to_nan_without_moving_the_phase() {
        let mut nco = started(1_000.0, 5);
        let phase = nco.phase;
        nco.set_freq(f32::NAN, 48_000.0);
        let mut samples = noise(11);
        nco.mix(&mut samples);
        let mut out = vec![Complex::new(0.0, 0.0); 11];
        nco.mix_into(&noise(11), &mut out);
        assert!(
            samples
                .iter()
                .chain(&out)
                .all(|s| s.re.is_nan() && s.im.is_nan())
        );
        assert_eq!(nco.phase, phase);
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
