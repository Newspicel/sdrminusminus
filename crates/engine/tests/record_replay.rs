//! Record → replay e2e (PLAN §16 M3): a siggen set is recorded to a SigMF pair, the pair
//! opens as a `virtual:file:` playback device, and an NFM channel recovers the 1 kHz tone
//! from the recorded IQ. Hermetic — tempdir only, no fixture files (PLAN §14).

// Tests may unwrap/expect (CLAUDE.md); clippy's `allow-unwrap-in-tests` only covers
// `#[cfg(test)]` items, which an integration-test crate's helpers are not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use common::{assert_tone_dominates, settle_then_collect_second};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::{NFM_CARRIER_OFFSET_HZ, VirtualDriver};
use sdrmm_engine::{Engine, FinalizedRecording};
use sdrmm_recorder::{BYTES_PER_SAMPLE, SigmfReader};
use sdrmm_wire::{ChannelParams, ChannelSettings, DeviceSettings, NfmParams};
use tempfile::TempDir;

/// Same rationale as listen.rs: 2.4 Msps keeps the static siggen tones clear of the
/// modulated carrier bands.
const TEST_RATE: f64 = 2_400_000.0;
/// The judged playback window (0.5 s settle + 1 s judge + startup slack) must fit inside
/// one loop pass of the recording, or the wrap transient lands in the judged audio.
const RECORD_SAMPLES: u64 = (2.0 * TEST_RATE) as u64;

fn recording_engine(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

/// Record at least `min_samples` of the siggen at `rate`, then stop and tear the set down.
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
    engine.start_recording(ds).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let snap = engine.snapshot();
        let recording = snap.device_sets[0].recording.clone();
        if recording.as_ref().is_some_and(|r| r.samples >= min_samples) {
            break;
        }
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

    let playback_id = format!("virtual:file:{}", finalized.stem.display());
    assert!(
        engine.probe_devices().iter().any(|d| d.id() == playback_id),
        "finalized recording must probe as a playback device"
    );
    let ds = engine.create_device_set(&playback_id).unwrap();
    let ch = engine
        .add_channel(
            ds,
            ChannelSettings {
                offset_hz: NFM_CARRIER_OFFSET_HZ,
                squelch_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
            },
        )
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn playback_streams_spectrum_frames() {
    let dir = TempDir::new().unwrap();
    let engine = recording_engine(dir.path());
    // A short clip suffices: looped playback keeps the spectrum tap fed indefinitely.
    let finalized = record_siggen(&engine, TEST_RATE, TEST_RATE as u64 / 4).await;

    let ds = engine
        .create_device_set(&format!("virtual:file:{}", finalized.stem.display()))
        .unwrap();
    let mut rx = engine.subscribe_spectrum(ds).unwrap();
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
