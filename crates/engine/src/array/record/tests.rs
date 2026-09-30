use num_complex::Complex;
use sdrmm_recorder::{CollectionArray, CollectionReader, LaneMeta, ReadChunk};
use sdrmm_wire::{ArrayGeometry, Coherence, DcArtifact, LaneKey, NoiseSource};

use super::*;

const RATE: f64 = 48_000.0;

fn writer(stem: &std::path::Path) -> CollectionWriter {
    let lanes: Vec<LaneMeta> = (0..2)
        .map(|stream| LaneMeta {
            lane: LaneKey {
                device: "virtual:test".to_owned(),
                stream,
            },
            center_hz: 100e6,
        })
        .collect();
    CollectionWriter::create(
        stem,
        &lanes,
        RATE,
        CollectionArray {
            node: "array-1".to_owned(),
            tier: Coherence::TimeSync,
            geometry: ArrayGeometry::default(),
            noise_source: NoiseSource::Isolated,
            retune_keeps_phase: false,
            dc_artifact: DcArtifact::None,
        },
    )
    .expect("collection")
}

fn block(len: usize, value: f32) -> Vec<Complex<f32>> {
    vec![Complex::new(value, -value); len]
}

#[test]
fn a_recording_keeps_lanes_gaps_and_noise_windows() {
    let dir = tempfile::TempDir::new().expect("scratch");
    let stem = dir.path().join("array");
    let (mut tap, reader) = record_tap(2, RATE);
    let recording = ArrayRecording::start(
        "sdrmm-array-rec-test".to_owned(),
        "array".to_owned(),
        "now".to_owned(),
        writer(&stem),
        &tap,
        reader,
    )
    .expect("recorder");
    let first = block(1_000, 0.25);
    tap.push(&[&first, &first], 10_000, &[]);
    let second = block(1_000, 0.5);
    tap.push(
        &[&second, &second],
        11_500,
        &[
            RecordNote::NoiseOn { at: 11_500 },
            RecordNote::NoiseOff { at: 12_000 },
        ],
    );
    let third = block(500, 0.75);
    tap.push(&[&third, &third], 12_500, &[]);
    drop(tap);
    let status = recording.finish();
    assert_eq!(status.error, None);
    assert_eq!(status.samples, 2_500);
    assert_eq!(status.dropped, 0);

    let mut reader = CollectionReader::open(&stem).expect("read back");
    let mut out = vec![Vec::new(); 2];
    let mut samples = 0;
    let mut chunks = Vec::new();
    loop {
        match reader.read(&mut out, 4_096).expect("chunk") {
            ReadChunk::End => break,
            ReadChunk::Samples(count) => samples += count,
            other => chunks.push(other),
        }
    }
    assert_eq!(samples, 2_500);
    assert!(chunks.contains(&ReadChunk::Gap(500)));
    assert!(chunks.contains(&ReadChunk::Noise { on: true }));
    assert!(chunks.contains(&ReadChunk::Noise { on: false }));
}

#[test]
fn a_full_ring_drops_the_block_on_every_lane_and_counts_it() {
    let (mut tap, mut reader) = record_tap(2, RATE);
    let capacity = reader.lanes[0].buffer().capacity();
    let big = block(capacity, 1.0);
    tap.push(&[&big, &big], 0, &[]);
    let more = block(10, 1.0);
    tap.push(&[&more, &more], capacity as u64, &[]);
    assert_eq!(tap.shared.dropped.load(Ordering::Relaxed), 10);
    assert_eq!(reader.notes.pop(), Ok(RecordNote::Start { at: 0 }));
    assert_eq!(
        reader.notes.pop(),
        Ok(RecordNote::Gap {
            at: capacity as u64,
            missing: 10
        })
    );
    assert!(reader.lanes.iter().all(|lane| lane.slots() == capacity));
}

#[test]
fn a_noise_note_behind_the_writer_marks_its_own_sample() {
    let dir = tempfile::TempDir::new().expect("scratch");
    let stem = dir.path().join("late");
    let (mut tap, reader) = record_tap(2, RATE);
    let recording = ArrayRecording::start(
        "sdrmm-array-rec-late".to_owned(),
        "late".to_owned(),
        "now".to_owned(),
        writer(&stem),
        &tap,
        reader,
    )
    .expect("recorder");
    let first = block(1_000, 0.25);
    tap.push(&[&first, &first], 0, &[]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while recording.status().samples < 1_000 {
        assert!(std::time::Instant::now() < deadline, "the writer stalled");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let second = block(1_000, 0.5);
    tap.push(
        &[&second, &second],
        1_000,
        &[
            RecordNote::NoiseOn { at: 600 },
            RecordNote::NoiseOff { at: 1_500 },
        ],
    );
    drop(tap);
    assert_eq!(recording.finish().error, None);
    let mut reader = CollectionReader::open(&stem).expect("read back");
    let mut out = vec![Vec::new(); 2];
    let mut samples = 0;
    let mut switches = Vec::new();
    loop {
        match reader.read(&mut out, 100).expect("chunk") {
            ReadChunk::End => break,
            ReadChunk::Samples(count) => samples += count,
            ReadChunk::Noise { on } => switches.push((on, samples)),
            _ => {}
        }
    }
    assert_eq!(switches, [(true, 600), (false, 1_500)]);
}
