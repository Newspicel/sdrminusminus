use num_complex::Complex;

use crate::{Decimator, design_lowpass};

mod bank;
pub use bank::SubbandFilterBank;

const FACTOR: usize = 5;
const PASSBAND: f64 = 0.3;
const MIXER_PERIOD: usize = 25;
const MIXER_STEP: usize = 2;
const MIXER_BLOCK: usize = 256;
const SPACING: f64 = (MIXER_STEP * FACTOR) as f64 / MIXER_PERIOD as f64;
const HALF_BANDS: usize = 6;
pub const SUBBANDS: usize = 2 * HALF_BANDS + 1;

#[derive(Clone, Copy, Debug)]
pub struct SubbandPlan {
    input_rate: f64,
}

impl SubbandPlan {
    #[must_use]
    pub fn new(input_rate: f64) -> Option<Self> {
        (input_rate.is_finite() && input_rate >= 5_000_000.0).then_some(Self { input_rate })
    }

    #[must_use]
    pub fn output_rate(self) -> f64 {
        self.input_rate / FACTOR as f64
    }

    #[must_use]
    pub fn bandwidth(self) -> f64 {
        2.0 * PASSBAND * self.output_rate()
    }

    #[must_use]
    pub fn center(self, band: usize) -> f64 {
        (band as f64 - HALF_BANDS as f64) * SPACING * self.output_rate()
    }

    #[must_use]
    pub fn select(self, offset: f64, bandwidth: f64) -> Option<usize> {
        if !offset.is_finite() || !bandwidth.is_finite() || bandwidth <= 0.0 {
            return None;
        }
        let half_width = bandwidth / 2.0;
        if offset.abs() + half_width > self.input_rate / 2.0 {
            return None;
        }
        let at = (offset / (SPACING * self.output_rate())).round() + HALF_BANDS as f64;
        if !(0.0..SUBBANDS as f64).contains(&at) {
            return None;
        }
        let band = at as usize;
        ((offset - self.center(band)).abs() + half_width <= PASSBAND * self.output_rate())
            .then_some(band)
    }

    #[must_use]
    pub fn decimator(self, band: usize, block_len: usize) -> SubbandDecimator {
        let mut result = SubbandDecimator {
            mixer: (band != HALF_BANDS).then(|| PeriodicMixer::new(band)),
            filter: Decimator::new(&prototype(), FACTOR),
            mixed: vec![Complex::new(0.0, 0.0); block_len],
        };
        let mut output = Vec::new();
        result.filter.process(&result.mixed, &mut output);
        result.reset();
        result
    }

    #[must_use]
    pub fn filter_bank(self, block_len: usize) -> SubbandFilterBank {
        SubbandFilterBank::new(block_len)
    }

    #[must_use]
    pub const fn alignment(self) -> usize {
        MIXER_PERIOD
    }

    #[must_use]
    pub fn history_len(self) -> usize {
        (prototype_len().div_ceil(MIXER_PERIOD) + 1) * MIXER_PERIOD
    }
}

fn prototype_len() -> usize {
    ((5.5 * FACTOR as f64 / (1.0 - 2.0 * PASSBAND)).ceil() as usize) | 1
}

fn prototype() -> Vec<f32> {
    design_lowpass(prototype_len(), 0.5 / FACTOR as f64)
}

#[derive(Clone, Debug)]
pub struct SubbandDecimator {
    mixer: Option<PeriodicMixer>,
    filter: Decimator,
    mixed: Vec<Complex<f32>>,
}

impl SubbandDecimator {
    pub fn reset(&mut self) {
        if let Some(mixer) = &mut self.mixer {
            mixer.phase = 0;
        }
        self.filter.reset();
    }

    pub fn process(&mut self, input: &[Complex<f32>], output: &mut Vec<Complex<f32>>) {
        if let Some(mixer) = &mut self.mixer {
            self.mixed.resize(input.len(), Complex::new(0.0, 0.0));
            mixer.mix_into(input, &mut self.mixed);
            self.filter.process(&self.mixed, output);
        } else {
            self.filter.process(input, output);
        }
    }
}

#[derive(Clone, Debug)]
struct PeriodicMixer {
    carrier: [Complex<f32>; MIXER_PERIOD + MIXER_BLOCK - 1],
    phase: usize,
}

impl PeriodicMixer {
    fn new(band: usize) -> Self {
        let frequency =
            -(band as f64 - HALF_BANDS as f64) * MIXER_STEP as f64 / MIXER_PERIOD as f64;
        let carrier = std::array::from_fn(|index| {
            let angle = std::f64::consts::TAU * frequency * (index % MIXER_PERIOD) as f64;
            let (sin, cos) = angle.sin_cos();
            Complex::new(cos as f32, sin as f32)
        });
        Self { carrier, phase: 0 }
    }

    fn mix_into(&mut self, input: &[Complex<f32>], output: &mut [Complex<f32>]) {
        for (input, output) in input
            .chunks(MIXER_BLOCK)
            .zip(output.chunks_mut(MIXER_BLOCK))
        {
            let carrier = &self.carrier[self.phase..self.phase + input.len()];
            for ((input, output), carrier) in input.iter().zip(output).zip(carrier) {
                *output = *input * *carrier;
            }
            self.phase = (self.phase + input.len()) % MIXER_PERIOD;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Nco,
        testutil::{complex_tone, rms_c},
    };

    #[test]
    fn periodic_mixers_match_the_grid_without_losing_phase() {
        let input = complex_tone(0.017, 4099);
        let mut output = vec![Complex::new(0.0, 0.0); input.len()];
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        for band in 0..SUBBANDS {
            let mut mixer = PeriodicMixer::new(band);
            let mut at = 0;
            for size in [0, 1, 24, 25, 26, 255, 256, 257, 4099]
                .into_iter()
                .cycle()
                .take(90)
            {
                mixer.mix_into(&input[..size], &mut output[..size]);
                for index in 0..size {
                    let angle = -std::f64::consts::TAU * plan.center(band) / plan.input_rate
                        * (at + index) as f64;
                    let (sin, cos) = angle.sin_cos();
                    let expected = input[index] * Complex::new(cos as f32, sin as f32);
                    assert!((output[index] - expected).norm() < 2e-6);
                }
                at += size;
            }
        }
    }

    #[test]
    fn selection_covers_narrow_channels_and_rejects_unprotected_bands() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        for offset in (-9_000_000..=9_000_000).step_by(25_000) {
            let band = plan.select(f64::from(offset), 240_000.0).unwrap();
            assert!((f64::from(offset) - plan.center(band)).abs() + 120_000.0 <= 1_200_000.0);
        }
        for (offset, bandwidth) in [
            (0.0, 3_000_000.0),
            (9_990_000.0, 48_000.0),
            (f64::NAN, 48_000.0),
            (0.0, 0.0),
        ] {
            assert!(plan.select(offset, bandwidth).is_none());
        }
        assert!(SubbandPlan::new(3_200_000.0).is_none());
        assert!(SubbandPlan::new(f64::INFINITY).is_none());
    }

    #[test]
    fn every_subband_preserves_its_passband_and_rejects_folding_blockers() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        for band in 0..SUBBANDS {
            let center = plan.center(band);
            let mut decimator = plan.decimator(band, 2048);
            let mut output = Vec::new();
            for relative in [-0.3, 0.0, 0.3, -0.7, 0.7, -1.0, 1.0, -2.0, 2.0] {
                let frequency = center + relative * plan.output_rate();
                let input = complex_tone(frequency / plan.input_rate, 20_000);
                decimator.reset();
                decimator.process(&input, &mut output);
                let level = rms_c(&output[128..]);
                if relative.abs() <= PASSBAND {
                    assert!(
                        (0.99..1.01).contains(&level),
                        "band={band} relative={relative} level={level}"
                    );
                } else {
                    assert!(
                        level < 0.00316,
                        "band={band} relative={relative} blocker={level}"
                    );
                }
            }
        }
    }

    #[test]
    fn ragged_blocks_and_resets_preserve_samples_and_output_count() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        let input = complex_tone(0.125, 16_389);
        let mut whole = plan.decimator(8, input.len());
        let mut expected = Vec::new();
        whole.process(&input, &mut expected);
        assert_eq!(expected.len(), input.len().div_ceil(FACTOR));
        let mut ragged = plan.decimator(8, 4099);
        let mut actual = Vec::new();
        let mut block = Vec::new();
        let mut at = 0;
        for size in [0, 1, 17, 2048, 4099].into_iter().cycle() {
            let end = (at + size).min(input.len());
            ragged.process(&input[at..end], &mut block);
            actual.extend_from_slice(&block);
            at = end;
            if at == input.len() {
                break;
            }
        }
        assert_eq!(actual, expected);
        ragged.reset();
        ragged.process(&input, &mut actual);
        assert_eq!(actual, expected);
    }

    #[test]
    fn downstream_fm_rejects_a_strong_blocker_that_folds_onto_the_channel() {
        use std::f64::consts::TAU;

        use crate::{Ddc, FmDemod};

        let rate = 20_000_000.0;
        let offset = 1_700_000.0;
        let plan = SubbandPlan::new(rate).unwrap();
        let band = plan.select(offset, 48_000.0).unwrap();
        let mut coarse = plan.decimator(band, 2048);
        let mut ddc = Ddc::new(plan.output_rate(), 48_000.0, offset - plan.center(band)).unwrap();
        let mut carrier = Nco::new(offset as f32, rate as f32);
        let mut blocker = Nco::new((offset + plan.output_rate()) as f32, rate as f32);
        let input: Vec<_> = (0..2_000_000)
            .map(|index| {
                let phase = 2.5 * (TAU * 1000.0 * index as f64 / rate).sin();
                carrier.next_sample() * Complex::from_polar(1.0, phase as f32)
                    + blocker.next_sample() * 100.0
            })
            .collect();
        let mut selected = Vec::new();
        let mut block = Vec::new();
        let mut output = Vec::new();
        let mut bank = plan.filter_bank(2048);
        for combined in [false, true] {
            output.clear();
            coarse.reset();
            ddc.reset();
            bank.reset();
            for chunk in input.chunks(2048) {
                let selected = if combined {
                    bank.process(chunk);
                    bank.samples(band)
                } else {
                    coarse.process(chunk, &mut selected);
                    &selected
                };
                ddc.process(selected, &mut block);
                output.extend_from_slice(&block);
            }
            let mut audio = Vec::new();
            FmDemod::new(48_000.0, 2500.0).process(&output, &mut audio);
            let settled = &audio[480..];
            let tone: Complex<f64> = settled
                .iter()
                .enumerate()
                .map(|(index, &sample)| {
                    Complex::from_polar(f64::from(sample), -TAU * 1000.0 * index as f64 / 48_000.0)
                })
                .sum();
            let tone_power = 2.0 * tone.norm_sqr() / (settled.len() as f64).powi(2);
            let total = settled
                .iter()
                .map(|&sample| f64::from(sample).powi(2))
                .sum::<f64>()
                / settled.len() as f64;
            assert!(
                (0.45..0.55).contains(&tone_power),
                "tone power={tone_power}"
            );
            assert!(
                total - tone_power < 0.001,
                "distortion power={}",
                total - tone_power
            );
        }
    }
}
