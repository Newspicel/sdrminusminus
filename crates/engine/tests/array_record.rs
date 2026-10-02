#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{path::Path, time::Duration};

use common::array::{
    ARRAY, Bench, KRAKEN, LOCK_WAIT, WAIT, calibrated, df, kraken_array, lanes, processor_status,
    status, wait_calibrated, wait_solve_after, wait_status, wrap_deg,
};
use sdrmm_device_virtual::ReportedGap;
use sdrmm_recorder::{lane_stem, meta_path};
use sdrmm_wire::{ArrayRecordingRequest, ArrayStatus, CalPhase, SyncState};

const GAP: u64 = 5_000;
const TAIL: Duration = Duration::from_secs(2);

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

fn replay(bench: &Bench, stem: &str) {
    bench.engine.remove_array(ARRAY).unwrap();
    let replay = bench
        .engine
        .create_device_set(&format!("recording:{stem}"))
        .unwrap();
    let mut spec = kraken_array(replay);
    spec.lanes = lanes(replay, 0..5);
    bench.engine.apply_array(spec).unwrap();
}

fn same_phases(original: &ArrayStatus, replayed: &ArrayStatus) {
    for (lane, (was, now)) in phases(original).iter().zip(phases(replayed)).enumerate() {
        assert!(
            wrap_deg(now - was).abs() < 0.5,
            "lane {lane}: {now} replayed, {was} recorded"
        );
    }
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
    replay(&bench, &stem);
    let replayed = wait_calibrated(&bench.engine, LOCK_WAIT);
    same_phases(&original, &replayed);
}

#[test]
fn a_recording_with_one_calibration_replays_without_a_search() {
    let dir = tempfile::TempDir::new().unwrap();
    let bench = Bench::recording_into(dir.path().to_path_buf());
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    let stem = bench
        .engine
        .start_array_recording(ARRAY, named("bench-once"))
        .unwrap();
    bench.engine.calibrate_array(ARRAY).unwrap();
    let original = wait_solve_after(&bench.engine, &first, WAIT);
    stop(&bench);
    replay(&bench, &stem);
    let replayed = wait_calibrated(&bench.engine, LOCK_WAIT);
    same_phases(&original, &replayed);
}

#[test]
fn a_finished_replay_settles_to_its_recorded_calibration() {
    let dir = tempfile::TempDir::new().unwrap();
    let bench = Bench::recording_into(dir.path().to_path_buf());
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    let stem = bench
        .engine
        .start_array_recording(ARRAY, named("bench-end"))
        .unwrap();
    bench.engine.calibrate_array(ARRAY).unwrap();
    wait_solve_after(&bench.engine, &first, WAIT);
    std::thread::sleep(TAIL);
    stop(&bench);
    replay(&bench, &stem);
    bench.engine.apply_processor(df("df")).unwrap();
    let recorded = wait_calibrated(&bench.engine, LOCK_WAIT);
    wait_status(
        &bench.engine,
        "the replay past its noise window",
        WAIT,
        |now| {
            let df = processor_status(now, "df");
            df.running && df.gated.is_none()
        },
    );
    bench.engine.calibrate_array(ARRAY).unwrap();
    wait_status(
        &bench.engine,
        "a solve the recording cannot finish",
        WAIT,
        |now| now.cal == CalPhase::Measuring,
    );
    let settled = wait_status(&bench.engine, "the recorded calibration", WAIT, calibrated);
    assert_eq!(settled.last_solve_at, recorded.last_solve_at);
    same_phases(&recorded, &settled);
}

#[test]
fn a_recording_started_before_the_lock_replays_to_the_same_phases() {
    let dir = tempfile::TempDir::new().unwrap();
    let bench = Bench::recording_into(dir.path().to_path_buf());
    let ds = bench.open(KRAKEN);
    bench.engine.apply_array(kraken_array(ds)).unwrap();
    let stem = bench
        .engine
        .start_array_recording(ARRAY, named("bench-cold"))
        .unwrap();
    let first = wait_calibrated(&bench.engine, LOCK_WAIT);
    bench.engine.calibrate_array(ARRAY).unwrap();
    let original = wait_solve_after(&bench.engine, &first, WAIT);
    stop(&bench);
    replay(&bench, &stem);
    let replayed = wait_status(&bench.engine, "replayed phases", LOCK_WAIT, |now| {
        now.phase_ready && now.sync == SyncState::Locked
    });
    same_phases(&original, &replayed);
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
