use num_complex::Complex;
use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_device::{GapScope, LaneMark, UNKNOWN_ERROR, Uncertainty};

use super::tap::LaneFeed;

pub(crate) const ALIGN_BLOCK: usize = 16_384;
pub(crate) const NOTE_SLOTS: usize = 64;
const ALIGN_ROUNDS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AlignNote {
    Gap {
        lane: u8,
        at: u64,
        missing: u64,
    },
    Uncertain {
        lane: u8,
        at: u64,
        error: u64,
        scope: GapScope,
        cause: Uncertainty,
    },
    Mark {
        lane: u8,
        at: u64,
        mark: LaneMark,
    },
    Realigned {
        at: u64,
    },
}

impl AlignNote {
    pub(crate) const fn at(&self) -> u64 {
        match *self {
            Self::Gap { at, .. }
            | Self::Uncertain { at, .. }
            | Self::Mark { at, .. }
            | Self::Realigned { at } => at,
        }
    }
}

pub(crate) struct AlignNotes {
    notes: [AlignNote; NOTE_SLOTS],
    len: usize,
    dropped: u32,
    forced: bool,
}

impl AlignNotes {
    pub(crate) const fn new() -> Self {
        Self {
            notes: [AlignNote::Realigned { at: 0 }; NOTE_SLOTS],
            len: 0,
            dropped: 0,
            forced: false,
        }
    }

    pub(crate) fn push(&mut self, note: AlignNote) {
        if self.len < NOTE_SLOTS {
            self.notes[self.len] = note;
            self.len += 1;
            return;
        }
        self.dropped += 1;
        if matches!(note, AlignNote::Realigned { .. }) {
            return;
        }
        if !self.forced {
            self.forced = true;
            if !matches!(self.notes[NOTE_SLOTS - 1], AlignNote::Realigned { .. }) {
                self.dropped += 1;
            }
        }
        self.notes[NOTE_SLOTS - 1] = AlignNote::Realigned { at: note.at() };
    }

    pub(crate) fn as_slice(&self) -> &[AlignNote] {
        &self.notes[..self.len]
    }

    pub(crate) const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub(crate) const fn forced(&self) -> bool {
        self.forced
    }

    pub(crate) const fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
        self.forced = false;
    }
}

pub(crate) struct Aligner {
    feeds: Vec<Option<LaneFeed>>,
    offsets: [i64; MAX_LANES],
    raw: Vec<Vec<Complex<f32>>>,
    index: u64,
    realigns: u64,
    skipped: u64,
    realigning: bool,
    delivered: bool,
    moved: bool,
}

impl Aligner {
    pub(crate) fn new(mut feeds: Vec<Option<LaneFeed>>) -> Self {
        feeds.truncate(MAX_LANES);
        let raw = (0..feeds.len())
            .map(|_| Vec::with_capacity(ALIGN_BLOCK))
            .collect();
        Self {
            feeds,
            offsets: [0; MAX_LANES],
            raw,
            index: 0,
            realigns: 0,
            skipped: 0,
            realigning: false,
            delivered: false,
            moved: false,
        }
    }

    pub(crate) fn lanes(&self) -> usize {
        self.feeds.len()
    }

    pub(crate) fn offsets(&self) -> &[i64] {
        &self.offsets[..self.feeds.len()]
    }

    pub(crate) fn set_offsets(&mut self, offsets: &[i64]) {
        for (held, offset) in self.offsets.iter_mut().zip(offsets) {
            self.moved |= *held != *offset;
            *held = *offset;
        }
    }

    const fn counts(&self) -> bool {
        self.delivered && !self.moved
    }

    const fn mark_delivered(&mut self) {
        self.delivered = true;
        self.moved = false;
    }

    pub(crate) fn swap_feed(&mut self, slot: usize, feed: Option<LaneFeed>) -> Option<LaneFeed> {
        match self.feeds.get_mut(slot) {
            Some(held) => std::mem::replace(held, feed),
            None => feed,
        }
    }

    pub(crate) fn has_lost_lane(&self) -> bool {
        self.feeds.iter().any(Option::is_none)
    }

    pub(crate) const fn index(&self) -> u64 {
        self.index
    }

    pub(crate) const fn realigns(&self) -> u64 {
        self.realigns
    }

    pub(crate) const fn skipped(&self) -> u64 {
        self.skipped
    }

    pub(crate) fn events_lost(&self) -> u64 {
        self.feeds.iter().flatten().map(LaneFeed::events_lost).sum()
    }

    pub(crate) fn origins(&self, out: &mut [Option<i64>]) {
        for (slot, feed) in out.iter_mut().zip(&self.feeds) {
            *slot = feed.as_ref().and_then(LaneFeed::origin_ns);
        }
    }

    pub(crate) fn take_feeds(&mut self) -> Vec<Option<LaneFeed>> {
        std::mem::take(&mut self.feeds)
    }

    pub(crate) fn next(&mut self, notes: &mut AlignNotes) -> Option<usize> {
        if self.feeds.is_empty() || self.has_lost_lane() {
            return None;
        }
        let mut common = None;
        for _ in 0..ALIGN_ROUNDS {
            self.settle(notes);
            let target = self.common()?;
            match self.line_up(target, notes) {
                Some(true) => {
                    common = Some(target);
                    break;
                }
                Some(false) => {}
                None => return None,
            }
        }
        let common = common?;
        self.realigning = false;
        let count = self
            .feeds
            .iter()
            .flatten()
            .map(LaneFeed::ready)
            .min()
            .unwrap_or(0)
            .min(ALIGN_BLOCK);
        if count == 0 {
            return None;
        }
        for (feed, raw) in self.feeds.iter_mut().zip(&mut self.raw) {
            if let Some(feed) = feed {
                feed.take_into(count, raw);
            }
        }
        self.index = common;
        self.mark_delivered();
        Some(count)
    }

    pub(crate) fn ended(&self) -> bool {
        self.feeds.iter().flatten().any(LaneFeed::ended)
    }

    pub(crate) fn settle(&mut self, notes: &mut AlignNotes) {
        for (lane, feed) in self.feeds.iter_mut().enumerate() {
            if let Some(feed) = feed {
                feed.settle(notes, lane, self.offsets[lane]);
            }
        }
    }

    fn common(&self) -> Option<u64> {
        let mut common: Option<i128> = None;
        for (feed, offset) in self.feeds.iter().zip(&self.offsets) {
            let read = feed.as_ref()?.read_index()?;
            let lane_common = i128::from(read) - i128::from(*offset);
            common = Some(common.map_or(lane_common, |held| held.max(lane_common)));
        }
        u64::try_from(common?.max(0)).ok()
    }

    fn line_up(&mut self, common: u64, notes: &mut AlignNotes) -> Option<bool> {
        let mut aligned = true;
        let counts = self.counts();
        for lane in 0..self.feeds.len() {
            let offset = self.offsets[lane];
            let Ok(target) = u64::try_from(i128::from(common) + i128::from(offset)) else {
                self.unaligned(lane, common, notes);
                return None;
            };
            let feed = self.feeds[lane].as_mut()?;
            let read = feed.read_index()?;
            if read >= target {
                continue;
            }
            if !self.realigning {
                self.realigning = true;
                self.realigns += u64::from(counts);
                notes.push(AlignNote::Realigned { at: common });
            }
            let need = usize::try_from(target - read).unwrap_or(usize::MAX);
            let skipped = feed.skip(need.min(feed.skippable()));
            if counts {
                self.skipped += skipped as u64;
            }
            if skipped < need {
                aligned = false;
            }
        }
        Some(aligned)
    }

    fn unaligned(&mut self, lane: usize, common: u64, notes: &mut AlignNotes) {
        self.offsets[lane] = 0;
        notes.push(AlignNote::Uncertain {
            lane: u8::try_from(lane).unwrap_or(u8::MAX),
            at: common,
            error: UNKNOWN_ERROR,
            scope: GapScope::Lane,
            cause: Uncertainty::Unaligned,
        });
    }

    pub(crate) fn with_raw<R>(&self, count: usize, f: impl FnOnce(&[&[Complex<f32>]]) -> R) -> R {
        let mut view: [&[Complex<f32>]; MAX_LANES] = [&[]; MAX_LANES];
        for (slot, lane) in view.iter_mut().zip(&self.raw) {
            *slot = &lane[..count.min(lane.len())];
        }
        f(&view[..self.raw.len()])
    }

    pub(crate) fn raw_mut(&mut self) -> &mut [Vec<Complex<f32>>] {
        &mut self.raw
    }
}

#[cfg(test)]
mod tests;
