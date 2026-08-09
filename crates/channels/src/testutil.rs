//! Analytic signal + spectrum helpers shared by the demodulator tests (compiled only for
//! tests). Mirrors `sdrmm-dsp`'s private test utilities, which this crate cannot import.

use std::f64::consts::TAU;

use num_complex::Complex;
use rustfft::FftPlanner;
use sdrmm_wire::{ChannelParams, ChannelSettings};

use crate::{AUDIO_RATE, ChannelOutputs, ChannelRx};

pub(crate) fn settings(params: ChannelParams) -> ChannelSettings {
    ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        params,
    }
}

pub(crate) fn complex_tone(freq_norm: f64, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            let p = TAU * freq_norm * n as f64;
            Complex::new(p.cos() as f32, p.sin() as f32)
        })
        .collect()
}

/// Phase-accumulated FM of an `f_mod` cosine at peak deviation `deviation_hz`: an ideal
/// discriminator scaled to that deviation returns a unit-amplitude cosine.
pub(crate) fn fm_iq(rate: f64, f_mod: f64, deviation_hz: f64, len: usize) -> Vec<Complex<f32>> {
    let mut phase = 0.0f64;
    (0..len)
        .map(|k| {
            phase += TAU * deviation_hz * (TAU * f_mod * k as f64 / rate).cos() / rate;
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

/// Feed `iq` through the channel in deliberately ragged blocks and concatenate the audio,
/// checking the advertised rate on every producing block.
pub(crate) fn run_ragged(chan: &mut dyn ChannelRx, iq: &[Complex<f32>]) -> Vec<f32> {
    let mut out = ChannelOutputs::default();
    let mut audio = Vec::new();
    let mut pos = 0;
    for len in [997usize, 1, 4_096, 65, 2_048, 7, 1_024].iter().cycle() {
        if pos >= iq.len() {
            break;
        }
        let end = (pos + len).min(iq.len());
        out.reset();
        chan.process(&iq[pos..end], &mut out);
        if !out.audio_pcm.is_empty() {
            assert_eq!(out.audio_rate, AUDIO_RATE);
        }
        audio.extend_from_slice(&out.audio_pcm);
        pos = end;
    }
    audio
}

/// Deterministic uniform complex noise in roughly `±amp` (xorshift32, like `sdrmm-dsp`'s).
pub(crate) fn complex_noise(seed: u32, amp: f32, len: usize) -> Vec<Complex<f32>> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
    };
    (0..len).map(|_| Complex::new(next(), next())).collect()
}

pub(crate) fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// Dominant tone of a real signal: `(peak frequency in Hz, peak±3-bin power over the rest of
/// the half spectrum)`. Callers keep the tone on a bin center so leakage stays in the guard.
pub(crate) fn dominant_tone(audio: &[f32], rate: f64) -> (f64, f64) {
    let n = audio.len();
    let mut buf: Vec<Complex<f32>> = audio.iter().map(|&v| Complex::new(v, 0.0)).collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let power: Vec<f64> = buf[..=n / 2]
        .iter()
        .map(|v| f64::from(v.norm_sqr()))
        .collect();
    let peak = power
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap();
    let lo = peak.saturating_sub(3);
    let hi = (peak + 3).min(n / 2);
    let signal: f64 = power[lo..=hi].iter().sum();
    let rest = (power.iter().sum::<f64>() - signal).max(1e-30);
    (peak as f64 * rate / n as f64, signal / rest)
}
