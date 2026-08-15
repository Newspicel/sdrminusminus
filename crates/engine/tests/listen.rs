#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;

use common::{SETTLE_PACKETS, assert_tone_dominates, collect_packets, settle_then_collect_second};
use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::{
    AM_CARRIER_OFFSET_HZ, NFM_CARRIER_OFFSET_HZ, VirtualDriver, WFM_CARRIER_OFFSET_HZ,
};
use sdrmm_engine::{Engine, audio::OPUS_FRAME_SAMPLES};
use sdrmm_wire::{AmParams, ChannelParams, ChannelSettings, DeviceSettings, NfmParams, WfmParams};

const TEST_RATE: f64 = 2_400_000.0;
const QUIET_OFFSET_HZ: f64 = -900_000.0;

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry, None)
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

fn settings(params: ChannelParams, offset_hz: f64, squelch_db: Option<f32>) -> ChannelSettings {
    ChannelSettings {
        offset_hz,
        squelch_db,
        squelch_auto_db: None,
        params,
        audio: Default::default(),
    }
}

fn nfm(offset_hz: f64, squelch_db: Option<f32>) -> ChannelSettings {
    settings(
        ChannelParams::Nfm(NfmParams::default()),
        offset_hz,
        squelch_db,
    )
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

#[tokio::test]
async fn nfm_channel_demodulates_the_test_carrier() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn am_channel_demodulates_the_test_carrier() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(
            ds,
            0,
            settings(
                ChannelParams::Am(AmParams::default()),
                AM_CARRIER_OFFSET_HZ,
                None,
            ),
        )
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn wfm_channel_demodulates_the_test_carrier_in_stereo() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(
            ds,
            0,
            settings(
                ChannelParams::Wfm(WfmParams::default()),
                WFM_CARRIER_OFFSET_HZ,
                None,
            ),
        )
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    let channels = settle_then_collect_second(&mut rx).await;
    assert_eq!(channels.len(), 2, "wfm defaults to a two-channel stream");
    assert_tone_dominates(&channels);
    let difference: Vec<f32> = channels[0]
        .iter()
        .zip(&channels[1])
        .map(|(l, r)| l - r)
        .collect();
    let (left, delta) = (rms(&channels[0]), rms(&difference));
    assert!(
        delta < 0.05 * left,
        "no pilot on air, yet the channels differ: rms {delta} against {left}"
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn wfm_stereo_toggle_changes_the_layout_of_the_live_stream() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let wfm = |stereo: bool| {
        settings(
            ChannelParams::Wfm(WfmParams {
                deemphasis_us: 50.0,
                stereo,
            }),
            WFM_CARRIER_OFFSET_HZ,
            None,
        )
    };
    let ch = engine.add_channel(ds, 0, wfm(true)).unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    let stereo = collect_packets(&mut rx, SETTLE_PACKETS).await;
    assert!(
        stereo.iter().all(|p| p.channels == 2),
        "stream did not start stereo"
    );

    engine.patch_channel(ds, ch, wfm(false)).unwrap();
    let mut waited = 0;
    while collect_packets(&mut rx, 1).await[0].channels != 1 {
        waited += 1;
        assert!(waited < SETTLE_PACKETS, "layout never followed the patch");
    }

    let mono = collect_packets(&mut rx, SETTLE_PACKETS).await;
    assert!(mono.iter().all(|p| p.channels == 1), "layout flapped back");
    for pair in mono.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1), "seq gap");
        assert_eq!(
            pair[1].timestamp,
            pair[0].timestamp + OPUS_FRAME_SAMPLES as u64,
            "the frame clock did not survive the layout change"
        );
    }
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn squelch_gates_empty_spectrum_and_patch_reopens() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(QUIET_OFFSET_HZ, Some(-20.0)))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();

    let silence = settle_then_collect_second(&mut rx).await;
    let level = rms(&silence[0]);
    assert!(level < 0.01, "squelched audio rms {level}");

    engine
        .patch_channel(ds, ch, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn patch_channel_offset_retunes_onto_the_carrier() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(QUIET_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    collect_packets(&mut rx, SETTLE_PACKETS).await;

    engine
        .patch_channel(ds, ch, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);

    let snapshot = engine.snapshot();
    assert_eq!(
        snapshot.device_sets[0].channels[0].settings.offset_hz,
        NFM_CARRIER_OFFSET_HZ
    );
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn type_change_keeps_the_audio_subscription_alive() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(AM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    collect_packets(&mut rx, SETTLE_PACKETS).await;

    engine
        .patch_channel(
            ds,
            ch,
            settings(
                ChannelParams::Am(AmParams::default()),
                AM_CARRIER_OFFSET_HZ,
                None,
            ),
        )
        .unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn device_rate_change_rebuilds_channels_and_keeps_audio() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);

    engine
        .patch_device(
            ds,
            DeviceSettings {
                sample_rate: Some(3_200_000.0),
                ..Default::default()
            },
        )
        .unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn audio_packets_are_contiguous_and_timestamped() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, 0, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();

    let packets = collect_packets(&mut rx, 30).await;
    for pair in packets.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1), "seq gap");
        assert_eq!(
            pair[1].timestamp,
            pair[0].timestamp + OPUS_FRAME_SAMPLES as u64,
            "timestamp gap"
        );
    }
    engine.remove_device_set(ds).unwrap();
}
