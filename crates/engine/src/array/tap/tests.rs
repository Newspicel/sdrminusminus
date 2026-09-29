use std::time::Duration;

use num_complex::Complex;
use sdrmm_device::{GapScope, LaneEvent, LaneMark, Uncertainty, init_clock, now_ns};

use super::*;
use crate::array::align::{AlignNote, AlignNotes};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 8_192;

fn block(len: usize, value: f32) -> Vec<Complex<f32>> {
    vec![Complex::new(value, 0.0); len]
}

fn settle(feed: &mut LaneFeed) -> Vec<AlignNote> {
    let mut notes = AlignNotes::new();
    feed.settle(&mut notes, 0, 0);
    notes.as_slice().to_vec()
}

#[test]
fn a_dormant_tap_writes_nothing() {
    let (port, mut writer) = TapPort::new();
    writer.samples(&block(BLOCK, 1.0), 0);
    writer.event(LaneEvent::Mark {
        at: BLOCK as u64,
        mark: LaneMark::Retuned { in_flight: 0 },
    });
    assert!(writer.current.is_none());
    let mut feed = port.lease(RATE).expect("lease");
    assert!(settle(&mut feed).is_empty());
    assert_eq!(feed.read_index(), None);
    assert_eq!(feed.ready(), 0);
}

#[test]
fn a_lease_starts_at_the_next_block_without_a_gap() {
    let (port, mut writer) = TapPort::new();
    writer.samples(&block(BLOCK, 1.0), 0);
    let mut feed = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 2.0), BLOCK as u64);
    writer.samples(&block(BLOCK, 3.0), 2 * BLOCK as u64);
    assert!(settle(&mut feed).is_empty());
    assert_eq!(feed.read_index(), Some(BLOCK as u64));
    assert_eq!(feed.ready(), 2 * BLOCK - PRE_GUARD as usize);
    let mut out = Vec::with_capacity(BLOCK);
    feed.take_into(BLOCK, &mut out);
    assert_eq!(out.len(), BLOCK);
    assert!(out.iter().all(|sample| sample.re == 2.0));
    assert_eq!(feed.read_index(), Some(2 * BLOCK as u64));
}

#[test]
fn a_released_ring_leaves_the_capture_thread_through_the_outbox() {
    let (port, mut writer) = TapPort::new();
    let feed = port.lease(RATE).expect("lease");
    writer.samples(&block(64, 1.0), 0);
    assert!(writer.current.is_some());
    port.release(feed.lease());
    writer.samples(&block(64, 1.0), 64);
    assert!(writer.current.is_none());
    assert_eq!(port.retired(), 1);
    port.collect();
    assert_eq!(port.retired(), 0);
}

#[test]
fn a_dropped_feed_releases_its_lease() {
    let (port, mut writer) = TapPort::new();
    let feed = port.lease(RATE).expect("lease");
    writer.samples(&block(64, 1.0), 0);
    drop(feed);
    writer.samples(&block(64, 1.0), 64);
    assert!(writer.current.is_none());
    assert_eq!(port.retired(), 1);
}

#[test]
fn a_stale_lease_never_writes_into_a_new_one() {
    let (port, mut writer) = TapPort::new();
    let mut old = port.lease(RATE).expect("lease");
    let old_lease = old.lease();
    writer.samples(&block(BLOCK, 1.0), 0);
    let mut new = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 2.0), BLOCK as u64);
    writer.samples(&block(BLOCK, 3.0), 2 * BLOCK as u64);
    settle(&mut old);
    settle(&mut new);
    assert_eq!(old.skippable(), BLOCK);
    assert_eq!(new.read_index(), Some(BLOCK as u64));
    assert_eq!(new.skippable(), 2 * BLOCK);
    drop(old);
    writer.samples(&block(BLOCK, 4.0), 3 * BLOCK as u64);
    settle(&mut new);
    assert_eq!(new.skippable(), 3 * BLOCK);
    assert!(new.lease() > old_lease);
}

#[test]
fn a_full_ring_records_an_exact_gap() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    let capacity = tap_capacity(RATE);
    writer.samples(&block(capacity, 1.0), 0);
    writer.samples(&block(100, 2.0), capacity as u64);
    assert!(settle(&mut feed).is_empty());
    assert_eq!(feed.skippable(), capacity);
    assert_eq!(feed.skip(capacity), capacity);
    writer.samples(&block(10, 3.0), capacity as u64 + 100);
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Gap {
            lane: 0,
            at: capacity as u64,
            missing: 100
        }]
    );
    assert_eq!(feed.read_index(), Some(capacity as u64 + 100));
    assert_eq!(feed.skippable(), 10);
}

#[test]
fn a_long_stall_is_one_gap_not_one_per_block() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    let capacity = tap_capacity(RATE);
    writer.samples(&block(capacity, 1.0), 0);
    let mut at = capacity as u64;
    for _ in 0..40 {
        writer.samples(&block(BLOCK, 2.0), at);
        at += BLOCK as u64;
    }
    settle(&mut feed);
    feed.skip(capacity);
    writer.samples(&block(BLOCK, 3.0), at);
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Gap {
            lane: 0,
            at: capacity as u64,
            missing: 40 * BLOCK as u64
        }]
    );
    assert_eq!(feed.skippable(), BLOCK);
}

#[test]
fn a_device_side_jump_becomes_a_gap_the_reader_steps_over() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 1.0), 0);
    writer.samples(&block(BLOCK, 2.0), 3 * BLOCK as u64);
    settle(&mut feed);
    assert_eq!(feed.ready(), BLOCK);
    feed.skip(BLOCK);
    let notes = settle(&mut feed);
    assert_eq!(
        notes,
        [AlignNote::Gap {
            lane: 0,
            at: BLOCK as u64,
            missing: 2 * BLOCK as u64
        }]
    );
    assert_eq!(feed.read_index(), Some(3 * BLOCK as u64));
}

#[test]
fn lane_events_keep_their_order_with_samples() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    let big = 3 * PRE_GUARD as usize;
    let uncertain = LaneEvent::Uncertain {
        at: big as u64,
        error: 10,
        scope: GapScope::Lane,
        cause: Uncertainty::EstimatedGap,
    };
    let mark = LaneMark::GainChanged { in_flight: 5 };
    writer.samples(&block(big, 1.0), 0);
    writer.event(uncertain);
    writer.samples(&block(big, 2.0), big as u64);
    writer.event(LaneEvent::Mark {
        at: 2 * big as u64,
        mark,
    });
    writer.samples(&block(big, 3.0), 2 * big as u64);
    assert!(settle(&mut feed).is_empty());
    assert_eq!(feed.ready(), big);
    feed.skip(big);
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Uncertain {
            lane: 0,
            at: big as u64,
            error: 10,
            scope: GapScope::Lane,
            cause: Uncertainty::EstimatedGap,
        }]
    );
    assert_eq!(feed.ready(), big - PRE_GUARD as usize);
    feed.skip(big - PRE_GUARD as usize);
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Mark {
            lane: 0,
            at: 2 * big as u64,
            mark
        }]
    );
    assert_eq!(feed.ready(), big);
}

#[test]
fn an_event_after_a_device_jump_waits_behind_its_gap() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 1.0), 0);
    writer.event(LaneEvent::Uncertain {
        at: 2 * BLOCK as u64,
        error: 1,
        scope: GapScope::Lane,
        cause: Uncertainty::Rearmed,
    });
    writer.samples(&block(BLOCK, 2.0), 2 * BLOCK as u64);
    settle(&mut feed);
    feed.skip(feed.ready());
    let notes = settle(&mut feed);
    assert_eq!(notes.len(), 2);
    assert!(
        matches!(notes[0], AlignNote::Gap { at, missing, .. } if at == BLOCK as u64 && missing == BLOCK as u64)
    );
    assert!(matches!(notes[1], AlignNote::Uncertain { at, .. } if at == 2 * BLOCK as u64));
}

#[test]
fn stamps_estimate_the_stream_origin_within_a_block() {
    init_clock();
    let (port, mut writer) = TapPort::new();
    let rate = 1_000_000.0;
    let mut feed = port.lease(rate).expect("lease");
    let len = 10_000usize;
    let start = now_ns();
    for block_index in 0..5u64 {
        let due = start + block_index * 10_000_000;
        while now_ns() < due {
            std::thread::sleep(Duration::from_micros(200));
        }
        writer.samples(&block(len, 1.0), block_index * len as u64);
    }
    settle(&mut feed);
    let origin = feed.origin_ns().expect("stamped");
    let start = i64::try_from(start).expect("fits");
    assert!(
        origin >= start && origin < start + 10_000_000,
        "origin {origin} vs first block at {start}"
    );
}

#[test]
fn a_gap_forgets_the_origin_estimate() {
    let mut origin = OriginEstimate::new(1_000.0);
    origin.observe(0, 5_000_000);
    origin.observe(1_000, 1_004_000_000);
    assert_eq!(origin.origin(), Some(4_000_000));
    origin.reset();
    assert_eq!(origin.origin(), None);
}

#[test]
fn an_end_mark_ends_the_feed_until_samples_come_back() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 1.0), 0);
    let end = BLOCK as u64;
    writer.event(LaneEvent::Mark {
        at: end,
        mark: LaneMark::Ended,
    });
    assert!(settle(&mut feed).is_empty());
    assert!(!feed.ended());
    let ready = feed.ready();
    assert_eq!(ready, BLOCK - PRE_GUARD as usize);
    let mut out = Vec::with_capacity(2 * BLOCK);
    feed.take_into(ready, &mut out);
    assert!(settle(&mut feed).is_empty(), "the end is no note");
    assert!(feed.ended());
    assert_eq!(feed.ready(), 0);
    let held = feed.skippable();
    assert_eq!(feed.skip(held), PRE_GUARD as usize);
    assert_eq!(feed.read_index(), Some(end));
    assert!(feed.ended(), "the held tail is still before the end");
    writer.samples(&block(BLOCK, 2.0), end);
    assert!(settle(&mut feed).is_empty());
    assert!(feed.ended());
    let ready = feed.ready();
    assert_eq!(ready, BLOCK - PRE_GUARD as usize);
    feed.take_into(ready, &mut out);
    assert!(!feed.ended());
}

#[test]
fn stamps_leave_room_for_lane_events() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    let len = 10;
    for at in 0..2 * TAP_EVENT_SLOTS as u64 {
        writer.samples(&block(len, 1.0), at * len as u64);
    }
    let at = 2 * TAP_EVENT_SLOTS as u64 * len as u64;
    let mark = LaneMark::NoiseSource {
        on: true,
        in_flight: 0,
    };
    writer.event(LaneEvent::Mark { at, mark });
    assert_eq!(feed.events_lost(), 0);
    let mut notes = AlignNotes::new();
    while feed.read_index() != Some(at) {
        feed.settle(&mut notes, 0, 0);
        let skip = feed.skippable();
        assert!(skip > 0 || feed.read_index() == Some(at), "stuck");
        feed.skip(skip);
    }
    feed.settle(&mut notes, 0, 0);
    assert_eq!(notes.as_slice(), [AlignNote::Mark { lane: 0, at, mark }]);
}

#[test]
fn samples_that_land_after_a_settle_never_run_past_an_unseen_gap_or_mark() {
    let (port, mut writer) = TapPort::new();
    let mut feed = port.lease(RATE).expect("lease");
    writer.samples(&block(BLOCK, 1.0), 0);
    assert!(settle(&mut feed).is_empty());
    let resumed = BLOCK as u64 + 500;
    let marked = resumed + BLOCK as u64;
    let mark = LaneMark::NoiseSource {
        on: true,
        in_flight: 0,
    };
    writer.samples(&block(BLOCK, 2.0), resumed);
    writer.event(LaneEvent::Mark { at: marked, mark });
    writer.samples(&block(BLOCK, 3.0), marked);
    assert_eq!(feed.skippable(), BLOCK);
    assert_eq!(feed.ready(), BLOCK - PRE_GUARD as usize);
    let mut out = Vec::with_capacity(4 * BLOCK);
    feed.take_into(4 * BLOCK, &mut out);
    assert_eq!(out.len(), BLOCK);
    assert!(out.iter().all(|sample| sample.re == 1.0));
    assert_eq!(feed.read_index(), Some(BLOCK as u64));
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Gap {
            lane: 0,
            at: BLOCK as u64,
            missing: 500
        }]
    );
    assert_eq!(feed.read_index(), Some(resumed));
    let ready = feed.ready();
    assert_eq!(ready, BLOCK - PRE_GUARD as usize);
    feed.take_into(ready, &mut out);
    assert!(out.iter().all(|sample| sample.re == 2.0));
    assert_eq!(
        settle(&mut feed),
        [AlignNote::Mark {
            lane: 0,
            at: marked,
            mark
        }]
    );
}
