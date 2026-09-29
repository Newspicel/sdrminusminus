use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use sdrmm_device::{LaneEvent, LaneMark, lock, now_ns};
use sdrmm_wire::ArrayFailure;

use super::{
    align::{AlignNote, AlignNotes},
    window::PRE_GUARD,
};
use crate::EngineError;

pub(crate) const TAP_SECONDS: f64 = 0.1;
pub(crate) const TAP_MIN: usize = 1 << 16;
pub(crate) const TAP_MAX: usize = 1 << 22;
pub(crate) const TAP_EVENT_SLOTS: usize = 256;
const LEASE_SLOTS: usize = 2;
const RETIRED_SLOTS: usize = 4;
const ORIGIN_WINDOW: usize = 64;
const STAMP_RESERVE: usize = 16;
const NANOS_PER_SECOND: f64 = 1e9;

#[must_use]
pub(crate) fn tap_capacity(sample_rate: f64) -> usize {
    ((sample_rate * TAP_SECONDS) as usize).clamp(TAP_MIN, TAP_MAX)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TapEvent {
    Gap { at: u64, missing: u64 },
    Stamp { at: u64, host_ns: u64 },
    Lane(LaneEvent),
}

impl TapEvent {
    const fn at(&self) -> u64 {
        match *self {
            Self::Gap { at, .. }
            | Self::Stamp { at, .. }
            | Self::Lane(
                LaneEvent::Mark { at, .. }
                | LaneEvent::Uncertain { at, .. }
                | LaneEvent::HardwareTime { at, .. },
            ) => at,
        }
    }
}

pub(crate) struct TapRing {
    lease: u64,
    samples: Producer<Complex<f32>>,
    events: Producer<TapEvent>,
    ring_index: Option<u64>,
    pending: Option<TapEvent>,
    events_lost: Arc<AtomicU64>,
}

impl TapRing {
    fn write(&mut self, block: &[Complex<f32>], index: u64) {
        match self.ring_index {
            None => self.ring_index = Some(index),
            Some(expected) if index > expected => self.hold(expected, index - expected),
            Some(_) => {}
        }
        let end = index + block.len() as u64;
        if self.samples.slots() == 0 || !self.flush() {
            self.hold(index, block.len() as u64);
            self.ring_index = Some(end);
            return;
        }
        if self.events.slots() > STAMP_RESERVE {
            let _ = self.events.push(TapEvent::Stamp {
                at: index,
                host_ns: now_ns(),
            });
        }
        let take = self.samples.slots().min(block.len());
        if take > 0
            && let Ok(chunk) = self.samples.write_chunk_uninit(take)
        {
            chunk.fill_from_iter(block[..take].iter().copied());
        }
        if take < block.len() {
            self.hold(index + take as u64, (block.len() - take) as u64);
        }
        self.ring_index = Some(end);
    }

    fn event(&mut self, event: LaneEvent) {
        let at = TapEvent::Lane(event).at();
        match self.ring_index {
            None => self.ring_index = Some(at),
            Some(expected) if at > expected => {
                self.hold(expected, at - expected);
                self.ring_index = Some(at);
            }
            Some(_) => {}
        }
        if !self.flush() || self.events.push(TapEvent::Lane(event)).is_err() {
            self.events_lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn hold(&mut self, at: u64, missing: u64) {
        if missing == 0 {
            return;
        }
        self.pending = Some(match self.pending.take() {
            Some(TapEvent::Gap {
                at: held,
                missing: held_missing,
            }) => TapEvent::Gap {
                at: held,
                missing: (at + missing).saturating_sub(held).max(held_missing),
            },
            _ => TapEvent::Gap { at, missing },
        });
    }

    fn flush(&mut self) -> bool {
        match self.pending.take() {
            None => true,
            Some(gap) => match self.events.push(gap) {
                Ok(()) => true,
                Err(PushError::Full(gap)) => {
                    self.pending = Some(gap);
                    false
                }
            },
        }
    }
}

pub(crate) struct TapWriter {
    inbox: Consumer<Box<TapRing>>,
    outbox: Producer<Box<TapRing>>,
    active: Arc<AtomicU64>,
    current: Option<Box<TapRing>>,
}

impl TapWriter {
    pub(crate) fn samples(&mut self, samples: &[Complex<f32>], index: u64) {
        if let Some(ring) = self.live() {
            ring.write(samples, index);
        }
    }

    pub(crate) fn event(&mut self, event: LaneEvent) {
        if let Some(ring) = self.live() {
            ring.event(event);
        }
    }

    fn live(&mut self) -> Option<&mut TapRing> {
        self.adopt();
        let lease = self.current.as_ref()?.lease;
        if lease != self.active.load(Ordering::Relaxed) {
            self.retire_current();
            return None;
        }
        self.current.as_deref_mut()
    }

    fn adopt(&mut self) {
        if self.inbox.is_empty() || (self.current.is_some() && self.outbox.slots() == 0) {
            return;
        }
        if let Ok(next) = self.inbox.pop()
            && let Some(previous) = self.current.replace(next)
        {
            let _ = self.outbox.push(previous);
        }
    }

    fn retire_current(&mut self) {
        if let Some(ring) = self.current.take()
            && let Err(PushError::Full(ring)) = self.outbox.push(ring)
        {
            self.current = Some(ring);
        }
    }
}

pub(crate) struct TapPort {
    inbox: Mutex<Producer<Box<TapRing>>>,
    outbox: Mutex<Consumer<Box<TapRing>>>,
    active: Arc<AtomicU64>,
    next_lease: AtomicU64,
}

impl TapPort {
    pub(crate) fn new() -> (Arc<Self>, TapWriter) {
        let (inbox_tx, inbox_rx) = RingBuffer::new(LEASE_SLOTS);
        let (outbox_tx, outbox_rx) = RingBuffer::new(RETIRED_SLOTS);
        let active = Arc::new(AtomicU64::new(0));
        (
            Arc::new(Self {
                inbox: Mutex::new(inbox_tx),
                outbox: Mutex::new(outbox_rx),
                active: active.clone(),
                next_lease: AtomicU64::new(0),
            }),
            TapWriter {
                inbox: inbox_rx,
                outbox: outbox_tx,
                active,
                current: None,
            },
        )
    }

    pub(crate) fn lease(&self, sample_rate: f64) -> Result<LaneFeed, EngineError> {
        self.collect();
        let (samples_tx, samples_rx) = RingBuffer::new(tap_capacity(sample_rate));
        let (events_tx, events_rx) = RingBuffer::new(TAP_EVENT_SLOTS);
        let events_lost = Arc::new(AtomicU64::new(0));
        let lease = self.next_lease.fetch_add(1, Ordering::Relaxed) + 1;
        let ring = Box::new(TapRing {
            lease,
            samples: samples_tx,
            events: events_tx,
            ring_index: None,
            pending: None,
            events_lost: events_lost.clone(),
        });
        let mut inbox = lock(&self.inbox);
        if inbox.slots() == 0 {
            return Err(EngineError::Array(ArrayFailure::Busy));
        }
        self.active.store(lease, Ordering::Release);
        if inbox.push(ring).is_err() {
            return Err(EngineError::Array(ArrayFailure::Busy));
        }
        Ok(LaneFeed {
            lease,
            active: self.active.clone(),
            samples: samples_rx,
            events: events_rx,
            read_index: None,
            next: None,
            visible: 0,
            end_at: None,
            origin: OriginEstimate::new(sample_rate),
            events_lost,
        })
    }

    pub(crate) fn release(&self, lease: u64) {
        let _ = self
            .active
            .compare_exchange(lease, 0, Ordering::AcqRel, Ordering::Relaxed);
    }

    pub(crate) fn collect(&self) {
        let mut outbox = lock(&self.outbox);
        while outbox.pop().is_ok() {}
    }

    #[cfg(test)]
    fn retired(&self) -> usize {
        lock(&self.outbox).slots()
    }
}

pub(crate) struct LaneFeed {
    lease: u64,
    active: Arc<AtomicU64>,
    samples: Consumer<Complex<f32>>,
    events: Consumer<TapEvent>,
    read_index: Option<u64>,
    next: Option<TapEvent>,
    visible: usize,
    end_at: Option<u64>,
    origin: OriginEstimate,
    events_lost: Arc<AtomicU64>,
}

impl LaneFeed {
    pub(crate) const fn lease(&self) -> u64 {
        self.lease
    }

    pub(crate) fn events_lost(&self) -> u64 {
        self.events_lost.load(Ordering::Relaxed)
    }

    pub(crate) fn settle(&mut self, notes: &mut AlignNotes, lane: usize, offset: i64) {
        self.visible = self.samples.slots();
        loop {
            if self.next.is_none() {
                self.next = self.events.pop().ok();
            }
            let Some(event) = self.next else {
                return;
            };
            let read = *self.read_index.get_or_insert(event.at());
            if !self.apply(event, read, notes, lane, offset) {
                return;
            }
            self.next = None;
        }
    }

    fn apply(
        &mut self,
        event: TapEvent,
        read: u64,
        notes: &mut AlignNotes,
        lane: usize,
        offset: i64,
    ) -> bool {
        let lane = u8::try_from(lane).unwrap_or(u8::MAX);
        match event {
            TapEvent::Stamp { at, host_ns } => self.origin.observe(at, host_ns),
            TapEvent::Lane(LaneEvent::HardwareTime { .. }) => {}
            TapEvent::Gap { at, missing } if at <= read => {
                self.read_index = Some(read.max(at + missing));
                self.origin.reset();
                notes.push(AlignNote::Gap {
                    lane,
                    at: common(at, offset),
                    missing,
                });
            }
            TapEvent::Lane(LaneEvent::Uncertain {
                at,
                error,
                scope,
                cause,
            }) if at <= read => {
                self.origin.reset();
                notes.push(AlignNote::Uncertain {
                    lane,
                    at: common(at, offset),
                    error,
                    scope,
                    cause,
                });
            }
            TapEvent::Lane(LaneEvent::Mark {
                at,
                mark: LaneMark::Ended,
            }) if at <= read + PRE_GUARD => self.end_at = Some(at),
            TapEvent::Lane(LaneEvent::Mark { at, mark }) if at <= read + PRE_GUARD => {
                notes.push(AlignNote::Mark {
                    lane,
                    at: common(at, offset),
                    mark,
                });
            }
            _ => return false,
        }
        true
    }

    pub(crate) const fn read_index(&self) -> Option<u64> {
        self.read_index
    }

    pub(crate) const fn ended(&self) -> bool {
        self.end_at.is_some()
    }

    pub(crate) fn ready(&self) -> usize {
        let Some(read) = self.read_index else {
            return 0;
        };
        let held = self.visible;
        match self.next {
            Some(TapEvent::Lane(LaneEvent::Mark { at, .. })) => {
                held.min(at.saturating_sub(PRE_GUARD).saturating_sub(read) as usize)
            }
            Some(event) => held.min(event.at().saturating_sub(read) as usize),
            None => held.saturating_sub(PRE_GUARD as usize),
        }
    }

    pub(crate) fn skippable(&self) -> usize {
        let Some(read) = self.read_index else {
            return 0;
        };
        let held = self.visible;
        self.next.map_or(held, |event| {
            held.min(event.at().saturating_sub(read) as usize)
        })
    }

    pub(crate) fn skip(&mut self, count: usize) -> usize {
        let count = count.min(self.visible);
        if count == 0 {
            return 0;
        }
        let Ok(chunk) = self.samples.read_chunk(count) else {
            return 0;
        };
        chunk.commit_all();
        self.advance(count);
        count
    }

    pub(crate) fn take_into(&mut self, count: usize, out: &mut Vec<Complex<f32>>) {
        out.clear();
        let count = count.min(self.visible).min(out.capacity());
        let Ok(chunk) = self.samples.read_chunk(count) else {
            return;
        };
        let (head, tail) = chunk.as_slices();
        out.extend_from_slice(head);
        out.extend_from_slice(tail);
        chunk.commit_all();
        self.advance(count);
    }

    fn advance(&mut self, count: usize) {
        self.visible = self.visible.saturating_sub(count);
        if let Some(read) = self.read_index.as_mut() {
            *read += count as u64;
        }
        let resumed = self
            .end_at
            .zip(self.read_index)
            .is_some_and(|(end, read)| read > end);
        if resumed {
            self.end_at = None;
        }
    }

    pub(crate) fn origin_ns(&self) -> Option<i64> {
        self.origin.origin()
    }
}

impl Drop for LaneFeed {
    fn drop(&mut self) {
        let _ = self
            .active
            .compare_exchange(self.lease, 0, Ordering::AcqRel, Ordering::Relaxed);
    }
}

fn common(at: u64, offset: i64) -> u64 {
    u64::try_from(i128::from(at) - i128::from(offset)).unwrap_or(0)
}

pub(crate) struct OriginEstimate {
    window: Box<[i64; ORIGIN_WINDOW]>,
    at: usize,
    filled: usize,
    rate: f64,
}

impl OriginEstimate {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            window: Box::new([0; ORIGIN_WINDOW]),
            at: 0,
            filled: 0,
            rate,
        }
    }

    pub(crate) fn observe(&mut self, index: u64, host_ns: u64) {
        if !(self.rate.is_finite() && self.rate > 0.0) {
            return;
        }
        let origin = (host_ns as f64 - index as f64 * NANOS_PER_SECOND / self.rate).round();
        if !origin.is_finite() {
            return;
        }
        self.window[self.at] = origin as i64;
        self.at = (self.at + 1) % ORIGIN_WINDOW;
        self.filled = (self.filled + 1).min(ORIGIN_WINDOW);
    }

    pub(crate) fn origin(&self) -> Option<i64> {
        self.window[..self.filled].iter().min().copied()
    }

    pub(crate) const fn reset(&mut self) {
        self.at = 0;
        self.filled = 0;
    }
}

#[cfg(test)]
mod tests;
