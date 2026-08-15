// Tests may unwrap/expect (CLAUDE.md); clippy's `allow-unwrap-in-tests` only covers
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Per-channel audio recording, end to end through the virtual radio: what the engine writes is
//! the audio a listener on that channel would have heard.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::{NFM_CARRIER_OFFSET_HZ, VirtualDriver};
use sdrmm_engine::Engine;
use sdrmm_recorder::read_audio_info;
use sdrmm_wire::{
    AdsbParams, AudioRecordingStatus, ChannelParams, ChannelSettings, DeviceSettings, NfmParams,
    WfmParams,
};

/// Same rate the listening tests use: it keeps the siggen's static tones clear of the modulated
/// carriers, so what lands in the file is the carrier and not a neighbour.
const TEST_RATE: f64 = 2_400_000.0;

fn engine(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.to_path_buf())),
    );
    Engine::with_registry(registry, Some(dir.to_path_buf()))
}

fn set_at_test_rate(engine: &Engine) -> u32 {
    let ds = engine.create_device_set("virtual:siggen").unwrap();
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(TEST_RATE),
                ..Default::default()
            },
        )
        .unwrap();
    ds
}

fn settings(params: ChannelParams, offset_hz: f64) -> ChannelSettings {
    ChannelSettings {
        offset_hz,
        squelch_db: None,
        squelch_auto_db: None,
        params,
        audio: Default::default(),
    }
}

fn nfm_channel(engine: &Engine, ds: u32) -> u32 {
    engine
        .add_channel(
            ds,
            0,
            settings(
                ChannelParams::Nfm(NfmParams::default()),
                NFM_CARRIER_OFFSET_HZ,
            ),
        )
        .unwrap()
}

fn live_status(engine: &Engine, ds: u32, ch: u32) -> Option<AudioRecordingStatus> {
    engine
        .snapshot()
        .device_sets
        .iter()
        .find(|s| s.id == ds)?
        .channels
        .iter()
        .find(|c| c.id == ch)?
        .audio_recording
        .clone()
}

/// The virtual radio is paced in real time, so a recording's progress has to be waited for.
async fn wait_for_frames(engine: &Engine, ds: u32, ch: u32, min: u64) -> AudioRecordingStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = live_status(engine, ds, ch) {
            assert_eq!(status.error, None, "the recording faulted while waiting");
            if status.frames >= min {
                return status;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the recording never reached {min} frames"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Sixteen-bit samples straight out of the file, so the assertion is on what was written and
/// not on what the engine says it wrote.
fn wav_samples(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap();
    bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect()
}

fn rms(samples: &[i16]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

#[tokio::test]
async fn a_channel_recording_lands_as_a_playable_wav_of_its_audio() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    let started = engine.start_channel_recording(ds, ch).unwrap();
    assert!(started.file.ends_with(".wav"));
    assert_eq!(started.channels, 1);
    started.started_at.parse::<jiff::Timestamp>().unwrap();

    // A tenth of a second of audio, which is well past the first block.
    let live = wait_for_frames(&engine, ds, ch, 4_800).await;
    assert_eq!(live.file, started.file);

    let final_status = engine.stop_channel_recording(ds, ch).unwrap();
    assert_eq!(final_status.error, None);
    assert!(final_status.frames >= live.frames);
    assert_eq!(final_status.bytes, final_status.frames * 2);
    assert!(live_status(&engine, ds, ch).is_none());

    let path = engine.audio_recordings_dir().unwrap().join(&started.file);
    let info = read_audio_info(&path).unwrap();
    assert_eq!(info.channels, 1);
    assert_eq!(info.sample_rate, 48_000);
    assert_eq!(info.frames, final_status.frames);
    assert!(
        rms(&wav_samples(&path)) > 100.0,
        "the file holds no audio at all"
    );

    engine.remove_device_set(ds).unwrap();
}

/// A stereo mode records both sides, and the header has to say so — a file that claimed mono
/// would play the two channels back at half speed, interleaved into each other.
#[tokio::test]
async fn a_stereo_channel_records_two_channel_audio() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(
            ds,
            0,
            settings(
                ChannelParams::Wfm(WfmParams::default()),
                sdrmm_device_virtual::WFM_CARRIER_OFFSET_HZ,
            ),
        )
        .unwrap();

    let started = engine.start_channel_recording(ds, ch).unwrap();
    assert_eq!(started.channels, 2);
    wait_for_frames(&engine, ds, ch, 4_800).await;
    let done = engine.stop_channel_recording(ds, ch).unwrap();
    assert_eq!(done.bytes, done.frames * 4, "two channels of 16-bit audio");

    let path = engine.audio_recordings_dir().unwrap().join(&started.file);
    assert_eq!(read_audio_info(&path).unwrap().channels, 2);
    engine.remove_device_set(ds).unwrap();
}

/// The recording belongs to the channel, not to the pipeline underneath it: a device rate
/// change rebuilds every host, and the file has to keep growing across the swap.
#[tokio::test]
async fn a_rate_change_does_not_end_a_channel_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    engine.start_channel_recording(ds, ch).unwrap();
    let before = wait_for_frames(&engine, ds, ch, 2_400).await;
    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(2_048_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    let after = wait_for_frames(&engine, ds, ch, before.frames + 4_800).await;
    assert_eq!(after.file, before.file, "the recording was restarted");
    assert_eq!(after.error, None);

    let done = engine.stop_channel_recording(ds, ch).unwrap();
    assert!(done.frames > before.frames);
    engine.remove_device_set(ds).unwrap();
}

/// Removing the channel is an implicit stop: the file has to be finished rather than left
/// half-written with a writer thread still holding it.
#[tokio::test]
async fn removing_the_channel_finalizes_its_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    let started = engine.start_channel_recording(ds, ch).unwrap();
    wait_for_frames(&engine, ds, ch, 4_800).await;
    engine.remove_channel(ds, ch).unwrap();

    let path = engine.audio_recordings_dir().unwrap().join(&started.file);
    let info = read_audio_info(&path).unwrap();
    assert!(info.frames > 0);
    assert!(
        engine.stop_channel_recording(ds, ch).is_err(),
        "the recording outlived its channel"
    );
    engine.remove_device_set(ds).unwrap();
}

/// Switching the channel to a mode with no audio leaves the recording nothing to write, so it
/// is finished then and there rather than left open on a stream that has stopped.
#[tokio::test]
async fn a_mode_change_to_a_silent_decoder_finishes_the_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    let started = engine.start_channel_recording(ds, ch).unwrap();
    wait_for_frames(&engine, ds, ch, 4_800).await;
    engine
        .patch_channel(
            ds,
            ch,
            settings(ChannelParams::Adsb(AdsbParams::default()), 0.0),
        )
        .unwrap();

    assert!(live_status(&engine, ds, ch).is_none());
    let path = engine.audio_recordings_dir().unwrap().join(&started.file);
    assert!(read_audio_info(&path).unwrap().frames > 0);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn what_cannot_be_recorded_is_refused_rather_than_left_running_empty() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    engine.start_channel_recording(ds, ch).unwrap();
    assert!(
        engine.start_channel_recording(ds, ch).is_err(),
        "a second recording of the same channel was accepted"
    );
    engine.stop_channel_recording(ds, ch).unwrap();
    assert!(engine.stop_channel_recording(ds, ch).is_err());

    // A decoder with no audio output would record a file that never grows.
    let adsb = engine
        .add_channel(
            ds,
            0,
            settings(ChannelParams::Adsb(AdsbParams::default()), 0.0),
        )
        .unwrap();
    let refused = engine.start_channel_recording(ds, adsb).unwrap_err();
    assert!(
        refused.to_string().contains("no audio"),
        "unexpected refusal: {refused}"
    );

    assert!(engine.start_channel_recording(ds, 9_999).is_err());
    assert!(engine.start_channel_recording(9_999, ch).is_err());
    engine.remove_device_set(ds).unwrap();
}

/// Without a recordings directory there is nowhere to write, and that has to be said rather
/// than reported as a channel that will not record.
#[tokio::test]
async fn recording_without_a_recordings_directory_is_refused() {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);

    assert!(engine.audio_recordings_dir().is_none());
    let err = engine.start_channel_recording(ds, ch).unwrap_err();
    assert!(err.to_string().contains("recordings directory"), "{err}");
    engine.remove_device_set(ds).unwrap();
}
