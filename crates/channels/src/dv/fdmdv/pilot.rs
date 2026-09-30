use std::sync::LazyLock;

use sdrmm_dsp::fft::Transform;

use super::{SYMBOL_SAMPLES, SYMBOL_SPAN, Sample, fir, shaped, tables};

const LUT_SYMBOLS: usize = 4;
const LUT_LEN: usize = LUT_SYMBOLS * SYMBOL_SAMPLES;
const SETTLING_SYMBOLS: usize = 4;
const LAGGED_START: usize = 3 * SYMBOL_SAMPLES;
const LPF_TAPS: usize = 30;
const BASEBAND_LEN: usize = LPF_TAPS + super::MAX_FRAME;
const LPF_LEN: usize = LUT_LEN;
const FFT_LEN: usize = 256;
const DECIMATION: usize = (super::SAMPLE_RATE_HZ / (2.0 * MAX_OFFSET_HZ)) as usize;
const MAX_OFFSET_HZ: f64 = 200.0;
const HZ_PER_BIN: f64 = 2.0 * MAX_OFFSET_HZ / FFT_LEN as f64;

static LUT: LazyLock<[Sample; LUT_LEN]> = LazyLock::new(pilot_lut);

pub(super) struct CoarseFrequency {
    branches: [PilotBranch; 2],
    spectrum: [Sample; FFT_LEN],
    fft: Transform,
}

struct PilotBranch {
    lut_index: usize,
    baseband: [Sample; BASEBAND_LEN],
    lpf: [Sample; LPF_LEN],
}

struct Peak {
    offset_hz: f32,
    power: f32,
}

impl CoarseFrequency {
    pub(super) fn new() -> Self {
        Self {
            branches: [PilotBranch::new(0), PilotBranch::new(LAGGED_START)],
            spectrum: [Sample::ZERO; FFT_LEN],
            fft: Transform::forward(FFT_LEN),
        }
    }

    pub(super) fn estimate(&mut self, samples: &[Sample], search: bool) -> f32 {
        for branch in &mut self.branches {
            branch.update(samples);
        }
        if !search {
            return 0.0;
        }
        let [aligned, lagged] = self
            .branches
            .each_ref()
            .map(|branch| branch.peak(&mut self.spectrum, &mut self.fft));
        if aligned.power > lagged.power {
            aligned.offset_hz
        } else {
            lagged.offset_hz
        }
    }
}

impl PilotBranch {
    fn new(lut_index: usize) -> Self {
        Self {
            lut_index,
            baseband: [Sample::ZERO; BASEBAND_LEN],
            lpf: [Sample::ZERO; LPF_LEN],
        }
    }

    fn update(&mut self, samples: &[Sample]) {
        let length = samples.len();
        self.baseband.copy_within(length.., 0);
        let lut = LUT[self.lut_index..].iter().chain(LUT.iter());
        for ((mixed, &sample), &pilot) in self.baseband[BASEBAND_LEN - length..]
            .iter_mut()
            .zip(samples)
            .zip(lut)
        {
            *mixed = sample * pilot;
        }
        self.lut_index = (self.lut_index + length) % LUT_LEN;

        self.lpf.copy_within(length.., 0);
        fir(
            &tables::PILOT_LOWPASS,
            &self.baseband[BASEBAND_LEN - length - (LPF_TAPS - 1)..],
            &mut self.lpf[LPF_LEN - length..],
        );
    }

    fn peak(&self, spectrum: &mut [Sample; FFT_LEN], fft: &mut Transform) -> Peak {
        spectrum.fill(Sample::ZERO);
        let decimated = self.lpf.iter().step_by(DECIMATION);
        for ((bin, &sample), &weight) in spectrum
            .iter_mut()
            .zip(decimated)
            .zip(&tables::PILOT_WINDOW)
        {
            *bin = sample.scale(weight);
        }
        fft.process(spectrum);
        let (bin, power) = strongest(spectrum);
        Peak {
            offset_hz: bin_offset_hz(bin),
            power,
        }
    }
}

fn strongest(spectrum: &[Sample]) -> (usize, f32) {
    spectrum
        .iter()
        .enumerate()
        .fold((0, 0.0), |(best, max), (bin, value)| {
            let power = value.norm_sqr();
            if power > max {
                (bin, power)
            } else {
                (best, max)
            }
        })
}

fn bin_offset_hz(bin: usize) -> f32 {
    let signed = if bin >= FFT_LEN / 2 {
        bin as f64 - FFT_LEN as f64
    } else {
        bin as f64
    };
    (signed * HZ_PER_BIN) as f32
}

fn pilot_lut() -> [Sample; LUT_LEN] {
    let mut lut = [Sample::ZERO; LUT_LEN];
    let mut history = [0.0f32; SYMBOL_SPAN];
    let mut symbol = std::f32::consts::SQRT_2;
    let mut invert = false;
    for symbol_index in 0..SETTLING_SYMBOLS + LUT_SYMBOLS {
        if invert {
            symbol = -symbol;
        }
        invert = !invert;
        history[SYMBOL_SPAN - 1] = std::f32::consts::SQRT_2 / 2.0 * symbol;
        let shaped: [f32; SYMBOL_SAMPLES] = std::array::from_fn(|n| shaped(&history, n));
        history.rotate_left(1);
        history[SYMBOL_SPAN - 1] = 0.0;
        let Some(slot) = symbol_index.checked_sub(SETTLING_SYMBOLS) else {
            continue;
        };
        for (entry, sample) in lut[slot * SYMBOL_SAMPLES..].iter_mut().zip(shaped) {
            *entry = Sample::ONE
                .scale(std::f32::consts::SQRT_2 * 2.0 * sample)
                .conj();
        }
    }
    lut
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_map_to_signed_offsets() {
        assert_eq!(bin_offset_hz(0), 0.0);
        assert_eq!(bin_offset_hz(1), 1.5625);
        assert_eq!(bin_offset_hz(127), 198.4375);
        assert_eq!(bin_offset_hz(128), -200.0);
        assert_eq!(bin_offset_hz(255), -1.5625);
    }

    #[test]
    fn the_first_strongest_bin_wins() {
        let mut spectrum = [Sample::ZERO; 8];
        spectrum[2] = Sample::new(0.0, 3.0);
        spectrum[5] = Sample::new(3.0, 0.0);
        assert_eq!(strongest(&spectrum), (2, 9.0));
        assert_eq!(strongest(&[Sample::ZERO; 4]), (0, 0.0));
    }

    #[test]
    fn the_pilot_is_a_real_waveform_with_energy_in_every_symbol() {
        let lut = &*LUT;
        assert!(lut.iter().all(|value| value.im == 0.0));
        let energy = |range: std::ops::Range<usize>| {
            lut[range].iter().map(|value| value.norm_sqr()).sum::<f32>()
        };
        for symbol in 0..LUT_SYMBOLS {
            assert!(energy(symbol * SYMBOL_SAMPLES..(symbol + 1) * SYMBOL_SAMPLES) > 1.0);
        }
    }

    #[test]
    fn a_pilot_tone_offset_is_found_in_the_spectrum() {
        let mut coarse = CoarseFrequency::new();
        let step = super::super::rotation(50.0);
        let mut phase = Sample::ONE;
        let mut lut = LUT.iter().cycle();
        let mut estimate = 0.0;
        for _ in 0..10 {
            let block: Vec<Sample> = (0..SYMBOL_SAMPLES)
                .map(|_| {
                    phase *= step;
                    lut.next()
                        .map_or(Sample::ZERO, |pilot| pilot.conj() * phase)
                })
                .collect();
            estimate = coarse.estimate(&block, true);
        }
        assert!((estimate - 50.0).abs() <= HZ_PER_BIN as f32, "{estimate}");
    }
}
