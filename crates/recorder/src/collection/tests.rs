use std::fs;

use num_complex::Complex;
use sdrmm_wire::{ArrayGeometry, Coherence, DcArtifact, LaneKey, NoiseSource};
use tempfile::TempDir;

use super::*;
use crate::read_meta;

const RATE: f64 = 2_400_000.0;
const CENTERS: [f64; 3] = [433_920_000.0, 433_920_000.0, 433_920_000.0];

fn lanes() -> Vec<LaneMeta> {
    CENTERS
        .iter()
        .enumerate()
        .map(|(stream, center)| LaneMeta {
            lane: LaneKey {
                device: "virtual:kraken5".to_owned(),
                stream: stream as u32,
            },
            center_hz: *center,
        })
        .collect()
}

fn array() -> CollectionArray {
    CollectionArray {
        node: "array".to_owned(),
        tier: Coherence::TimeSync,
        geometry: ArrayGeometry::default(),
        noise_source: NoiseSource::Isolated,
        retune_keeps_phase: false,
        dc_artifact: DcArtifact::Managed,
    }
}

fn block(lane: usize, start: usize, n: usize) -> Vec<Complex<f32>> {
    (start..start + n)
        .map(|i| Complex::new(lane as f32 + 0.25, i as f32))
        .collect()
}

fn write(writer: &mut CollectionWriter, start: usize, n: usize) {
    let blocks: Vec<Vec<Complex<f32>>> = (0..writer.lanes()).map(|l| block(l, start, n)).collect();
    let views: Vec<&[Complex<f32>]> = blocks.iter().map(Vec::as_slice).collect();
    writer.write(&views).unwrap();
}

fn read_all(reader: &mut CollectionReader) -> Vec<(ReadChunk, Vec<Vec<Complex<f32>>>)> {
    let mut out = vec![Vec::new(); reader.lanes()];
    let mut chunks = Vec::new();
    loop {
        let chunk = reader.read(&mut out, 64).unwrap();
        let samples = match chunk {
            ReadChunk::Samples(_) => out.clone(),
            ReadChunk::End => break,
            _ => Vec::new(),
        };
        chunks.push((chunk, samples));
    }
    chunks
}

fn kinds(chunks: &[(ReadChunk, Vec<Vec<Complex<f32>>>)]) -> Vec<ReadChunk> {
    let mut merged: Vec<ReadChunk> = Vec::new();
    for (chunk, _) in chunks {
        match (merged.last_mut(), chunk) {
            (Some(ReadChunk::Samples(total)), ReadChunk::Samples(n)) => *total += n,
            _ => merged.push(chunk.clone()),
        }
    }
    merged
}

fn lane_samples(chunks: &[(ReadChunk, Vec<Vec<Complex<f32>>>)], lane: usize) -> Vec<Complex<f32>> {
    chunks
        .iter()
        .filter(|(chunk, _)| matches!(chunk, ReadChunk::Samples(_)))
        .flat_map(|(_, lanes)| lanes[lane].clone())
        .collect()
}

#[test]
fn a_collection_round_trips_lanes_and_offsets() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("bank");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    writer.offsets(&[0, 12, -7]).unwrap();
    write(&mut writer, 0, 100);
    writer.offsets(&[0, 13, -7]).unwrap();
    write(&mut writer, 100, 50);
    writer.finalize().unwrap();

    let mut reader = CollectionReader::open(&stem).unwrap();
    assert_eq!(reader.lanes(), 3);
    assert_eq!(reader.sample_rate(), RATE);
    assert_eq!(reader.centers_hz(), CENTERS);
    assert_eq!(reader.array(), &array());
    assert_eq!(reader.lane_keys()[2].stream, 2);
    assert_eq!(reader.total_samples(), 150);
    let chunks = read_all(&mut reader);
    assert_eq!(
        kinds(&chunks),
        vec![
            ReadChunk::Offsets(vec![0, 12, -7]),
            ReadChunk::Samples(100),
            ReadChunk::Offsets(vec![0, 13, -7]),
            ReadChunk::Samples(50),
        ]
    );
    for lane in 0..3 {
        assert_eq!(
            lane_samples(&chunks, lane),
            block(lane, 0, 150),
            "lane {lane}"
        );
    }
}

#[test]
fn the_collection_names_each_lane_by_its_meta_hash() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("hashed");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    write(&mut writer, 0, 10);
    writer.finalize().unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(collection_path(&stem)).unwrap()).unwrap();
    let collection = &json["collection"];
    assert_eq!(collection["core:version"], "1.2.6");
    assert_eq!(collection["core:extensions"][0]["name"], "sdrmm");
    assert_eq!(collection["sdrmm:array"]["node"], "array");
    assert_eq!(collection["sdrmm:array"]["tier"], "time_sync");
    assert_eq!(collection["sdrmm:array"]["dc_artifact"], "managed");
    assert_eq!(
        collection["sdrmm:array"]["lanes"][1]["device"],
        "virtual:kraken5"
    );
    let streams = collection["core:streams"].as_array().unwrap();
    assert_eq!(streams.len(), 3);
    for (lane, stream) in streams.iter().enumerate() {
        assert_eq!(stream["name"], format!("hashed-lane{lane}"));
        let digest = Sha512::digest(fs::read(meta_path(&lane_stem(&stem, lane))).unwrap());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(stream["hash"], hex);
        let meta = read_meta(&lane_stem(&stem, lane)).unwrap();
        assert_eq!(meta.global.lane, Some(lane as u32));
        assert_eq!(meta.global.rx_stream, Some(lane as u32));
        assert_eq!(meta.global.datatype, "cf32_le");
    }
    assert!(!tmp_collection_path(&stem).exists());
}

#[test]
fn a_gap_is_a_capture_with_a_global_index() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("gapped");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    write(&mut writer, 0, 100);
    writer.gap(50).unwrap();
    write(&mut writer, 100, 30);
    writer.gap(5).unwrap();
    write(&mut writer, 130, 20);
    writer.finalize().unwrap();

    for (lane, center) in CENTERS.iter().enumerate() {
        let meta = read_meta(&lane_stem(&stem, lane)).unwrap();
        let captures: Vec<(u64, Option<u64>)> = meta
            .captures
            .iter()
            .map(|capture| (capture.sample_start, capture.global_index))
            .collect();
        assert_eq!(
            captures,
            vec![(0, None), (100, Some(150)), (130, Some(185))]
        );
        assert_eq!(meta.captures[1].frequency, Some(*center));
        let json = serde_json::to_value(&meta).unwrap();
        assert_eq!(json["captures"][1]["core:global_index"], 150);
    }
    let mut reader = CollectionReader::open(&stem).unwrap();
    assert_eq!(
        kinds(&read_all(&mut reader)),
        vec![
            ReadChunk::Samples(100),
            ReadChunk::Gap(50),
            ReadChunk::Samples(30),
            ReadChunk::Gap(5),
            ReadChunk::Samples(20),
        ]
    );
}

#[test]
fn noise_windows_are_annotations() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("noisy");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    write(&mut writer, 0, 200);
    writer.noise(20, 40).unwrap();
    writer.noise(150, 100).unwrap();
    writer.finalize().unwrap();

    for lane in 0..3 {
        let json = serde_json::to_value(read_meta(&lane_stem(&stem, lane)).unwrap()).unwrap();
        let annotation = &json["annotations"][0];
        assert_eq!(annotation["core:sample_start"], 20);
        assert_eq!(annotation["core:sample_count"], 40);
        assert_eq!(annotation["core:label"], "noise");
    }
    let mut reader = CollectionReader::open(&stem).unwrap();
    assert_eq!(
        kinds(&read_all(&mut reader)),
        vec![
            ReadChunk::Samples(20),
            ReadChunk::Noise { on: true },
            ReadChunk::Samples(40),
            ReadChunk::Noise { on: false },
            ReadChunk::Samples(90),
            ReadChunk::Noise { on: true },
            ReadChunk::Samples(50),
            ReadChunk::Noise { on: false },
        ]
    );
}

#[test]
fn a_retune_starts_a_capture_on_every_lane() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("retuned");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    write(&mut writer, 0, 10);
    let moved = [145e6, 145.1e6, 145.2e6];
    writer.retuned(&moved, "2026-09-28T12:00:00Z").unwrap();
    write(&mut writer, 10, 10);
    writer.finalize().unwrap();

    let meta = read_meta(&lane_stem(&stem, 2)).unwrap();
    assert_eq!(meta.captures[1].sample_start, 10);
    assert_eq!(meta.captures[1].frequency, Some(145.2e6));
    let mut reader = CollectionReader::open(&stem).unwrap();
    assert_eq!(
        kinds(&read_all(&mut reader)),
        vec![
            ReadChunk::Samples(10),
            ReadChunk::Retuned(moved.to_vec()),
            ReadChunk::Samples(10),
        ]
    );
}

#[test]
fn a_changed_lane_meta_fails_the_hash() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("tampered");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    write(&mut writer, 0, 10);
    writer.finalize().unwrap();
    let lane = meta_path(&lane_stem(&stem, 1));
    let text = fs::read_to_string(&lane).unwrap();
    fs::write(&lane, text.replace("433920000", "433920001")).unwrap();

    match CollectionReader::open(&stem) {
        Err(SigmfError::Malformed(message)) => assert!(message.contains("hash"), "{message}"),
        other => panic!("a changed lane must be refused, got {other:?}"),
    }
}

#[test]
fn a_claimed_stem_and_wrong_lane_counts_are_refused() {
    let dir = TempDir::new().unwrap();
    let stem = dir.path().join("taken");
    let mut writer = CollectionWriter::create(&stem, &lanes(), RATE, array()).unwrap();
    assert!(matches!(
        CollectionWriter::create(&stem, &lanes(), RATE, array()),
        Err(SigmfError::StemTaken(_))
    ));
    let short = block(0, 0, 4);
    assert!(matches!(
        writer.write(&[&short]),
        Err(SigmfError::LaneCount {
            expected: 3,
            got: 1
        })
    ));
    let long = block(0, 0, 5);
    assert!(matches!(
        writer.write(&[&short, &long, &short]),
        Err(SigmfError::LaneLength)
    ));
    assert!(matches!(
        writer.retuned(&[1.0], "now"),
        Err(SigmfError::LaneCount { .. })
    ));
    assert!(matches!(
        CollectionWriter::create(&dir.path().join("empty"), &[], RATE, array()),
        Err(SigmfError::LaneCount { .. })
    ));
    writer.finalize().unwrap();
    assert!(matches!(
        CollectionWriter::create(&stem, &lanes(), RATE, array()),
        Err(SigmfError::StemTaken(_))
    ));
}

#[test]
fn a_collection_and_a_recording_never_share_a_stem() {
    let dir = TempDir::new().unwrap();
    let solo = dir.path().join("solo");
    SigmfWriter::create(&solo, RATE, 100e6, "test")
        .unwrap()
        .finalize()
        .unwrap();
    assert!(matches!(
        CollectionWriter::create(&solo, &lanes(), RATE, array()),
        Err(SigmfError::StemTaken(_))
    ));
    let bank = dir.path().join("bank");
    let writer = CollectionWriter::create(&bank, &lanes(), RATE, array()).unwrap();
    assert!(matches!(
        SigmfWriter::create(&bank, RATE, 100e6, "test"),
        Err(SigmfError::StemTaken(_))
    ));
    writer.finalize().unwrap();
    assert!(matches!(
        SigmfWriter::create(&bank, RATE, 100e6, "test"),
        Err(SigmfError::StemTaken(_))
    ));
    let meta = br#"{"global":{"core:datatype":"cf32_le","core:version":"1.2.6"},"captures":[]}"#;
    let imported = crate::import_pair(dir.path(), "bank", meta, &[0u8; 8][..]).unwrap();
    assert_eq!(imported.stem, dir.path().join("bank-2"));
}

#[test]
fn collections_are_found_by_their_suffix() {
    let dir = TempDir::new().unwrap();
    for name in ["b", "a"] {
        let mut writer =
            CollectionWriter::create(&dir.path().join(name), &lanes(), RATE, array()).unwrap();
        write(&mut writer, 0, 4);
        writer.finalize().unwrap();
    }
    let found = scan_collections(dir.path()).unwrap();
    assert_eq!(found, vec![dir.path().join("a"), dir.path().join("b")]);
    assert!(
        scan_collections(&dir.path().join("missing"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn lane_stems_are_told_apart_from_other_recordings() {
    let dir = Path::new("/recordings");
    let collection = dir.join("bank");
    assert_eq!(lane_of(&collection, &lane_stem(&collection, 0)), Some(0));
    assert_eq!(lane_of(&collection, &lane_stem(&collection, 12)), Some(12));
    for other in [
        "bank",
        "bank-lane",
        "bank-lanex",
        "bank-lane1b",
        "banks-lane1",
    ] {
        assert_eq!(lane_of(&collection, &dir.join(other)), None, "{other}");
    }
    assert_eq!(
        lane_of(&collection, &dir.join("sub").join("bank-lane0")),
        None
    );
}
