#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    AtvModulation, AtvParams, AtvStandard, ChannelParams, ChannelSettings, NfmParams, SstvMode,
    SstvParams,
};
use tempfile::TempDir;

const VIDEO_TIMEOUT: Duration = Duration::from_secs(30);
const DEVICE_RATE: f64 = 2_400_000.0;
const CENTER_HZ: f64 = 434_250_000.0;
const OFFSET_HZ: f64 = 200_000.0;

fn atv_params() -> AtvParams {
    AtvParams {
        modulation: AtvModulation::Am,
        standard: AtvStandard::Ccir625,
        ..AtvParams::default()
    }
}

#[tokio::test]
async fn an_atv_transmission_reaches_the_video_stream_as_a_picture() {
    let dir = TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.path().to_path_buf())),
    );
    let engine = Arc::new(Engine::with_registry(
        registry,
        Some(dir.path().to_path_buf()),
    ));

    let params = atv_params();
    let source = sdrmm_channels::testgen::atv::AtvSource::new(&params, DEVICE_RATE);
    let mut iq = sdrmm_channels::testgen::atv::bars(&source, 8);
    sdrmm_channels::testgen::shift(&mut iq, OFFSET_HZ, DEVICE_RATE);

    let path = dir.path().join("atv");
    let mut writer = SigmfWriter::create(&path, DEVICE_RATE, CENTER_HZ, "atv fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    let device = format!("virtual:file:{}", path.display());

    let ds = engine.create_device_set(&device).unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: OFFSET_HZ,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Atv(params),
                audio: Default::default(),
            },
        )
        .unwrap();
    let mut rx = engine.subscribe_video(ds, ch).unwrap();

    let packet = tokio::time::timeout(VIDEO_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Ok(packet) => return packet,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("video stream closed")
                }
            }
        }
    })
    .await;
    engine.remove_device_set(ds).unwrap();
    let packet = packet.expect("a picture within the timeout");

    let picture = &packet.picture;
    assert_eq!(picture.height, 576, "CCIR 625 scans out 576 active lines");
    assert_eq!(
        picture.luma.len(),
        usize::from(picture.width) * usize::from(picture.height),
        "the payload must be exactly the geometry it claims"
    );
    assert!(
        packet.timestamp > 0,
        "the picture must be stamped with the channel's own sample clock"
    );

    let width = usize::from(picture.width);
    let row = 300 * width;
    let mean = |from: usize, to: usize| {
        let span = &picture.luma[row + from..row + to];
        span.iter().map(|&v| u32::from(v)).sum::<u32>() / span.len() as u32
    };
    let black = mean(width / 20, width / 8);
    let white = mean(width * 7 / 8, width * 19 / 20);
    assert!(black < 60, "left bar should be black, got {black}");
    assert!(white > 190, "right bar should be white, got {white}");
}

#[tokio::test]
async fn a_channel_without_video_refuses_the_subscription() {
    let dir = TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_recordings(dir.path().to_path_buf())),
    );
    let engine = Arc::new(Engine::with_registry(
        registry,
        Some(dir.path().to_path_buf()),
    ));

    let ds = engine.create_device_set("virtual:siggen").unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let refused = engine.subscribe_video(ds, ch).unwrap_err();
    assert!(refused.is_bad_request(), "{refused}");
    assert!(
        engine
            .subscribe_video(ds, ch + 1)
            .unwrap_err()
            .is_not_found(),
        "an unknown channel is a 404, not a bad request"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn an_sstv_transmission_reaches_the_image_stream_as_a_finished_picture() {
    const SSTV_DEVICE_RATE: f64 = 48_000.0;
    const SSTV_OFFSET_HZ: f64 = 4_000.0;
    let mode = SstvMode::Robot36;

    let dir = TempDir::new().unwrap();
    let mut registry = DeviceRegistry::new();
    registry.register(
        10,
        Box::new(VirtualDriver::with_accelerated_recordings(
            dir.path().to_path_buf(),
            20.0,
        )),
    );
    let engine = Arc::new(Engine::with_registry(
        registry,
        Some(dir.path().to_path_buf()),
    ));

    let sent = sdrmm_channels::testgen::sstv::bars(mode);
    let native = sdrmm_channels::testgen::sstv::transmission(mode, &sent, 16_000.0);
    let mut iq = sdrmm_channels::testgen::resample(&native, 16_000.0, SSTV_DEVICE_RATE);
    iq.extend(sdrmm_channels::testgen::silence(
        SSTV_DEVICE_RATE as usize * 3,
    ));
    sdrmm_channels::testgen::shift(&mut iq, SSTV_OFFSET_HZ, SSTV_DEVICE_RATE);

    let path = dir.path().join("sstv");
    let mut writer =
        SigmfWriter::create(&path, SSTV_DEVICE_RATE, CENTER_HZ, "sstv fixture").unwrap();
    writer.write_block(&iq).unwrap();
    writer.finalize().unwrap();
    let device = format!("virtual:file:{}", path.display());

    let mut images = engine.subscribe_images();
    let ds = engine.create_device_set(&device).unwrap();
    let ch = engine
        .add_channel(
            ds,
            0,
            ChannelSettings {
                offset_hz: SSTV_OFFSET_HZ,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Sstv(SstvParams::default()),
                audio: Default::default(),
            },
        )
        .unwrap();
    let mut video = engine.subscribe_video(ds, ch).unwrap();

    let captured = tokio::time::timeout(VIDEO_TIMEOUT, async {
        loop {
            match images.recv().await {
                Ok(capture) if capture.complete => return capture,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("image stream closed")
                }
            }
        }
    })
    .await
    .expect("a finished picture within the timeout");

    assert!(
        matches!(
            video.try_recv(),
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
        ),
        "the picture must also build up on the live video stream"
    );
    engine.remove_device_set(ds).unwrap();

    assert_eq!(captured.device_set, ds);
    assert_eq!(captured.channel, ch);
    assert_eq!(captured.freq_hz, CENTER_HZ + SSTV_OFFSET_HZ);
    assert_eq!(captured.source, "sstv");
    assert_eq!(captured.mode, "Robot 36");
    assert_eq!(captured.lines, 240);
    let picture = &captured.picture;
    assert_eq!((picture.width, picture.height), mode.size());
    assert_eq!(
        picture.rgb.len(),
        usize::from(picture.width) * usize::from(picture.height) * 3
    );

    let width = usize::from(picture.width);
    let row = 120 * width;
    let bar = |index: usize| {
        let x = row + width * (8 * index + 4) / 64;
        [
            picture.rgb[x * 3],
            picture.rgb[x * 3 + 1],
            picture.rgb[x * 3 + 2],
        ]
    };
    let white = bar(0);
    let black = bar(7);
    assert!(
        white.iter().all(|&v| v > 170),
        "the first bar should be white, got {white:?}"
    );
    assert!(
        black.iter().all(|&v| v < 85),
        "the last bar should be black, got {black:?}"
    );
}
