use std::sync::Arc;

use num_complex::Complex;
use sdrmm_device::{LaneEvent, LaneMark};

use super::*;
use crate::array::{
    tap::{TapPort, TapWriter},
    window::PRE_GUARD,
};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 8_192;
const LATE: u64 = 4_096;

struct Lanes {
    _ports: Vec<Arc<TapPort>>,
    writers: Vec<TapWriter>,
}

fn aligner(lanes: usize) -> (Lanes, Aligner) {
    let mut ports = Vec::new();
    let mut writers = Vec::new();
    let mut feeds = Vec::new();
    for _ in 0..lanes {
        let (port, writer) = TapPort::new();
        feeds.push(Some(port.lease(RATE).expect("lease")));
        ports.push(port);
        writers.push(writer);
    }
    (
        Lanes {
            _ports: ports,
            writers,
        },
        Aligner::new(feeds),
    )
}

fn ramp(start: u64, len: usize, shift: i64) -> Vec<Complex<f32>> {
    (0..len as u64)
        .map(|at| Complex::new((start + at) as f32 - shift as f32, 0.0))
        .collect()
}

fn lane_starts(aligner: &Aligner, count: usize) -> (Vec<Complex<f32>>, Vec<Complex<f32>>) {
    aligner.with_raw(count, |raw| (raw[0].to_vec(), raw[1].to_vec()))
}

fn drain(aligner: &mut Aligner, notes: &mut AlignNotes) -> u64 {
    let mut end = 0;
    while let Some(count) = aligner.next(notes) {
        end = aligner.index() + count as u64;
    }
    end
}

fn quiet(notes: &AlignNotes) -> bool {
    notes.as_slice().is_empty() && notes.dropped() == 0
}

#[test]
fn offsets_line_lanes_up_on_the_common_index() {
    let (mut lanes, mut aligner) = aligner(2);
    aligner.set_offsets(&[0, 100]);
    lanes.writers[0].samples(&ramp(0, 4 * BLOCK, 0), 0);
    lanes.writers[1].samples(&ramp(0, 4 * BLOCK, 100), 0);
    let mut notes = AlignNotes::new();
    let count = aligner.next(&mut notes).expect("a block");
    assert!(count > 0);
    assert_eq!(aligner.index(), 0);
    let (first, second) = lane_starts(&aligner, count);
    assert_eq!(first, second);
    assert_eq!(first[0].re, 0.0);
}

#[test]
fn lining_up_at_start_is_neither_a_realign_nor_a_drop() {
    let (mut lanes, mut aligner) = aligner(2);
    lanes.writers[0].samples(&ramp(0, 4 * BLOCK, 0), 0);
    lanes.writers[1].samples(&ramp(700, 4 * BLOCK, 0), 700);
    let mut notes = AlignNotes::new();
    let first = aligner.next(&mut notes).expect("a block");
    aligner.set_offsets(&[0, 300]);
    let second = aligner.next(&mut notes).expect("a synced block");
    let (lead, lag) = lane_starts(&aligner, second);
    assert_eq!(lag[0].re - lead[0].re, 300.0);
    assert!(first > 0);
    assert_eq!((aligner.realigns(), aligner.skipped()), (0, 0));
}

#[test]
fn a_solved_offset_change_skips_forward_without_counting_a_drop() {
    let (mut lanes, mut aligner) = aligner(2);
    for writer in &mut lanes.writers {
        writer.samples(&ramp(0, 8 * BLOCK, 0), 0);
    }
    let mut notes = AlignNotes::new();
    aligner.set_offsets(&[0, 20]);
    let synced = aligner.next(&mut notes).expect("a block");
    notes.clear();
    aligner.set_offsets(&[0, 50]);
    let moved = aligner.next(&mut notes).expect("a block");
    assert_eq!(
        notes.as_slice(),
        [AlignNote::Realigned { at: synced as u64 }]
    );
    let (lead, lag) = lane_starts(&aligner, moved);
    assert_eq!(lag[0].re - lead[0].re, 50.0);
    assert_eq!((aligner.realigns(), aligner.skipped()), (0, 0));
}

#[test]
fn a_gap_on_one_lane_skips_every_lane() {
    let (mut lanes, mut aligner) = aligner(2);
    lanes.writers[0].samples(&ramp(0, 4 * BLOCK, 0), 0);
    lanes.writers[1].samples(&ramp(0, BLOCK, 0), 0);
    lanes.writers[1].samples(&ramp(2 * BLOCK as u64, 2 * BLOCK, 0), 2 * BLOCK as u64);
    let mut notes = AlignNotes::new();
    assert_eq!(aligner.next(&mut notes), Some(BLOCK));
    assert!(quiet(&notes));
    let count = aligner.next(&mut notes).expect("a block after the gap");
    assert_eq!(aligner.index(), 2 * BLOCK as u64);
    assert!(notes.as_slice().contains(&AlignNote::Gap {
        lane: 1,
        at: BLOCK as u64,
        missing: BLOCK as u64
    }));
    assert!(
        notes
            .as_slice()
            .iter()
            .any(|note| matches!(note, AlignNote::Realigned { .. }))
    );
    let (first, second) = lane_starts(&aligner, count);
    assert_eq!(first, second);
    assert_eq!(first[0].re, 2.0 * BLOCK as f32);
    assert_eq!((aligner.realigns(), aligner.skipped()), (1, BLOCK as u64));
}

#[test]
fn a_lane_swapped_in_after_a_solve_counts_its_realign_and_drop() {
    let (mut lanes, mut aligner) = aligner(2);
    let block = BLOCK as u64;
    for writer in &mut lanes.writers {
        writer.samples(&ramp(0, 2 * BLOCK, 0), 0);
    }
    let mut notes = AlignNotes::new();
    assert_eq!(drain(&mut aligner, &mut notes), 2 * block - PRE_GUARD);
    aligner.set_offsets(&[0, 20]);
    for writer in &mut lanes.writers {
        writer.samples(&ramp(2 * block, BLOCK, 0), 2 * block);
    }
    let solved = drain(&mut aligner, &mut notes);
    assert_eq!(solved, 3 * block - PRE_GUARD - 20);
    assert_eq!((aligner.realigns(), aligner.skipped()), (0, 0));
    let (port, mut writer) = TapPort::new();
    let fresh = port.lease(RATE).expect("lease");
    assert!(aligner.swap_feed(1, Some(fresh)).is_some());
    let restart = 3 * block + LATE;
    lanes.writers[0].samples(&ramp(3 * block, 2 * BLOCK, 0), 3 * block);
    writer.samples(&ramp(restart + 20, BLOCK, 0), restart + 20);
    notes.clear();
    let count = aligner.next(&mut notes).expect("a block after the swap");
    assert_eq!(aligner.index(), restart);
    let (lead, lag) = lane_starts(&aligner, count);
    assert_eq!(lag[0].re - lead[0].re, 20.0);
    assert_eq!(
        (aligner.realigns(), aligner.skipped()),
        (1, restart - solved)
    );
}

#[test]
fn events_are_translated_to_the_common_index() {
    let (mut lanes, mut aligner) = aligner(2);
    aligner.set_offsets(&[0, 1_000]);
    lanes.writers[0].samples(&ramp(0, 2 * BLOCK, 0), 0);
    lanes.writers[1].samples(&ramp(0, 2 * BLOCK, 1_000), 0);
    lanes.writers[1].event(LaneEvent::Mark {
        at: 2 * BLOCK as u64,
        mark: LaneMark::Retuned { in_flight: 0 },
    });
    lanes.writers[0].samples(&ramp(2 * BLOCK as u64, 2 * BLOCK, 0), 2 * BLOCK as u64);
    lanes.writers[1].samples(&ramp(2 * BLOCK as u64, 2 * BLOCK, 1_000), 2 * BLOCK as u64);
    let mut notes = AlignNotes::new();
    let mut seen = Vec::new();
    while aligner.next(&mut notes).is_some() {
        seen.extend_from_slice(notes.as_slice());
        notes.clear();
    }
    assert!(seen.contains(&AlignNote::Mark {
        lane: 1,
        at: 2 * BLOCK as u64 - 1_000,
        mark: LaneMark::Retuned { in_flight: 0 },
    }));
}

#[test]
fn a_mark_cuts_the_block_a_guard_ahead_of_itself() {
    let (mut lanes, mut aligner) = aligner(1);
    lanes.writers[0].samples(&ramp(0, 4 * BLOCK, 0), 0);
    lanes.writers[0].event(LaneEvent::Mark {
        at: 4 * BLOCK as u64,
        mark: LaneMark::NoiseSource {
            on: true,
            in_flight: 0,
        },
    });
    lanes.writers[0].samples(&ramp(4 * BLOCK as u64, BLOCK, 0), 4 * BLOCK as u64);
    let mut notes = AlignNotes::new();
    let mut end = 0;
    while let Some(count) = aligner.next(&mut notes) {
        if !quiet(&notes) {
            break;
        }
        end = aligner.index() + count as u64;
    }
    assert_eq!(end, 4 * BLOCK as u64 - PRE_GUARD);
    assert!(matches!(notes.as_slice(), [AlignNote::Mark { .. }]));
}

#[test]
fn a_lost_lane_holds_the_array() {
    let (port, mut writer) = TapPort::new();
    let feed = port.lease(RATE).expect("lease");
    let mut aligner = Aligner::new(vec![Some(feed), None]);
    writer.samples(&ramp(0, 4 * BLOCK, 0), 0);
    let mut notes = AlignNotes::new();
    assert_eq!(aligner.next(&mut notes), None);
    assert!(aligner.has_lost_lane());
    let (other, mut other_writer) = TapPort::new();
    let fresh = other.lease(RATE).expect("lease");
    assert!(aligner.swap_feed(1, Some(fresh)).is_none());
    other_writer.samples(&ramp(0, 4 * BLOCK, 0), 0);
    assert!(aligner.next(&mut notes).is_some());
}

#[test]
fn overflowing_notes_count_and_force_a_realign() {
    let mut notes = AlignNotes::new();
    for at in 0..(NOTE_SLOTS as u64 + 6) {
        notes.push(AlignNote::Mark {
            lane: 0,
            at,
            mark: LaneMark::Retuned { in_flight: 0 },
        });
    }
    assert_eq!(notes.as_slice().len(), NOTE_SLOTS);
    assert!(notes.forced());
    assert_eq!(notes.dropped(), 7);
    assert!(matches!(
        notes.as_slice()[NOTE_SLOTS - 1],
        AlignNote::Realigned { .. }
    ));
    notes.clear();
    assert!(quiet(&notes));
    assert!(!notes.forced());
}
