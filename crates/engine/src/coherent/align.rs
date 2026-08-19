use num_complex::Complex;

use super::tap::CoherentTaps;

/// The most samples one aggregator pass hands to a processor. Big enough that the per-pass
/// bookkeeping disappears, small enough that a processor still sees the stream as it arrives.
pub(crate) const ALIGN_BLOCK: usize = 16_384;

const MAX_LANES: usize = sdrmm_wire::MAX_STREAMS as usize;

/// Turns per-lane rings that each drop their own samples into one stream of ranges every lane
/// agrees on.
///
/// Lanes only ever desync by whole recorded gaps, so catching up is exact: the lane furthest
/// ahead names the index everyone else discards up to.
pub(crate) struct Aligner {
    taps: CoherentTaps,
    lanes: Vec<Vec<Complex<f32>>>,
    realignments: u64,
    index: u64,
}

impl Aligner {
    pub(crate) fn new(taps: CoherentTaps) -> Self {
        let lanes = (0..taps.feeds.len())
            .map(|_| Vec::with_capacity(ALIGN_BLOCK))
            .collect();
        Self {
            taps,
            lanes,
            realignments: 0,
            index: 0,
        }
    }

    pub(crate) const fn sample_rate(&self) -> f64 {
        self.taps.sample_rate
    }

    pub(crate) const fn realignments(&self) -> u64 {
        self.realignments
    }

    /// The stream index of the first sample of the range `lanes` currently holds.
    pub(crate) const fn index(&self) -> u64 {
        self.index
    }

    #[cfg(test)]
    pub(crate) fn lanes(&self) -> &[Vec<Complex<f32>>] {
        &self.lanes
    }

    /// Hands every lane's slice of the current range to `f` without building a list on the heap,
    /// which is what keeps the aggregator loop allocation-free.
    pub(crate) fn with_lanes<R>(&self, count: usize, f: impl FnOnce(&[&[Complex<f32>]]) -> R) -> R {
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        let lanes = self.lanes.len().min(MAX_LANES);
        for (slot, lane) in view.iter_mut().zip(&self.lanes) {
            *slot = &lane[..count];
        }
        f(&view[..lanes])
    }

    /// Hands the beam ring to whoever is going to write into it, so the aligner keeps only what
    /// it reads from.
    pub(crate) fn take_beam(&mut self) -> Option<super::tap::BeamSink> {
        self.taps.beam.take()
    }

    pub(crate) fn release(mut self) -> CoherentTaps {
        self.taps.rewind();
        self.taps
    }

    /// Fills every lane with the next range they all cover, or reports that one of them has not
    /// caught up yet.
    pub(crate) fn next(&mut self) -> Option<usize> {
        for feed in &mut self.taps.feeds {
            feed.settle();
        }
        let common = self
            .taps
            .feeds
            .iter()
            .map(|feed| feed.read_index())
            .max()
            .unwrap_or(0);
        let mut aligned = true;
        for feed in &mut self.taps.feeds {
            let behind = common - feed.read_index();
            if behind == 0 {
                continue;
            }
            self.realignments += 1;
            let ready = feed.ready() as u64;
            feed.skip(behind.min(ready) as usize);
            feed.settle();
            aligned &= feed.read_index() == common;
        }
        if !aligned {
            return None;
        }
        let count = self
            .taps
            .feeds
            .iter()
            .map(super::tap::LaneFeed::ready)
            .min()
            .unwrap_or(0)
            .min(ALIGN_BLOCK);
        if count == 0 {
            return None;
        }
        self.index = common;
        for (feed, lane) in self.taps.feeds.iter_mut().zip(&mut self.lanes) {
            feed.take(count, lane);
        }
        Some(count)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{super::tap::lane_taps, *};

    fn block(len: usize, value: f32) -> Vec<Complex<f32>> {
        vec![Complex::new(value, 0.0); len]
    }

    #[test]
    fn lanes_that_agree_hand_over_the_whole_range() {
        let (mut taps, shared) = lane_taps(2, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        for (lane, tap) in taps.iter_mut().enumerate() {
            tap.push(&block(256, lane as f32), 0);
        }
        let mut aligner = Aligner::new(shared);
        assert_eq!(aligner.next(), Some(256));
        assert_eq!(aligner.index(), 0);
        assert_eq!(aligner.lanes()[1][0].re, 1.0);
        assert_eq!(aligner.realignments(), 0);
    }

    #[test]
    fn one_lane_losing_a_block_realigns_both_on_the_next_common_index() {
        let (mut taps, shared) = lane_taps(2, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(128, 1.0), 0);
        taps[0].push(&block(128, 2.0), 128);
        taps[1].push(&block(128, 3.0), 0);
        taps[1].push(&block(128, 4.0), 256);

        let mut aligner = Aligner::new(shared);
        assert_eq!(aligner.next(), Some(128));
        assert_eq!(aligner.index(), 0);

        taps[0].push(&block(128, 5.0), 256);
        assert_eq!(aligner.next(), Some(128));
        assert_eq!(aligner.index(), 256);
        assert_eq!(aligner.lanes()[0][0].re, 5.0);
        assert_eq!(aligner.lanes()[1][0].re, 4.0);
        assert!(aligner.realignments() > 0);
    }

    #[test]
    fn a_lane_with_nothing_yet_holds_the_others_back() {
        let (mut taps, shared) = lane_taps(2, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(64, 1.0), 0);
        let mut aligner = Aligner::new(shared);
        assert_eq!(aligner.next(), None);
        taps[1].push(&block(64, 2.0), 0);
        assert_eq!(aligner.next(), Some(64));
    }
}
