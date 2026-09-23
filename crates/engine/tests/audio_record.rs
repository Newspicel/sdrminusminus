#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_recording::RecordingDriver;
use sdrmm_device_virtual::{NFM_CARRIER_OFFSET_HZ, VirtualDriver};
use sdrmm_engine::Engine;
use sdrmm_recorder::read_audio_info;
use sdrmm_wire::{
    AdsbParams, AudioRecordingStatus, ChannelParams, ChannelSettings, DeviceSettings, NfmParams,
    WfmParams,
};

const TEST_RATE: f64 = 2_400_000.0;

fn engine(dir: &Path) -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    registry.register(10, Box::new(RecordingDriver::new(Some(dir.to_path_buf()))));
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
        frequency_hz: 100_000_000.0 + offset_hz,
        squelch: sdrmm_wire::Squelch::Off,
        params,
        blanker: Default::default(),
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
        .audio_recordings
        .first()
        .cloned()
}

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

#[tokio::test(flavor = "multi_thread")]
async fn a_recording_through_audio_fx_holds_the_processed_audio() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);
    engine
        .set_audio_fx(
            "fx",
            sdrmm_wire::AudioProcessing {
                agc: sdrmm_wire::AudioAgcMode::Fast,
                ..Default::default()
            },
        )
        .unwrap();
    let route = sdrmm_wire::AudioRoute {
        fx: vec!["fx".to_owned()],
        ..sdrmm_wire::AudioRoute::channel(ds, ch)
    };

    let started = engine.start_route_recording(&route).unwrap();
    assert!(engine.start_route_recording(&route).is_err());
    let live = wait_for_frames(&engine, ds, ch, 9_600).await;
    let stopping = Instant::now();
    let done = engine.stop_route_recording(&route).unwrap();
    assert!(stopping.elapsed() < Duration::from_secs(2), "stopping hung");
    assert_eq!(done.error, None);
    assert!(done.frames >= live.frames);

    let path = engine.audio_recordings_dir().unwrap().join(&started.file);
    assert_eq!(read_audio_info(&path).unwrap().frames, done.frames);
    assert!(rms(&wav_samples(&path)) > 100.0, "the file holds no audio");
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_channel_records_raw_and_through_audio_fx_at_once() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);
    engine
        .set_audio_fx("fx", sdrmm_wire::AudioProcessing::default())
        .unwrap();
    let processed = sdrmm_wire::AudioRoute {
        fx: vec!["fx".to_owned()],
        ..sdrmm_wire::AudioRoute::channel(ds, ch)
    };
    let raw = engine.start_channel_recording(ds, ch).unwrap();
    let cleaned = engine.start_route_recording(&processed).unwrap();
    assert_ne!(raw.file, cleaned.file);
    let statuses = engine
        .snapshot()
        .device_sets
        .into_iter()
        .find(|s| s.id == ds)
        .unwrap()
        .channels
        .into_iter()
        .find(|c| c.id == ch)
        .unwrap()
        .audio_recordings;
    let routes: Vec<Vec<String>> = statuses.into_iter().map(|s| s.fx).collect();
    assert_eq!(routes, vec![Vec::<String>::new(), vec!["fx".to_owned()]]);
    engine.stop_route_recording(&processed).unwrap();
    engine.stop_channel_recording(ds, ch).unwrap();
    assert!(live_status(&engine, ds, ch).is_none());
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn a_recording_through_an_unknown_audio_fx_is_refused_and_leaves_no_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let engine = engine(dir.path());
    let ds = set_at_test_rate(&engine);
    let ch = nfm_channel(&engine, ds);
    let route = sdrmm_wire::AudioRoute {
        fx: vec!["ghost".to_owned()],
        ..sdrmm_wire::AudioRoute::channel(ds, ch)
    };
    assert!(engine.start_route_recording(&route).is_err());
    assert!(live_status(&engine, ds, ch).is_none());
    let left = std::fs::read_dir(engine.audio_recordings_dir().unwrap())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "a refused recording left a file behind");
    engine.remove_device_set(ds).unwrap();
}

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
