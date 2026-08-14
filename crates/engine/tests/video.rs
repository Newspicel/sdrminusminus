#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::VirtualDriver;
use sdrmm_engine::Engine;
use sdrmm_recorder::SigmfWriter;
use sdrmm_wire::{
    AtvModulation, AtvParams, AtvStandard, ChannelParams, ChannelSettings, NfmParams,
};
use tempfile::TempDir;

/// Playback is real-time paced, and the receiver has to hunt sync before it scans anything out;
/// the timeout covers a full loop pass plus scheduler slack on a loaded CI box.
const VIDEO_TIMEOUT: Duration = Duration::from_secs(30);
/// Deliberately not the mode's 2 Msps channel rate: the DDC has to decimate for this to prove
/// anything about the plumbing.
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
                params: ChannelParams::Atv(params),
            },
        )
        .unwrap();
    let mut rx = engine.subscribe_video(ds, ch).unwrap();

    let packet = tokio::time::timeout(VIDEO_TIMEOUT, async {
        loop {
            match rx.recv().await {
                Ok(packet) => return packet,
                // Drop-oldest is the contract for this stream; a starved runner may lag.
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

/// A mode that scans out nothing must refuse the subscription rather than open a stream that
/// would stay silent — a panel waiting on it is indistinguishable from a dead receiver.
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
                params: ChannelParams::Nfm(NfmParams::default()),
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
