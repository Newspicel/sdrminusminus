//! Helpers shared by the engine e2e tests: packet collection, Opus decode, and the Goertzel
//! tone-dominance assertion. The virtual devices pace themselves to real time, hence the
//! generous waits.

use std::time::Duration;

use sdrmm_device_virtual::MOD_TONE_HZ;
use sdrmm_engine::{AudioPacket, audio::OPUS_FRAME_SAMPLES};
use tokio::sync::broadcast;

const AUDIO_RATE: f64 = 48_000.0;
const WAIT: Duration = Duration::from_secs(10);
/// One second of packets: bin-aligns 700/1000/1500/2300 Hz probes for leakage-free Goertzel.
const ONE_SECOND_PACKETS: usize = 50;
/// Half a second for DDC/demod/AGC transients (and squelch hold) to settle.
pub const SETTLE_PACKETS: usize = 25;

pub async fn collect_packets(
    rx: &mut broadcast::Receiver<AudioPacket>,
    n: usize,
) -> Vec<AudioPacket> {
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

pub fn assert_tone_dominates(audio: &[f32]) {
    let tone = goertzel_power(audio, MOD_TONE_HZ);
    let probes = [700.0, 1_500.0, 2_300.0].map(|f| goertzel_power(audio, f));
    let mean = probes.iter().sum::<f64>() / probes.len() as f64;
    assert!(
        tone > 10.0 * mean,
        "1 kHz tone does not dominate: tone {tone:.3e}, probe mean {mean:.3e} ({probes:?})"
    );
}

pub async fn settle_then_collect_second(rx: &mut broadcast::Receiver<AudioPacket>) -> Vec<f32> {
    collect_packets(rx, SETTLE_PACKETS).await;
    decode(&collect_packets(rx, ONE_SECOND_PACKETS).await)
}
