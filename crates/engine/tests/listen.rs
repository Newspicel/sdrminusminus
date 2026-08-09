//! Engine e2e (PLAN §14): virtual siggen → DDC channel → demod → squelch → Opus, asserted
//! on the decoded audio. Real-time paced by the virtual device, so waits are generous.

// Tests may unwrap/expect (CLAUDE.md); clippy's `allow-unwrap-in-tests` only covers
// `#[cfg(test)]` items, which an integration-test crate's helpers are not.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use sdrmm_device::DeviceRegistry;
use sdrmm_device_virtual::{
    AM_CARRIER_OFFSET_HZ, MOD_TONE_HZ, NFM_CARRIER_OFFSET_HZ, VirtualDriver, WFM_CARRIER_OFFSET_HZ,
};
use sdrmm_engine::{AudioPacket, Engine, audio::OPUS_FRAME_SAMPLES};
use sdrmm_wire::{AmParams, ChannelParams, ChannelSettings, DeviceSettings, NfmParams, WfmParams};
use tokio::sync::broadcast;

const AUDIO_RATE: f64 = 48_000.0;
/// 2.4 Msps keeps the siggen's static tones (+360/+120/−720 kHz) clear of every modulated
/// carrier band (see `device-virtual`); the default 2.048 Msps parks a tone 7.2 kHz from
/// the NFM carrier, inside the channel DDC passband.
const TEST_RATE: f64 = 2_400_000.0;
/// Beyond the ±840 kHz drift-tone sweep at `TEST_RATE` and away from all tones/carriers.
const QUIET_OFFSET_HZ: f64 = -900_000.0;
const WAIT: Duration = Duration::from_secs(10);
/// One second of packets: bin-aligns 700/1000/1500/2300 Hz probes for leakage-free Goertzel.
const ONE_SECOND_PACKETS: usize = 50;
/// Half a second for DDC/demod/AGC transients (and squelch hold) to settle.
const SETTLE_PACKETS: usize = 25;

fn engine() -> Arc<Engine> {
    let mut registry = DeviceRegistry::new();
    registry.register(10, Box::new(VirtualDriver::new()));
    Engine::with_registry(registry)
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
        params,
    }
}

fn nfm(offset_hz: f64, squelch_db: Option<f32>) -> ChannelSettings {
    settings(
        ChannelParams::Nfm(NfmParams::default()),
        offset_hz,
        squelch_db,
    )
}

async fn collect_packets(rx: &mut broadcast::Receiver<AudioPacket>, n: usize) -> Vec<AudioPacket> {
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        match tokio::time::timeout(WAIT, rx.recv())
            .await
            .expect("audio packet within timeout")
        {
            Ok(packet) => out.push(packet),
            // Drop-oldest contract: a briefly starved test runner may lag; keep collecting.
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("audio stream closed"),
        }
    }
    out
}

fn decode(packets: &[AudioPacket]) -> Vec<f32> {
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Mono).expect("decoder");
    let mut out = Vec::with_capacity(packets.len() * OPUS_FRAME_SAMPLES);
    let mut frame = [0.0f32; OPUS_FRAME_SAMPLES];
    for packet in packets {
        let n = decoder
            .decode_float(&packet.opus, &mut frame, false)
            .expect("opus decode");
        out.extend_from_slice(&frame[..n]);
    }
    out
}

fn goertzel_power(samples: &[f32], freq_hz: f64) -> f64 {
    let w = std::f64::consts::TAU * freq_hz / AUDIO_RATE;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = f64::from(x) + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    coeff.mul_add(-(s1 * s2), s1 * s1 + s2 * s2)
}

fn assert_tone_dominates(audio: &[f32]) {
    let tone = goertzel_power(audio, MOD_TONE_HZ);
    let probes = [700.0, 1_500.0, 2_300.0].map(|f| goertzel_power(audio, f));
    let mean = probes.iter().sum::<f64>() / probes.len() as f64;
    assert!(
        tone > 10.0 * mean,
        "1 kHz tone does not dominate: tone {tone:.3e}, probe mean {mean:.3e} ({probes:?})"
    );
}

fn rms(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    (sum / samples.len().max(1) as f64).sqrt()
}

async fn settle_then_collect_second(rx: &mut broadcast::Receiver<AudioPacket>) -> Vec<f32> {
    collect_packets(rx, SETTLE_PACKETS).await;
    decode(&collect_packets(rx, ONE_SECOND_PACKETS).await)
}

#[tokio::test]
async fn nfm_channel_demodulates_the_test_carrier() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, nfm(NFM_CARRIER_OFFSET_HZ, None))
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
async fn wfm_channel_demodulates_the_test_carrier() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(
            ds,
            settings(
                ChannelParams::Wfm(WfmParams::default()),
                WFM_CARRIER_OFFSET_HZ,
                None,
            ),
        )
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn squelch_gates_empty_spectrum_and_patch_reopens() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, nfm(QUIET_OFFSET_HZ, Some(-20.0)))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();

    // Closed gate: PCM keeps flowing (jitter-buffer contract) but carries silence.
    let silence = settle_then_collect_second(&mut rx).await;
    let level = rms(&silence);
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
    let ch = engine.add_channel(ds, nfm(QUIET_OFFSET_HZ, None)).unwrap();
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
        .add_channel(ds, nfm(AM_CARRIER_OFFSET_HZ, None))
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
    // Same receiver across the pipeline swap — the audio stream must survive it.
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);
    engine.remove_device_set(ds).unwrap();
}

#[tokio::test]
async fn device_rate_change_rebuilds_channels_and_keeps_audio() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, nfm(NFM_CARRIER_OFFSET_HZ, None))
        .unwrap();
    let mut rx = engine.subscribe_audio(ds, ch).unwrap();
    assert_tone_dominates(&settle_then_collect_second(&mut rx).await);

    // Carrier offsets are center-relative, so the channel must keep demodulating after the
    // rate change — through the same subscription.
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

/// Timestamps derive from DSP-side sample stamps: on an intact stream they advance by
/// exactly one frame per packet (asserted here); PCM lost to encoder lag surfaces as a
/// timestamp jump with seq still contiguous (covered by the `audio` unit tests — forcing a
/// real lag deterministically needs direct access to the PCM channel).
#[tokio::test]
async fn audio_packets_are_contiguous_and_timestamped() {
    let engine = engine();
    let ds = set_at_test_rate(&engine);
    let ch = engine
        .add_channel(ds, nfm(NFM_CARRIER_OFFSET_HZ, None))
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
