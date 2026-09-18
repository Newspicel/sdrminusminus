use num_complex::Complex;
use sdrmm_dsp::subband::{SUBBANDS, SubbandDecimator, SubbandPlan};

use super::{ChannelHost, DSP_BLOCK};

const MIN_SHARED_CHANNELS: usize = 2;

struct Band {
    decimator: SubbandDecimator,
    output: Vec<Complex<f32>>,
    next: Option<u64>,
}

pub(crate) struct Subbands {
    input_rate: f64,
    bands: Vec<Band>,
    counts: [usize; SUBBANDS],
    center: Option<f64>,
}

impl Subbands {
    pub(super) fn new(input_rate: f64) -> Self {
        let bands = SubbandPlan::new(input_rate)
            .map(|plan| {
                (0..SUBBANDS)
                    .map(|band| Band {
                        decimator: plan.decimator(band, DSP_BLOCK),
                        output: Vec::with_capacity(DSP_BLOCK),
                        next: None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            input_rate,
            bands,
            counts: [0; SUBBANDS],
            center: None,
        }
    }

    pub(super) fn prepare(
        &mut self,
        channels: &mut [(u32, Box<ChannelHost>)],
        center: f64,
        rate: f64,
    ) {
        self.counts.fill(0);
        if self.center != Some(center) {
            for band in &mut self.bands {
                band.next = None;
            }
            self.center = Some(center);
        }
        if self.bands.is_empty() || rate != self.input_rate {
            return;
        }
        for (_, host) in channels {
            if let Some(band) = host.subband(center, rate) {
                self.counts[band] += 1;
            }
        }
    }

    pub(super) fn process(&mut self, input: &[Complex<f32>], index: u64) {
        for (band, count) in self.bands.iter_mut().zip(self.counts) {
            if count < MIN_SHARED_CHANNELS {
                band.output.clear();
                continue;
            }
            if band.next != Some(index) {
                band.decimator.reset();
            }
            band.next = Some(index.saturating_add(input.len() as u64));
            band.decimator.process(input, &mut band.output);
        }
    }

    pub(super) fn samples(&self, band: usize) -> Option<&[Complex<f32>]> {
        self.bands
            .get(band)
            .filter(|_| self.counts[band] >= MIN_SHARED_CHANNELS)
            .map(|band| band.output.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_test_support::assert_no_alloc;

    use super::*;

    #[test]
    fn sparse_bands_stay_direct_and_gaps_restarts_and_retunes_reset_history() {
        let input = vec![Complex::new(0.5, 0.25); DSP_BLOCK];
        let mut bank = Subbands::new(20_000_000.0);
        bank.counts[7] = 1;
        bank.process(&input, 0);
        assert!(bank.samples(7).is_none());
        bank.counts[7] = 2;
        bank.process(&input, DSP_BLOCK as u64);
        let fresh = bank.samples(7).unwrap().to_vec();
        bank.process(&input, 2 * DSP_BLOCK as u64);
        assert_ne!(bank.samples(7).unwrap(), fresh);
        assert_no_alloc("subband recovery", || {
            bank.process(&input, 10 * DSP_BLOCK as u64);
            assert_eq!(bank.samples(7).unwrap(), fresh);
            bank.counts[7] = 0;
            bank.process(&input, 11 * DSP_BLOCK as u64);
            bank.counts[7] = 2;
            bank.process(&input, 12 * DSP_BLOCK as u64);
            assert_eq!(bank.samples(7).unwrap(), fresh);
            bank.prepare(&mut [], 100_001_000.0, 20_000_000.0);
            bank.counts[7] = 2;
            bank.process(&input, 13 * DSP_BLOCK as u64);
            assert_eq!(bank.samples(7).unwrap(), fresh);
        });
        bank.prepare(&mut [], 100_001_000.0, 8_000_000.0);
        bank.process(&input, 14 * DSP_BLOCK as u64);
        assert!(bank.samples(7).is_none());
    }
}
