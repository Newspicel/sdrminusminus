use num_complex::Complex;
use sdrmm_wire::{ArrayGeometry, Coherence, DcArtifact, LaneKey, NoiseSource};
use tempfile::TempDir;

use super::*;
use crate::{
    CollectionArray, CollectionReader, CollectionWriter, LaneMeta, SigmfWriter, annotate, lane_stem,
};

pub(crate) fn collection(dir: &Path, name: &str, lanes: usize, samples: usize) -> PathBuf {
    let stem = dir.join(name);
    let metas: Vec<LaneMeta> = (0..lanes)
        .map(|stream| LaneMeta {
            lane: LaneKey {
                device: "virtual:kraken5".to_owned(),
                stream: stream as u32,
            },
            center_hz: 433_920_000.0,
        })
        .collect();
    let array = CollectionArray {
        node: "array".to_owned(),
        tier: Coherence::TimeSync,
        geometry: ArrayGeometry::default(),
        noise_source: NoiseSource::Isolated,
        retune_keeps_phase: false,
        dc_artifact: DcArtifact::Managed,
    };
    let mut writer = CollectionWriter::create(&stem, &metas, 2_400_000.0, array).unwrap();
    let block = vec![Complex::new(0.25f32, -0.25); samples];
    let views: Vec<&[Complex<f32>]> = (0..lanes).map(|_| block.as_slice()).collect();
    writer.write(&views).unwrap();
    writer.finalize().unwrap();
    stem
}

fn single(dir: &Path, name: &str) -> PathBuf {
    let stem = dir.join(name);
    let mut writer = SigmfWriter::create(&stem, 48_000.0, 7_100_000.0, "hw").unwrap();
    writer.write_block(&[Complex::new(0.5, 0.5)]).unwrap();
    writer.finalize().unwrap();
    stem
}

#[test]
fn a_collection_is_one_library_entry_and_its_lanes_are_hidden() {
    let dir = TempDir::new().unwrap();
    let take = collection(dir.path(), "take", 3, 16);
    let alone = single(dir.path(), "alone");
    let lookalike = single(dir.path(), "take-laneX");

    assert_eq!(
        scan_library(dir.path()).unwrap(),
        [
            Stored::Recording(alone),
            Stored::Collection(take.clone()),
            Stored::Recording(lookalike),
        ]
    );
    assert_eq!(Stored::at(&take), Stored::Collection(take.clone()));
    assert_eq!(Stored::at(&take).shown_file(), collection_path(&take));
    assert!(scan_library(&dir.path().join("gone")).unwrap().is_empty());
}

#[test]
fn removing_a_collection_takes_every_lane_with_it() {
    let dir = TempDir::new().unwrap();
    let take = collection(dir.path(), "take", 4, 8);
    let kept = single(dir.path(), "kept");
    std::fs::remove_file(meta_path(&lane_stem(&take, 2))).unwrap();

    Stored::at(&take).remove().unwrap();

    assert_eq!(scan_library(dir.path()).unwrap(), [Stored::Recording(kept)]);
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 2, "{left:?}");
    Stored::at(&take).remove().unwrap();
}

#[test]
fn a_collection_annotation_lands_in_the_collection_and_keeps_it_playable() {
    let dir = TempDir::new().unwrap();
    let take = collection(dir.path(), "take", 2, 8);
    let tags = vec!["df".to_owned()];

    annotate(&take, Some("Rooftop"), &tags, Some("five lanes")).unwrap();

    let reader = CollectionReader::open(&take).unwrap();
    assert_eq!(reader.notes().name.as_deref(), Some("Rooftop"));
    assert_eq!(reader.notes().tags, tags);
    assert_eq!(reader.notes().note.as_deref(), Some("five lanes"));
    assert_eq!(reader.hardware(), ["virtual:kraken5"]);
    assert!(reader.started_at().is_some());
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(collection_path(&take)).unwrap()).unwrap();
    assert_eq!(raw["collection"]["sdrmm:name"], "Rooftop");
    assert_eq!(raw["collection"]["core:description"], "five lanes");

    annotate(&take, None, &[], None).unwrap();
    let cleared = CollectionReader::open(&take).unwrap();
    assert_eq!(cleared.notes(), &crate::RecordingNotes::default());
}
