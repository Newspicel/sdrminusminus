use num_complex::Complex;

use super::{
    FACTOR, HALF_BANDS, MIXER_PERIOD, MIXER_STEP, SUBBANDS, prototype, transform::Inverse25,
};
use crate::fir::Accumulate;

pub struct SubbandFilterBank {
    taps: Vec<f32>,
    input: Vec<Complex<f32>>,
    fft: Inverse25,
    output: [Vec<Complex<f32>>; SUBBANDS],
    phase: usize,
}

impl SubbandFilterBank {
    pub(super) fn new(block_len: usize) -> Self {
        let taps = prototype();
        let mut input = Vec::with_capacity(taps.len() - 1 + block_len);
        input.resize(taps.len() - 1, Complex::new(0.0, 0.0));
        Self {
            taps,
            input,
            fft: Inverse25::new(),
            output: std::array::from_fn(|_| Vec::with_capacity(block_len.div_ceil(FACTOR))),
            phase: 0,
        }
    }

    pub fn reset(&mut self) {
        self.input.clear();
        self.input
            .resize(self.taps.len() - 1, Complex::new(0.0, 0.0));
        for output in &mut self.output {
            output.clear();
        }
        self.phase = 0;
    }

    pub fn process(&mut self, input: &[Complex<f32>]) {
        self.input.extend_from_slice(input);
        let samples = self.input.len().saturating_sub(self.taps.len() - 1);
        let frames = samples.div_ceil(FACTOR);
        for output in &mut self.output {
            output.clear();
        }
        for frame in 0..frames {
            let mut transform = [Complex::new(0.0, 0.0); MIXER_PERIOD];
            let window = &self.input[frame * FACTOR..frame * FACTOR + self.taps.len()];
            for (samples, taps) in window
                .rchunks(MIXER_PERIOD)
                .zip(self.taps.chunks(MIXER_PERIOD))
            {
                for ((sum, &sample), &tap) in
                    transform.iter_mut().zip(samples.iter().rev()).zip(taps)
                {
                    *sum = sum.add_product(sample, tap);
                }
            }
            transform.rotate_left((self.phase + frame) % (MIXER_PERIOD / FACTOR) * FACTOR);
            self.fft.process(&mut transform);
            for (band, output) in self.output.iter_mut().enumerate() {
                let bin = ((band + MIXER_PERIOD - HALF_BANDS) * MIXER_STEP) % MIXER_PERIOD;
                output.push(transform[bin]);
            }
        }
        self.phase = (self.phase + frames) % (MIXER_PERIOD / FACTOR);
        self.input.drain(..frames * FACTOR);
    }

    #[must_use]
    pub fn samples(&self, band: usize) -> &[Complex<f32>] {
        &self.output[band]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subband::SubbandPlan;

    #[test]
    fn all_bands_match_independent_filters_across_ragged_blocks_and_reset() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        let mut state = 0x123456789abcdefu64;
        let input: Vec<_> = (0..65_537)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                Complex::new(
                    (state as i32) as f32 / i32::MAX as f32,
                    ((state >> 32) as i32) as f32 / i32::MAX as f32,
                )
            })
            .collect();
        let mut bank = plan.filter_bank(4099);
        let mut filters: Vec<_> = (0..SUBBANDS)
            .map(|band| plan.decimator(band, 4099))
            .collect();
        let mut expected = Vec::new();
        for _ in 0..2 {
            let mut at = 0;
            for size in [0, 1, 17, 5, 25, 2048, 4099].into_iter().cycle() {
                let end = (at + size).min(input.len());
                bank.process(&input[at..end]);
                for (band, filter) in filters.iter_mut().enumerate() {
                    filter.process(&input[at..end], &mut expected);
                    let actual = bank.samples(band);
                    assert_eq!(actual.len(), expected.len());
                    for (actual, expected) in actual.iter().zip(&expected) {
                        assert!(
                            (*actual - *expected).norm() < 2e-6,
                            "band={band} at={at} actual={actual} expected={expected}"
                        );
                    }
                }
                at = end;
                if at == input.len() {
                    break;
                }
            }
            bank.reset();
            for filter in &mut filters {
                filter.reset();
            }
        }
    }
}
