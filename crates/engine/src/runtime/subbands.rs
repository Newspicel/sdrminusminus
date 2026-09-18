use num_complex::Complex;
use sdrmm_dsp::subband::{SUBBANDS, SubbandDecimator, SubbandFilterBank, SubbandPlan};

use super::{ChannelHost, DSP_BLOCK};

const MIN_SHARED_CHANNELS: usize = 2;
const MIN_BANK_BANDS: usize = 12;

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
    bank: Option<SubbandFilterBank>,
    bank_next: Option<u64>,
    use_bank: bool,
    history: Vec<Complex<f32>>,
    next_input: Option<u64>,
    alignment: usize,
}

impl Subbands {
    pub(super) fn new(input_rate: f64) -> Self {
        let plan = SubbandPlan::new(input_rate);
        let bands = plan
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
            bank: plan.map(|plan| plan.filter_bank(DSP_BLOCK)),
            bank_next: None,
            use_bank: false,
            history: vec![Complex::new(0.0, 0.0); plan.map_or(0, SubbandPlan::history_len)],
            next_input: None,
            alignment: plan.map_or(1, SubbandPlan::alignment),
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
            self.next_input = None;
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
        if self.next_input != Some(index) {
            self.history.fill(Complex::new(0.0, 0.0));
            self.bank_next = None;
            for band in &mut self.bands {
                band.next = None;
            }
        }
        self.next_input = Some(index.saturating_add(input.len() as u64));
        let skip = (self.history.len() + self.alignment - (index % self.alignment as u64) as usize)
            % self.alignment;
        let warm = &self.history[skip..];
        self.use_bank = self.counts.iter().filter(|&&count| count > 0).count() >= MIN_BANK_BANDS;
        if self.use_bank {
            if let Some(bank) = &mut self.bank {
                if self.bank_next != Some(index) {
                    bank.reset();
                    bank.process(warm);
                }
                bank.process(input);
                self.bank_next = self.next_input;
            }
        } else {
            for (band, count) in self.bands.iter_mut().zip(self.counts) {
                if count < MIN_SHARED_CHANNELS {
                    band.output.clear();
                    continue;
                }
                if band.next != Some(index) {
                    band.decimator.reset();
                    band.decimator.process(warm, &mut band.output);
                }
                band.next = Some(index.saturating_add(input.len() as u64));
                band.decimator.process(input, &mut band.output);
            }
        }
        self.remember(input);
    }

    fn remember(&mut self, input: &[Complex<f32>]) {
        let count = input.len().min(self.history.len());
        self.history.copy_within(count.., 0);
        let start = self.history.len() - count;
        self.history[start..].copy_from_slice(&input[input.len() - count..]);
    }

    pub(super) fn samples(&self, band: usize) -> Option<&[Complex<f32>]> {
        if self.use_bank {
            return self
                .bank
                .as_ref()
                .filter(|_| self.counts[band] > 0)
                .map(|bank| bank.samples(band));
        }
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
    fn sparse_bands_stay_direct_and_gaps_and_retunes_reset_history() {
        let input = vec![Complex::new(0.5, 0.25); DSP_BLOCK];
        let mut bank = Subbands::new(20_000_000.0);
        bank.counts[7] = 1;
        bank.process(&input, 0);
        assert!(bank.samples(7).is_none());
        bank.counts[7] = 2;
        bank.process(&input, DSP_BLOCK as u64);
        let mut fresh = Subbands::new(20_000_000.0);
        fresh.counts[7] = 2;
        fresh.process(&input, 10 * DSP_BLOCK as u64);
        assert_no_alloc("subband recovery", || {
            bank.process(&input, 10 * DSP_BLOCK as u64);
            assert_eq!(bank.samples(7), fresh.samples(7));
            bank.prepare(&mut [], 100_001_000.0, 20_000_000.0);
            bank.counts[7] = 2;
            bank.process(&input, 10 * DSP_BLOCK as u64);
            assert_eq!(bank.samples(7), fresh.samples(7));
        });
        bank.prepare(&mut [], 100_001_000.0, 8_000_000.0);
        bank.process(&input, 14 * DSP_BLOCK as u64);
        assert!(bank.samples(7).is_none());
    }

    #[test]
    fn switching_filter_paths_preserves_history_phase_and_sample_count_without_allocating() {
        let rate = 20_000_000.0;
        let plan = SubbandPlan::new(rate).unwrap();
        let input: Vec<_> = (0..DSP_BLOCK)
            .map(|index| {
                Complex::new(
                    (index % 17) as f32 / 17.0 - 0.5,
                    (index % 13) as f32 / 13.0 - 0.5,
                )
            })
            .collect();
        let mut bank = Subbands::new(rate);
        let mut references: Vec<_> = (0..SUBBANDS)
            .map(|band| plan.decimator(band, DSP_BLOCK))
            .collect();
        let mut expected = Vec::with_capacity(DSP_BLOCK);
        let mut index = 0;
        for dense in [false, true, false, true, false] {
            bank.counts.fill(usize::from(dense));
            bank.counts[7] = 2;
            for size in [DSP_BLOCK, 17, 0, 3, 255].into_iter().cycle().take(20) {
                assert_no_alloc("subband filter path switch", || {
                    bank.process(&input[..size], index)
                });
                assert_eq!(bank.use_bank, dense);
                for (band, reference) in references.iter_mut().enumerate() {
                    reference.process(&input[..size], &mut expected);
                    if let Some(actual) = bank.samples(band) {
                        assert_eq!(actual.len(), expected.len());
                        for (actual, expected) in actual.iter().zip(&expected) {
                            assert!(
                                (*actual - *expected).norm() < 2e-6,
                                "band={band} index={index} dense={dense}"
                            );
                        }
                    } else {
                        assert!(!dense && band != 7);
                    }
                }
                index += size as u64;
            }
        }
    }

    #[test]
    fn wide_banks_discard_history_after_capture_gaps_and_radio_retunes() {
        let input = vec![Complex::new(0.5, -0.25); DSP_BLOCK];
        for rate in [8_000_000.0, 20_000_000.0] {
            let mut used = Subbands::new(rate);
            used.counts.fill(1);
            used.process(&input, 0);
            used.process(&input, DSP_BLOCK as u64);
            let mut fresh = Subbands::new(rate);
            fresh.counts.fill(1);
            fresh.process(&input, 100_003);
            assert_no_alloc("wide bank gap recovery", || used.process(&input, 100_003));
            for band in 0..SUBBANDS {
                assert_eq!(used.samples(band), fresh.samples(band));
            }
            used.prepare(&mut [], 101_000_000.0, rate);
            used.counts.fill(1);
            let mut fresh = Subbands::new(rate);
            fresh.counts.fill(1);
            fresh.process(&input, 100_003 + DSP_BLOCK as u64);
            assert_no_alloc("wide bank radio retune", || {
                used.process(&input, 100_003 + DSP_BLOCK as u64)
            });
            for band in 0..SUBBANDS {
                assert_eq!(used.samples(band), fresh.samples(band));
            }
        }
    }
}
