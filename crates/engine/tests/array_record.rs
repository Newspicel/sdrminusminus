#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{path::Path, time::Duration};

use common::array::{
    ARRAY, Bench, KRAKEN, LOCK_WAIT, WAIT, kraken_array, lanes, status, wait_calibrated,
    wait_solve_after, wait_status, wrap_deg,
};
use sdrmm_device_virtual::ReportedGap;
use sdrmm_recorder::{lane_stem, meta_path};
use sdrmm_wire::{ArrayRecordingRequest, ArrayStatus};

const GAP: u64 = 5_000;

fn named(name: &str) -> ArrayRecordingRequest {
    ArrayRecordingRequest {
        name: Some(name.to_owned()),
    }
}

fn exact_gap(bench: &Bench, error: u64) {
    bench.impair(KRAKEN, 1, |lane| {
        lane.gaps = vec![ReportedGap {
            at: 0,
            missing: GAP,
            reported: GAP,
            error,
        }];
    });
}

fn phases(status: &ArrayStatus) -> Vec<f64> {
    status
        .lanes
        .iter()
        .map(|lane| f64::from(lane.phase_deg))
        .collect()
}

fn captures(dir: &Path, stem: &str) -> Vec<(u64, u64)> {
    let meta = meta_path(&lane_stem(&dir.join(stem), 0));
    let text = std::fs::read_to_string(&meta).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&text).unwrap();
    meta["captures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|capture| {
            let start = capture["core:sample_start"].as_u64().unwrap();
            let global = capture["core:global_index"].as_u64().unwrap_or(start);
            (start, global)
        })
        .collect()
}

fn stop(bench: &Bench) {
    std::thread::sleep(Duration::from_millis(200));
    bench.engine.stop_array_recording(ARRAY).unwrap();
}

#[test]
fn an_array_recording_replays_to_the_same_calibration() {
    let dir = tempfile::TempDir::new().unwrap();
    let bench = Bench::recording_into(dir.path().to_path_buf());
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    let stem = bench
        .engine
        .start_array_recording(ARRAY, named("bench-array"))
        .unwrap();
    exact_gap(&bench, 3 * GAP);
    let resynced = wait_solve_after(&bench.engine, &first, WAIT);
    bench.engine.calibrate_array(ARRAY).unwrap();
    let original = wait_solve_after(&bench.engine, &resynced, WAIT);
    stop(&bench);
    assert_eq!(status(&bench.engine).recording, None);
    bench.engine.remove_array(ARRAY).unwrap();
    let replay = bench
        .engine
        .create_device_set(&format!("recording:{stem}"))
        .unwrap();
    let mut spec = kraken_array(replay);
    spec.lanes = lanes(replay, 0..5);
    bench.engine.apply_array(spec).unwrap();
    let replayed = wait_calibrated(&bench.engine, LOCK_WAIT);
    for (lane, (was, now)) in phases(&original).iter().zip(phases(&replayed)).enumerate() {
        assert!(
            wrap_deg(now - was).abs() < 0.5,
            "lane {lane}: {now} replayed, {was} recorded"
        );
    }
}

#[test]
fn a_recording_gap_becomes_a_capture_with_a_global_index() {
    let dir = tempfile::TempDir::new().unwrap();
    let bench = Bench::recording_into(dir.path().to_path_buf());
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let before = wait_calibrated(&bench.engine, LOCK_WAIT);
    let stem = bench
        .engine
        .start_array_recording(ARRAY, named("bench-gap"))
        .unwrap();
    wait_status(&bench.engine, "samples on disk", WAIT, |now| {
        now.recording
            .as_ref()
            .is_some_and(|recording| recording.samples > 0)
    });
    exact_gap(&bench, 0);
    wait_solve_after(&bench.engine, &before, WAIT);
    stop(&bench);
    let captures = captures(dir.path(), &stem);
    let skew = |(start, global): (u64, u64)| i128::from(global) - i128::from(start);
    let jumps: Vec<i128> = captures
        .windows(2)
        .map(|pair| skew(pair[1]) - skew(pair[0]))
        .filter(|jump| *jump != 0)
        .collect();
    assert_eq!(jumps, vec![i128::from(GAP)], "{captures:?}");
}
