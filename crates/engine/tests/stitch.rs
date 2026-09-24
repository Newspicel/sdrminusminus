#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_dsp::stitch::auto_offsets;
use sdrmm_engine::Engine;
use sdrmm_wire::{
    CoherentParams, CombinerParams, DeviceSet, DeviceSettings, StitchMode, StitchParams,
    StreamSettings,
};

const BANK: &str = "virtual:bank5";
const RATE: f64 = 1_024_000.0;
const CENTRE_HZ: f64 = 300_000_000.0;
const LANES: [u32; 5] = [0, 1, 2, 3, 4];

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
}

fn stitch(mode: StitchMode) -> CoherentParams {
    CoherentParams::Stitch(StitchParams { mode, lanes: 5 })
}

fn open(engine: &Engine) -> u32 {
    let ds = engine.create_device_set(BANK).unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                center_hz: Some(CENTRE_HZ),
                sample_rate: Some(RATE),
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    ds
}

fn set(engine: &Engine, ds: u32) -> DeviceSet {
    engine
        .snapshot()
        .device_sets
        .into_iter()
        .find(|set| set.id == ds)
        .unwrap()
}

fn centers(engine: &Engine, ds: u32) -> Vec<f64> {
    let set = set(engine, ds);
    LANES
        .iter()
        .map(|lane| {
            set.settings
                .for_stream(*lane, &set.capabilities.per_stream)
                .center_hz
                .unwrap()
        })
        .collect()
}

fn laid_out(centers: &[f64], center_hz: f64) -> bool {
    centers
        .iter()
        .zip(auto_offsets(LANES.len(), RATE))
        .all(|(hz, offset)| (hz - center_hz - offset).abs() < 1.0)
}

fn peak_at(db: &[f32], offset: f64) -> f32 {
    let bins = db.len();
    let at = (((offset + 0.5) * bins as f64).round() as usize).min(bins - 1);
    db[at.saturating_sub(3)..(at + 4).min(bins)]
        .iter()
        .fold(f32::NEG_INFINITY, |a, b| a.max(*b))
}

fn tune_wide(engine: &Engine, ds: u32, hz: f64) {
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: vec![StreamSettings {
                    stream: 5,
                    center_hz: Some(hz),
                    ..StreamSettings::default()
                }],
                ..DeviceSettings::default()
            },
        )
        .unwrap();
}

#[tokio::test]
async fn auto_lays_the_lanes_side_by_side_and_runs_the_extra_lane_five_times_as_fast() {
    let engine = engine();
    let ds = open(&engine);
    engine
        .add_coherent(ds, stitch(StitchMode::Auto), LANES.to_vec())
        .unwrap();
    let lead = centers(&engine, ds)[0];
    assert!(laid_out(
        &centers(&engine, ds),
        lead - auto_offsets(5, RATE)[0]
    ));
    let extra = set(&engine, ds).extra_lane.expect("an extra lane");
    assert_eq!(extra.stream, 5);
    assert_eq!(extra.sample_rate, RATE * 5.0);
    let mut rx = engine.subscribe_spectrum(ds, 5).expect("the wide lane");
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a wide spectrum frame")
        .expect("a frame");
    assert!((f64::from(frame.span_hz) - RATE * 5.0).abs() < 1.0);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn tuning_the_wide_lane_moves_every_lane_with_it() {
    let engine = engine();
    let ds = open(&engine);
    engine
        .add_coherent(ds, stitch(StitchMode::Auto), LANES.to_vec())
        .unwrap();
    tune_wide(&engine, ds, 310e6);
    assert!(laid_out(&centers(&engine, ds), 310e6));
    assert_eq!(set(&engine, ds).extra_lane.unwrap().center_hz, 310e6);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn manual_keeps_the_lanes_where_they_are_and_centres_between_them() {
    let engine = engine();
    let ds = open(&engine);
    let spread: Vec<StreamSettings> = LANES
        .iter()
        .map(|lane| StreamSettings {
            stream: *lane,
            center_hz: Some(CENTRE_HZ + f64::from(*lane) * 2e6),
            ..StreamSettings::default()
        })
        .collect();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                streams: spread,
                ..DeviceSettings::default()
            },
        )
        .unwrap();
    engine
        .add_coherent(ds, stitch(StitchMode::Manual), LANES.to_vec())
        .unwrap();
    let before = centers(&engine, ds);
    assert_eq!(before[4] - before[0], 8e6);
    assert_eq!(
        set(&engine, ds).extra_lane.unwrap().center_hz,
        CENTRE_HZ + 4e6
    );
    tune_wide(&engine, ds, CENTRE_HZ + 5e6);
    let after = centers(&engine, ds);
    assert!(before.iter().zip(&after).all(|(a, b)| b - a == 1e6));
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_stitch_keeps_the_lanes_to_itself() {
    let engine = engine();
    let ds = open(&engine);
    engine
        .add_coherent(ds, stitch(StitchMode::Auto), LANES.to_vec())
        .unwrap();
    let combiner = CoherentParams::Combiner(CombinerParams {
        lanes: 2,
        ..CombinerParams::default()
    });
    assert!(engine.add_coherent(ds, combiner, vec![0, 1]).is_err());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_lanes_signal_lands_where_it_is_on_the_air_in_the_wide_spectrum() {
    let engine = engine();
    let ds = open(&engine);
    engine
        .add_coherent(ds, stitch(StitchMode::Auto), LANES.to_vec())
        .unwrap();
    let wide_hz = set(&engine, ds).extra_lane.unwrap().center_hz;
    let marker_hz = CENTRE_HZ + sdrmm_device_virtual::stream_marker_offset_hz(0);
    let mut rx = engine.subscribe_spectrum(ds, 5).expect("the wide lane");
    let mut margin = f32::NEG_INFINITY;
    let mut mirror = f32::NEG_INFINITY;
    for _ in 0..24 {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a wide spectrum frame")
            .expect("a frame");
        let mut sorted = frame.db.to_vec();
        sorted.sort_by(f32::total_cmp);
        let floor = sorted[frame.db.len() / 2];
        let offset = (marker_hz - wide_hz) / f64::from(frame.span_hz);
        margin = margin.max(peak_at(&frame.db, offset) - floor);
        mirror = mirror.max(peak_at(&frame.db, -offset) - floor);
    }
    assert!(
        margin > 20.0,
        "the marker stands {margin:.1} dB above the floor"
    );
    assert!(
        mirror < 10.0,
        "its mirror stands {mirror:.1} dB above the floor"
    );
    engine.remove_device_set(ds).unwrap();
}
