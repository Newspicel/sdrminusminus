#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use common::{assert_tone_dominates, settle_then_collect_second};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_recording::RecordingDriver;
use sdrmm_device_virtual::{NFM_CARRIER_OFFSET_HZ, VirtualDriver};
use sdrmm_engine::{Engine, FinalizedRecording};
use sdrmm_recorder::{BYTES_PER_SAMPLE, SigmfReader};
use sdrmm_wire::{ChannelParams, ChannelSettings, DeviceSettings, GainKind, GainValue, NfmParams};
use tempfile::TempDir;

const TEST_RATE: f64 = 2_400_000.0;
const RECORD_SAMPLES: u64 = (2.0 * TEST_RATE) as u64;

fn recording_engine(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    registry.register(10, Box::new(RecordingDriver::new(Some(dir.to_path_buf()))));
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

async fn record_siggen(engine: &Engine, rate: f64, min_samples: u64) -> FinalizedRecording {
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(rate),
                ..Default::default()
            },
        )
        .unwrap();
    engine.start_recording(ds, 0).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snap = engine.snapshot();
        let recording = snap.device_sets[0].recording.clone();
        if recording.as_ref().is_some_and(|r| r.samples >= min_samples) {
            break;
        }
        assert!(
            recording.as_ref().is_none_or(|r| r.error.is_none()),
            "recording failed at {recording:?}"
        );
        assert!(
            Instant::now() < deadline,
            "recording stalled at {recording:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let finalized = engine.stop_recording(ds).unwrap();
    engine.remove_device_set(ds).unwrap();
    assert_eq!(finalized.error, None);
    finalized
}

#[tokio::test]
async fn recorded_siggen_replays_and_demodulates() {
    let dir = TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let finalized = record_siggen(&engine, TEST_RATE, RECORD_SAMPLES).await;

    assert!(finalized.samples >= RECORD_SAMPLES);
    assert_eq!(finalized.bytes, finalized.samples * BYTES_PER_SAMPLE);
    let reader = SigmfReader::open(&finalized.stem).unwrap();
    assert_eq!(reader.meta().global.sample_rate, Some(TEST_RATE));
    assert_eq!(reader.total_samples(), finalized.samples);

    let playback_id = format!(
        "recording:{}",
        finalized.stem.file_name().unwrap().display()
    );
    assert!(
        engine.probe_devices().iter().any(|d| d.id() == playback_id),
        "finalized recording must probe as a playback device"
    );
    let ds = engine.create_device_set(&playback_id).unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                frequency_hz: 100_000_000.0 + NFM_CARRIER_OFFSET_HZ,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                blanker: Default::default(),
            },
        )
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_recording_takes_back_the_settings_a_receiver_left_on_the_node() {
    let dir = TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let finalized = record_siggen(&engine, TEST_RATE, TEST_RATE as u64 / 4).await;

    let ds = engine
        .create_device_set(&format!(
            "recording:{}",
            finalized.stem.file_name().unwrap().display()
        ))
        .unwrap();
    let left_by_a_receiver = DeviceSettings {
        center_hz: Some(100_000_000.0),
        sample_rate: Some(2_048_000.0),
        gains: vec![GainValue::new(GainKind::Tuner, 30.0)],
        bias_tee: Some(true),
        ..DeviceSettings::default()
    };

    engine
        .patch_device(ds, left_by_a_receiver.clone())
        .expect_err("a patch naming a bias tee this source has no such thing as must be refused");

    let capabilities = engine.capabilities(ds).expect("the recording is open");
    engine
        .patch_device(ds, left_by_a_receiver.supported_by(&capabilities))
        .expect("what the recording cannot take is dropped, not refused");

    let set = &engine.snapshot().device_sets[0];
    assert_eq!(set.settings.center_hz, Some(100_000_000.0));
    assert_eq!(set.settings.sample_rate, Some(TEST_RATE));
    assert!(set.settings.gains.is_empty());
    assert_eq!(
        set.settings.bias_tee, None,
        "the receiver's bias tee followed the node onto the recording"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn playback_streams_spectrum_frames() {
    let dir = TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    let finalized = record_siggen(&engine, TEST_RATE, TEST_RATE as u64 / 4).await;

    let ds = engine
        .create_device_set(&format!(
            "recording:{}",
            finalized.stem.file_name().unwrap().display()
        ))
        .unwrap();
    let mut rx = engine.subscribe_spectrum(ds, 0).unwrap();
    let snap = tokio::time::timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("spectrum within timeout")
        .expect("snapshot");
    assert_eq!(snap.center_hz, 100_000_000.0);
    assert_eq!(snap.span_hz, TEST_RATE as f32);

    let mut sorted = snap.db.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let peak = *sorted.last().unwrap();
    assert!(
        peak - median > 20.0,
        "recorded tones must be visible in the playback spectrum (peak {peak}, median {median})"
    );
    engine.remove_device_set(ds).unwrap();
}
