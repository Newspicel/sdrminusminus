use num_complex::Complex;
use sdrmm_dsp::{Ddc, DdcError, subband::SubbandPlan};

use super::dsp_block_len;

struct Shared {
    plan: SubbandPlan,
    band: Option<usize>,
    ddc: Ddc,
    output_rate: f64,
}

pub(super) struct Downconverter {
    direct: Ddc,
    shared: Option<Shared>,
    sharing: bool,
}

impl Downconverter {
    pub(super) fn new(input_rate: f64, output_rate: f64, offset: f64) -> Result<Self, DdcError> {
        let mut direct = Ddc::new(input_rate, output_rate, offset)?;
        let shared =
            match SubbandPlan::new(input_rate).filter(|plan| output_rate <= plan.bandwidth()) {
                Some(plan) => {
                    let band = plan.select(offset, output_rate);
                    let center = band.map(|band| plan.center(band)).unwrap_or(0.0);
                    let mut ddc = Ddc::new(plan.output_rate(), output_rate, offset - center)?;
                    let block_len = dsp_block_len(input_rate);
                    let input = vec![Complex::new(0.0, 0.0); block_len];
                    let mut output = Vec::new();
                    let shared_len =
                        (block_len as f64 * plan.output_rate() / input_rate).ceil() as usize;
                    for _ in 0..8 {
                        direct.process(&input, &mut output);
                        ddc.process(&input[..shared_len], &mut output);
                    }
                    direct.reset();
                    ddc.reset();
                    Some(Shared {
                        plan,
                        band,
                        ddc,
                        output_rate,
                    })
                }
                None => None,
            };
        Ok(Self {
            direct,
            shared,
            sharing: false,
        })
    }

    pub(super) fn band(&self) -> Option<usize> {
        self.shared.as_ref().and_then(|shared| shared.band)
    }

    pub(super) fn reset(&mut self) {
        self.direct.reset();
        if let Some(shared) = &mut self.shared {
            shared.ddc.reset();
        }
    }

    pub(super) fn set_offset(&mut self, offset: f64) {
        self.direct.set_offset(offset);
        if let Some(shared) = &mut self.shared {
            shared.band = shared.plan.select(offset, shared.output_rate);
            let center = shared
                .band
                .map(|band| shared.plan.center(band))
                .unwrap_or(0.0);
            shared.ddc.set_offset(offset - center);
        }
    }

    pub(super) fn select_shared(&mut self, available: bool) -> bool {
        let sharing = available && self.band().is_some();
        if sharing == self.sharing {
            return false;
        }
        self.sharing = sharing;
        self.reset();
        true
    }

    pub(super) fn process(
        &mut self,
        input: &[Complex<f32>],
        selected: Option<&[Complex<f32>]>,
        output: &mut Vec<Complex<f32>>,
    ) {
        if let Some(samples) = selected
            && let Some(shared) = &mut self.shared
            && self.sharing
        {
            shared.ddc.process(samples, output);
        } else {
            self.direct.process(input, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::Nco;
    use sdrmm_test_support::assert_no_alloc;

    use super::{
        super::{DSP_BLOCK, MAX_DSP_BLOCK},
        *,
    };

    #[test]
    fn shared_conversion_preserves_frequency_level_and_sample_count() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        for output_rate in [48_000.0, 240_000.0] {
            for offset in [-9_400_000.0, -731_250.0, 187_500.0, 8_100_000.0] {
                let mut ddc = Downconverter::new(20_000_000.0, output_rate, offset).unwrap();
                let band = ddc.band().unwrap();
                let mut coarse = plan.decimator(band, DSP_BLOCK);
                let mut carrier = Nco::new((offset + 1000.0) as f32, 20_000_000.0);
                let input: Vec<_> = (0..262_149).map(|_| carrier.next_sample()).collect();
                let mut selected = Vec::new();
                let mut block = Vec::new();
                let mut output = Vec::new();
                ddc.select_shared(true);
                for chunk in input.chunks(2047) {
                    coarse.process(chunk, &mut selected);
                    ddc.process(chunk, Some(&selected), &mut block);
                    output.extend_from_slice(&block);
                }
                let expected = input.len() as f64 * output_rate / 20_000_000.0;
                assert!((output.len() as f64 - expected).abs() < 2.0);
                let settled = &output[256..];
                let power = settled.iter().map(|sample| sample.norm_sqr()).sum::<f32>()
                    / settled.len() as f32;
                assert!(
                    (0.98..1.02).contains(&power),
                    "offset={offset} rate={output_rate} power={power}"
                );
                let rotation: Complex<f32> = settled
                    .windows(2)
                    .map(|pair| pair[1] * pair[0].conj())
                    .sum();
                let frequency = f64::from(rotation.arg()) * output_rate / std::f64::consts::TAU;
                assert!(
                    (frequency - 1000.0).abs() < 2.0,
                    "offset={offset} rate={output_rate} frequency={frequency}"
                );
            }
        }
    }

    #[test]
    fn source_switches_and_retunes_reuse_prepared_storage() {
        let plan = SubbandPlan::new(20_000_000.0).unwrap();
        let mut ddc = Downconverter::new(20_000_000.0, 240_000.0, 100_000.0).unwrap();
        let mut coarse = plan.decimator(ddc.band().unwrap(), MAX_DSP_BLOCK);
        let input = vec![Complex::new(0.5, -0.25); MAX_DSP_BLOCK];
        let mut selected = Vec::with_capacity(MAX_DSP_BLOCK);
        let mut output = Vec::with_capacity(MAX_DSP_BLOCK);
        assert_no_alloc("shared conversion and route switches", || {
            for size in [1, 17, MAX_DSP_BLOCK, 3, 409, 2047] {
                for shared in [true, false, true] {
                    coarse.process(&input[..size], &mut selected);
                    ddc.select_shared(shared);
                    ddc.process(
                        &input[..size],
                        shared.then_some(selected.as_slice()),
                        &mut output,
                    );
                    ddc.set_offset(101_000.0);
                    ddc.reset();
                    coarse.reset();
                }
            }
        });
    }
}
