#![allow(dead_code)]

use std::time::Duration;

use sdrmm_device_virtual::MOD_TONE_HZ;
use sdrmm_engine::{AudioPacket, audio::OPUS_FRAME_SAMPLES};
use tokio::sync::broadcast;

const AUDIO_RATE: f64 = 48_000.0;
const WAIT: Duration = Duration::from_secs(10);
const ONE_SECOND_PACKETS: usize = 50;
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
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => panic!("audio stream closed"),
        }
    }
    out
}

fn decode(packets: &[AudioPacket]) -> Vec<Vec<f32>> {
    let channels = usize::from(packets.first().map_or(1, |p| p.channels));
    assert!(
        packets.iter().all(|p| usize::from(p.channels) == channels),
        "channel layout changed inside one collection"
    );
    let layout = if channels == 2 {
        opus::Channels::Stereo
    } else {
        opus::Channels::Mono
    };
    let mut decoder = opus::Decoder::new(48_000, layout).expect("decoder");
    let mut out = vec![Vec::with_capacity(packets.len() * OPUS_FRAME_SAMPLES); channels];
    let mut frame = vec![0.0f32; OPUS_FRAME_SAMPLES * channels];
    for packet in packets {
        let frames = decoder
            .decode_float(&packet.opus, &mut frame, false)
            .expect("opus decode");
        for (i, sample) in frame[..frames * channels].iter().enumerate() {
            out[i % channels].push(*sample);
        }
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

pub fn assert_tone_dominates(channels: &[Vec<f32>]) {
    assert!(!channels.is_empty(), "no audio channels decoded");
    for (index, audio) in channels.iter().enumerate() {
        let tone = goertzel_power(audio, MOD_TONE_HZ);
        let probes = [700.0, 1_500.0, 2_300.0].map(|f| goertzel_power(audio, f));
        let mean = probes.iter().sum::<f64>() / probes.len() as f64;
        assert!(
            tone > 10.0 * mean,
            "channel {index}: 1 kHz tone does not dominate: tone {tone:.3e}, \
             probe mean {mean:.3e} ({probes:?})"
        );
    }
}

pub async fn settle_then_collect_second(
    rx: &mut broadcast::Receiver<AudioPacket>,
) -> Vec<Vec<f32>> {
    collect_packets(rx, SETTLE_PACKETS).await;
    decode(&collect_packets(rx, ONE_SECOND_PACKETS).await)
}
