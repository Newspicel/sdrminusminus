//! The channel baseband tap.
//!
//! A channel's own passband — after the down-conversion and the channel filter, before the
//! demodulator — is what a constellation, an eye diagram and a baseband waterfall are all drawn
//! from. It is also the one stream in the engine whose full rate cannot simply be forwarded: a
//! native-rate channel runs at the device rate, and a continuous tap of one would be tens of
//! megabytes a second per watcher.
//!
//! So the tap sends *bursts*: `IQ_BLOCK_SAMPLES` consecutive samples, `IQ_BLOCKS_PER_SEC` times a
//! second, and nothing in between. Consecutive is the part that matters — symbol timing is only
//! legible in samples that were adjacent on the air — and no display needs every burst.

use std::sync::Arc;

use num_complex::Complex;

/// Bursts a subscribed channel sends per second.
pub const IQ_BLOCKS_PER_SEC: f64 = 20.0;
/// Consecutive complex samples in one burst. At 20 bursts a second this is ~330 kB/s on the wire,
/// whatever rate the channel itself runs at.
pub const IQ_BLOCK_SAMPLES: usize = 2048;
/// Bursts held for a subscriber that has stopped reading. Small on purpose: a display that has
/// fallen behind wants the newest burst, never a queue of stale ones.
pub(crate) const IQ_CHANNEL_CAP: usize = 4;

/// One burst of a channel's baseband on its way to the clients watching it.
#[derive(Clone, Debug)]
pub struct IqBlock {
    /// Bursts sent by this pipeline. Restarts when the pipeline is rebuilt, like a video `seq`.
    pub seq: u32,
    /// Channel-rate sample count at the first sample of the burst, so the gap between two bursts
    /// is legible as the time it really was.
    pub timestamp: u64,
    /// The channel's input rate: the bandwidth this baseband spans.
    pub sample_rate: f32,
    /// Absolute frequency the baseband is centred on.
    pub center_hz: f64,
    /// Shared rather than copied: one burst reaches every subscribed client.
    pub samples: Arc<[Complex<f32>]>,
}

/// Collects consecutive samples into bursts and paces them.
///
/// Lives on the DSP thread. `push` allocates exactly once per completed burst — the documented,
/// bounded deviation the PCM and picture hand-offs already make — and does nothing at all while
/// nobody is subscribed.
pub(crate) struct IqTap {
    block: Vec<Complex<f32>>,
    /// Samples still to be skipped before the next burst starts filling.
    skip: usize,
    /// Samples between the start of one burst and the start of the next.
    stride: usize,
    seq: u32,
    /// Channel-rate samples seen, which stamps each burst.
    position: u64,
}

impl IqTap {
    pub(crate) fn new(sample_rate: f64) -> Self {
        Self {
            block: Vec::with_capacity(IQ_BLOCK_SAMPLES),
            skip: 0,
            stride: stride_for(sample_rate),
            seq: 0,
            position: 0,
        }
    }

    /// Feed one processed block. Returns each completed burst, in order.
    ///
    /// `input` is the channel's filtered baseband; `center_hz` its absolute centre. The whole
    /// input is consumed, so a block longer than one stride can complete more than one burst.
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
                // The burst began `IQ_BLOCK_SAMPLES - 1` samples before this one.
                timestamp: start + offset as u64 + 1 - IQ_BLOCK_SAMPLES as u64,
                sample_rate,
                center_hz,
                samples: Arc::from(self.block.as_slice()),
            });
            self.seq = self.seq.wrapping_add(1);
            self.block.clear();
            // A stride shorter than a burst would mean sending faster than requested rather than
            // overlapping bursts, so the gap is never negative.
            self.skip = self.stride.saturating_sub(IQ_BLOCK_SAMPLES);
        }
        self.position += input.len() as u64;
    }

    /// Drop a partially filled burst. Used when the tap goes idle, so the samples either side of
    /// the gap are never spliced into one burst that claims they were adjacent.
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
        // One burst per 4096 samples: 2048 taken, 2048 skipped.
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

    /// A channel slower than one burst per period cannot be paced down any further: the tap sends
    /// back-to-back bursts rather than overlapping ones.
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

    /// The tap's whole promise is that a burst's samples were adjacent on the air. A gap in the
    /// feed must therefore throw away the half-filled burst rather than splice across it.
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

    /// The cadence is fixed for a host's life: a rate change rebuilds the whole pipeline, and the
    /// tap with it.
    #[test]
    fn the_stride_follows_the_channel_rate() {
        assert_eq!(IqTap::new(1000.0).stride, IQ_BLOCK_SAMPLES);
        assert_eq!(IqTap::new(4096.0 * IQ_BLOCKS_PER_SEC).stride, 4096);
    }
}
