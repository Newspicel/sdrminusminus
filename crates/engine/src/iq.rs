use std::sync::Arc;

use num_complex::Complex;

pub const IQ_BLOCKS_PER_SEC: f64 = 20.0;
pub const IQ_BLOCK_SAMPLES: usize = 2048;
pub(crate) const IQ_CHANNEL_CAP: usize = 4;

#[derive(Clone, Debug)]
pub struct IqBlock {
    pub seq: u32,
    pub timestamp: u64,
    pub sample_rate: f32,
    pub center_hz: f64,
    pub samples: Arc<[Complex<f32>]>,
}

pub(crate) struct IqTap {
    block: Vec<Complex<f32>>,
    skip: usize,
    stride: usize,
    seq: u32,
    position: u64,
}

impl IqTap {
    pub(crate) fn push_at(
        &mut self,
        input: &[Complex<f32>],
        position: u64,
        sample_rate: f32,
        center_hz: f64,
        emit: impl FnMut(IqBlock),
    ) {
        if self.position != position {
            self.reset();
            self.position = position;
        }
        self.push(input, sample_rate, center_hz, emit);
    }

    pub(crate) fn new(sample_rate: f64) -> Self {
        Self {
            block: Vec::with_capacity(IQ_BLOCK_SAMPLES),
            skip: 0,
            stride: stride_for(sample_rate),
            seq: 0,
            position: 0,
        }
    }

    pub(crate) fn push(
        &mut self,
        input: &[Complex<f32>],
        sample_rate: f32,
        center_hz: f64,
        mut emit: impl FnMut(IqBlock),
    ) {
        let start = self.position;
        for (offset, &sample) in input.iter().enumerate() {
            if self.skip > 0 {
                self.skip -= 1;
                continue;
            }
            self.block.push(sample);
            if self.block.len() < IQ_BLOCK_SAMPLES {
                continue;
            }
            emit(IqBlock {
                seq: self.seq,
                timestamp: start + offset as u64 + 1 - IQ_BLOCK_SAMPLES as u64,
                sample_rate,
                center_hz,
                samples: Arc::from(self.block.as_slice()),
            });
            self.seq = self.seq.wrapping_add(1);
            self.block.clear();
            self.skip = self.stride.saturating_sub(IQ_BLOCK_SAMPLES);
        }
        self.position += input.len() as u64;
    }

    pub(crate) fn reset(&mut self) {
        self.block.clear();
        self.skip = 0;
    }
}

fn stride_for(sample_rate: f64) -> usize {
    let requested = (sample_rate / IQ_BLOCKS_PER_SEC) as usize;
    requested.max(IQ_BLOCK_SAMPLES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(n: usize, from: u64) -> Vec<Complex<f32>> {
        (0..n)
            .map(|i| Complex::new((from as usize + i) as f32, 0.0))
            .collect()
    }

    #[test]
    fn a_burst_carries_consecutive_samples() {
        let mut tap = IqTap::new(4096.0 * IQ_BLOCKS_PER_SEC);
        let mut bursts = Vec::new();
        tap.push(&ramp(4096, 0), 4096.0, 100e6, |block| bursts.push(block));

        assert_eq!(bursts.len(), 1);
        let burst = &bursts[0];
        assert_eq!(burst.samples.len(), IQ_BLOCK_SAMPLES);
        assert_eq!(burst.timestamp, 0);
        for (i, sample) in burst.samples.iter().enumerate() {
            assert_eq!(
                sample.re, i as f32,
                "sample {i} is not the one that followed"
            );
        }
    }

    #[test]
    fn the_gap_between_bursts_is_the_requested_cadence() {
        let mut tap = IqTap::new(4096.0 * IQ_BLOCKS_PER_SEC);
        let mut stamps = Vec::new();
        for round in 0..3u64 {
            let from = round * 4096;
            tap.push(&ramp(4096, from), 4096.0, 100e6, |block| {
                stamps.push((block.seq, block.timestamp, block.samples[0].re));
            });
        }
        assert_eq!(
            stamps,
            vec![(0, 0, 0.0), (1, 4096, 4096.0), (2, 8192, 8192.0)]
        );
    }

    #[test]
    fn a_slow_channel_sends_back_to_back_bursts_and_never_overlaps() {
        let mut tap = IqTap::new(1000.0);
        let mut stamps = Vec::new();
        tap.push(&ramp(IQ_BLOCK_SAMPLES * 2, 0), 1000.0, 100e6, |block| {
            stamps.push(block.timestamp);
        });
        assert_eq!(stamps, vec![0, IQ_BLOCK_SAMPLES as u64]);
    }

    #[test]
    fn a_burst_spans_the_block_boundaries_it_was_fed_across() {
        let mut tap = IqTap::new(f64::from(IQ_BLOCK_SAMPLES as u32) * IQ_BLOCKS_PER_SEC);
        let mut bursts = Vec::new();
        for round in 0..8u64 {
            let from = round * 256;
            tap.push(&ramp(256, from), 1000.0, 100e6, |block| bursts.push(block));
        }
        assert_eq!(bursts.len(), 1);
        assert_eq!(bursts[0].samples.len(), IQ_BLOCK_SAMPLES);
        assert_eq!(bursts[0].samples[2047].re, 2047.0);
    }

    #[test]
    fn a_reset_discards_the_partial_burst_rather_than_splicing_over_a_gap() {
        let mut tap = IqTap::new(1000.0);
        let mut bursts = Vec::new();
        tap.push(&ramp(1024, 0), 1000.0, 100e6, |block| bursts.push(block));
        assert!(bursts.is_empty(), "half a burst is not a burst");

        tap.reset();
        tap.push(&ramp(IQ_BLOCK_SAMPLES, 5000), 1000.0, 100e6, |block| {
            bursts.push(block);
        });
        assert_eq!(bursts.len(), 1);
        assert_eq!(
            bursts[0].samples[0].re, 5000.0,
            "the burst must start after the gap, not before it"
        );
    }

    #[test]
    fn the_stride_follows_the_channel_rate() {
        assert_eq!(IqTap::new(1000.0).stride, IQ_BLOCK_SAMPLES);
        assert_eq!(IqTap::new(4096.0 * IQ_BLOCKS_PER_SEC).stride, 4096);
    }
}
