use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};

const TAP_SECONDS: f64 = 0.1;
const TAP_MIN: usize = 1 << 16;
const TAP_MAX: usize = 1 << 20;
const GAP_SLOTS: usize = 64;

#[must_use]
pub(crate) fn tap_capacity(sample_rate: f64) -> usize {
    ((sample_rate * TAP_SECONDS) as usize).clamp(TAP_MIN, TAP_MAX)
}

/// Samples the lane never delivered, named by where they belong in the stream rather than by how
/// many are missing from a count, so a lane that lost a block can be lined up again exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LaneGap {
    pub at: u64,
    pub missing: u64,
}

/// The capture callback's end of one lane's coherent tap.
///
/// Dormant it costs a relaxed load. Armed it copies the block and, whenever the stream index
/// skips — a device drop, a full ring, or the window where nothing was listening — leaves a
/// record of exactly which samples are absent instead of quietly shortening the lane.
pub(crate) struct LaneTap {
    samples: Producer<Complex<f32>>,
    gaps: Producer<LaneGap>,
    armed: Arc<AtomicBool>,
    ring_index: u64,
    pending: Option<LaneGap>,
}

impl LaneTap {
    pub(crate) fn push(&mut self, block: &[Complex<f32>], index: u64) {
        if !self.armed.load(Ordering::Relaxed) {
            return;
        }
        if index != self.ring_index {
            self.hold(LaneGap {
                at: self.ring_index,
                missing: index.saturating_sub(self.ring_index),
            });
            self.ring_index = index;
        }
        if !self.flush() {
            self.hold(LaneGap {
                at: index,
                missing: block.len() as u64,
            });
            self.ring_index = index + block.len() as u64;
            return;
        }
        let take = self.samples.slots().min(block.len());
        if take > 0
            && let Ok(chunk) = self.samples.write_chunk_uninit(take)
        {
            chunk.fill_from_iter(block[..take].iter().copied());
        }
        if take < block.len() {
            self.hold(LaneGap {
                at: index + take as u64,
                missing: (block.len() - take) as u64,
            });
        }
        self.ring_index = index + block.len() as u64;
    }

    /// Merges a gap into whatever is already waiting for room in the gap queue. Two gaps only
    /// ever merge when the second continues the first, which is the only way they can arise.
    fn hold(&mut self, gap: LaneGap) {
        self.pending = Some(match self.pending.take() {
            Some(held) => LaneGap {
                at: held.at,
                missing: held.missing + gap.missing + gap.at.saturating_sub(held.at + held.missing),
            },
            None => gap,
        });
        self.flush();
    }

    /// Whether the gap queue is clear. A gap that cannot be recorded blocks samples behind it,
    /// because samples the consumer cannot place are worse than samples it never sees.
    fn flush(&mut self) -> bool {
        match self.pending.take() {
            None => true,
            Some(gap) => match self.gaps.push(gap) {
                Ok(()) => true,
                Err(_) => {
                    self.pending = Some(gap);
                    false
                }
            },
        }
    }
}

/// The aggregator's end of one lane's tap, tracking where in the stream its next sample sits.
pub(crate) struct LaneFeed {
    samples: Consumer<Complex<f32>>,
    gaps: Consumer<LaneGap>,
    read_index: u64,
    next_gap: Option<LaneGap>,
}

impl LaneFeed {
    /// The stream index of the next sample this lane will hand over.
    pub(crate) const fn read_index(&self) -> u64 {
        self.read_index
    }

    /// Steps over any gap that starts exactly here, so `ready` never reports zero for a lane that
    /// is merely missing samples it will never receive.
    pub(crate) fn settle(&mut self) {
        loop {
            if self.next_gap.is_none() {
                self.next_gap = self.gaps.pop().ok();
            }
            match self.next_gap {
                Some(gap) if gap.at <= self.read_index => {
                    self.read_index = self.read_index.max(gap.at + gap.missing);
                    self.next_gap = None;
                }
                _ => return,
            }
        }
    }

    /// Samples available without crossing a gap.
    pub(crate) fn ready(&self) -> usize {
        let held = self.samples.slots();
        match self.next_gap {
            Some(gap) => held.min(gap.at.saturating_sub(self.read_index) as usize),
            None => held,
        }
    }

    pub(crate) fn skip(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        if let Ok(chunk) = self.samples.read_chunk(count.min(self.samples.slots())) {
            let taken = chunk.len();
            chunk.commit_all();
            self.read_index += taken as u64;
        }
    }

    pub(crate) fn take(&mut self, count: usize, out: &mut Vec<Complex<f32>>) {
        out.clear();
        let Ok(chunk) = self.samples.read_chunk(count) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        chunk.commit_all();
        self.read_index += count as u64;
    }
}

/// Where the summed array lands: an ordinary capture ring with an ordinary dsp thread on it, so
/// every decoder, recorder and spectrum subscription works on the beam without knowing it is one.
pub(crate) struct BeamSink {
    pub(crate) producer: crate::capture_ring::CaptureProducer,
    pub(crate) waker: Arc<crate::runtime::Waker>,
    pub(crate) overruns: Arc<std::sync::atomic::AtomicU64>,
}

impl BeamSink {
    pub(crate) fn push(&mut self, samples: &[Complex<f32>], index: u64) {
        let take = self.producer.push(samples, index);
        if take < samples.len() {
            self.overruns.fetch_add(
                (samples.len() - take) as u64,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        self.waker.wake();
    }
}

/// Every lane's consumer end plus the switch that decides whether the capture callbacks write at
/// all. Held by the capture runtime while nothing coherent is running, and lent to the aggregator
/// for as long as one is.
pub(crate) struct CoherentTaps {
    pub(crate) feeds: Vec<LaneFeed>,
    pub(crate) armed: Arc<AtomicBool>,
    pub(crate) sample_rate: f64,
    pub(crate) beam: Option<BeamSink>,
}

impl CoherentTaps {
    /// Drops everything the rings still hold without losing count of where each lane sits, so an
    /// aggregator that starts late begins on live samples rather than on a backlog.
    pub(crate) fn rewind(&mut self) {
        for feed in &mut self.feeds {
            loop {
                feed.settle();
                let ready = feed.ready();
                if ready == 0 {
                    break;
                }
                feed.skip(ready);
            }
        }
    }
}

pub(crate) fn lane_taps(lanes: usize, sample_rate: f64) -> (Vec<LaneTap>, CoherentTaps) {
    let capacity = tap_capacity(sample_rate);
    let armed = Arc::new(AtomicBool::new(false));
    let mut taps = Vec::with_capacity(lanes);
    let mut feeds = Vec::with_capacity(lanes);
    for _ in 0..lanes {
        let (samples_tx, samples_rx) = RingBuffer::<Complex<f32>>::new(capacity);
        let (gaps_tx, gaps_rx) = RingBuffer::<LaneGap>::new(GAP_SLOTS);
        taps.push(LaneTap {
            samples: samples_tx,
            gaps: gaps_tx,
            armed: armed.clone(),
            ring_index: 0,
            pending: None,
        });
        feeds.push(LaneFeed {
            samples: samples_rx,
            gaps: gaps_rx,
            read_index: 0,
            next_gap: None,
        });
    }
    (
        taps,
        CoherentTaps {
            feeds,
            armed,
            sample_rate,
            beam: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(len: usize, value: f32) -> Vec<Complex<f32>> {
        vec![Complex::new(value, 0.0); len]
    }

    #[test]
    fn a_dormant_tap_writes_nothing() {
        let (mut taps, mut shared) = lane_taps(1, 48_000.0);
        taps[0].push(&block(64, 1.0), 0);
        shared.feeds[0].settle();
        assert_eq!(shared.feeds[0].ready(), 0);
    }

    #[test]
    fn an_armed_tap_hands_over_what_it_was_given() {
        let (mut taps, mut shared) = lane_taps(1, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(64, 1.0), 0);
        taps[0].push(&block(64, 2.0), 64);
        let feed = &mut shared.feeds[0];
        feed.settle();
        assert_eq!(feed.ready(), 128);
        let mut out = Vec::new();
        feed.take(128, &mut out);
        assert_eq!(out.len(), 128);
        assert_eq!(out[0].re, 1.0);
        assert_eq!(out[127].re, 2.0);
        assert_eq!(feed.read_index(), 128);
    }

    #[test]
    fn a_device_side_jump_becomes_a_gap_the_reader_steps_over() {
        let (mut taps, mut shared) = lane_taps(1, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(32, 1.0), 0);
        taps[0].push(&block(32, 2.0), 1_000);
        let feed = &mut shared.feeds[0];
        feed.settle();
        assert_eq!(feed.ready(), 32);
        let mut out = Vec::new();
        feed.take(32, &mut out);
        assert_eq!(feed.read_index(), 32);
        feed.settle();
        assert_eq!(feed.read_index(), 1_000);
        assert_eq!(feed.ready(), 32);
    }

    #[test]
    fn arming_late_shows_up_as_one_gap_not_as_shifted_samples() {
        let (mut taps, mut shared) = lane_taps(1, 48_000.0);
        taps[0].push(&block(128, 1.0), 0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(64, 2.0), 128);
        let feed = &mut shared.feeds[0];
        feed.settle();
        assert_eq!(feed.read_index(), 128);
        assert_eq!(feed.ready(), 64);
    }

    #[test]
    fn a_full_ring_reports_the_samples_it_could_not_keep() {
        let (mut taps, mut shared) = lane_taps(1, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        let capacity = tap_capacity(48_000.0);
        taps[0].push(&block(capacity, 1.0), 0);
        taps[0].push(&block(16, 2.0), capacity as u64);
        let feed = &mut shared.feeds[0];
        feed.settle();
        assert_eq!(feed.ready(), capacity);
        feed.skip(capacity);
        feed.settle();
        assert_eq!(feed.read_index(), capacity as u64 + 16);
    }

    #[test]
    fn rewind_drops_everything_still_queued() {
        let (mut taps, mut shared) = lane_taps(2, 48_000.0);
        shared.armed.store(true, Ordering::Relaxed);
        taps[0].push(&block(64, 1.0), 0);
        taps[1].push(&block(64, 1.0), 0);
        shared.rewind();
        for feed in &mut shared.feeds {
            feed.settle();
            assert_eq!(feed.ready(), 0);
            assert_eq!(feed.read_index(), 64);
        }
    }
}
