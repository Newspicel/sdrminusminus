use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::fft::Transform;
use sdrmm_wire::{ChannelParams, ChannelSettings, DecoderEvent};

use crate::{AUDIO_RATE, ChannelOutputs, ChannelRx};

pub(crate) fn settings(params: ChannelParams) -> ChannelSettings {
    ChannelSettings {
        frequency_hz: 0.0,
        squelch: sdrmm_wire::Squelch::Off,
        params,
        blanker: Default::default(),
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

pub(crate) fn fm_iq(rate: f64, f_mod: f64, deviation_hz: f64, len: usize) -> Vec<Complex<f32>> {
    let mut phase = 0.0f64;
    (0..len)
        .map(|k| {
            phase += TAU * deviation_hz * (TAU * f_mod * k as f64 / rate).cos() / rate;
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

pub(crate) fn am_iq(rate: f64, f_mod: f64, depth: f32, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|k| {
            let audio = (TAU * f_mod * k as f64 / rate).cos() as f32;
            Complex::new(1.0 + depth * audio, 0.0)
        })
        .collect()
}

pub(crate) fn component(iq: &[Complex<f32>], freq_hz: f64, rate: f64) -> f64 {
    let step = TAU * freq_hz / rate;
    let sum: Complex<f64> = iq
        .iter()
        .enumerate()
        .map(|(k, s)| {
            Complex::new(f64::from(s.re), f64::from(s.im))
                * Complex::from_polar(1.0, -step * k as f64)
        })
        .sum();
    sum.norm() / iq.len() as f64
}

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

pub(crate) fn run_events(chan: &mut dyn ChannelRx, iq: &[Complex<f32>]) -> Vec<DecoderEvent> {
    let mut out = ChannelOutputs::default();
    let mut events = Vec::new();
    let mut pos = 0;
    for len in [997usize, 1, 4_096, 65, 2_048, 7, 1_024].iter().cycle() {
        if pos >= iq.len() {
            break;
        }
        let end = (pos + len).min(iq.len());
        out.reset();
        chan.process(&iq[pos..end], &mut out);
        events.append(&mut out.events);
        pos = end;
    }
    events
}

pub(crate) fn split_stereo(pcm: &[f32]) -> (Vec<f32>, Vec<f32>) {
    assert_eq!(pcm.len() % 2, 0, "interleaved stereo needs whole frames");
    (
        pcm.iter().step_by(2).copied().collect(),
        pcm.iter().skip(1).step_by(2).copied().collect(),
    )
}

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

pub(crate) fn frequency_shift(iq: &[Complex<f32>], hz: f64, rate: f64) -> Vec<Complex<f32>> {
    iq.iter()
        .enumerate()
        .map(|(index, &value)| {
            let phase = TAU * hz * index as f64 / rate;
            value * Complex::new(phase.cos() as f32, phase.sin() as f32)
        })
        .collect()
}

pub(crate) fn sample_clock_offset(iq: &[Complex<f32>], ppm: f64) -> Vec<Complex<f32>> {
    const HALF: isize = 16;
    let step = 1.0 / (1.0 + ppm * 1e-6);
    let count = ((iq.len() as f64 - HALF as f64) / step) as usize;
    (0..count)
        .map(|index| {
            let position = index as f64 * step;
            let center = position.floor() as isize;
            (center - HALF + 1..=center + HALF)
                .filter_map(|tap| {
                    let sample = iq.get(usize::try_from(tap).ok()?)?;
                    let x = position - tap as f64;
                    let sinc = if x.abs() < 1e-12 {
                        1.0
                    } else {
                        (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
                    };
                    let window = 0.42
                        + 0.5 * (std::f64::consts::PI * x / HALF as f64).cos()
                        + 0.08 * (TAU * x / HALF as f64).cos();
                    Some(sample * (sinc * window) as f32)
                })
                .sum()
        })
        .collect()
}

pub(crate) fn multipath(
    iq: &[Complex<f32>],
    paths: &[(usize, f32, f64)],
    rate: f64,
) -> Vec<Complex<f32>> {
    (0..iq.len())
        .map(|index| {
            paths
                .iter()
                .filter(|&&(delay, _, _)| delay <= index)
                .map(|&(delay, gain, doppler_hz)| {
                    let phase = TAU * doppler_hz * index as f64 / rate;
                    iq[index - delay] * gain * Complex::new(phase.cos() as f32, phase.sin() as f32)
                })
                .sum()
        })
        .collect()
}

pub(crate) fn at_snr(signal: &[Complex<f32>], snr_db: f32, seed: u64) -> Vec<Complex<f32>> {
    let power = signal
        .iter()
        .map(num_complex::Complex::norm_sqr)
        .sum::<f32>()
        / signal.len() as f32;
    let sigma = (power / 10f32.powf(snr_db / 10.0) / 2.0).sqrt();
    let mut noisy = signal.to_vec();
    add_awgn(&mut noisy, sigma, seed);
    noisy
}

pub(crate) fn add_awgn(iq: &mut [Complex<f32>], sigma: f32, seed: u64) {
    let mut state = seed | 1;
    let mut gaussian = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let u = f64::from((state >> 40) as u32) / f64::from(1u32 << 24);
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let v = f64::from((state >> 40) as u32) / f64::from(1u32 << 24);
        ((-2.0 * u.max(1e-12).ln()).sqrt() * (TAU * v).cos()) as f32
    };
    for sample in iq {
        *sample += Complex::new(gaussian() * sigma, gaussian() * sigma);
    }
}

pub(crate) fn tone_amplitude(audio: &[f32], freq_hz: f64, rate: f64) -> f32 {
    let step = TAU * freq_hz / rate;
    let (mut re, mut im) = (0.0f64, 0.0f64);
    for (n, &s) in audio.iter().enumerate() {
        re += f64::from(s) * (step * n as f64).cos();
        im += f64::from(s) * (step * n as f64).sin();
    }
    (2.0 * re.hypot(im) / audio.len() as f64) as f32
}

pub(crate) fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

fn half_spectrum(audio: &[f32]) -> Vec<f64> {
    let n = audio.len();
    let mut buf: Vec<Complex<f32>> = audio.iter().map(|&v| Complex::new(v, 0.0)).collect();
    Transform::forward(n).process(&mut buf);
    buf[..=n / 2]
        .iter()
        .map(|v| f64::from(v.norm_sqr()))
        .collect()
}

fn bin_power(power: &[f64], bin: usize) -> f64 {
    let lo = bin.saturating_sub(3);
    let hi = (bin + 3).min(power.len() - 1);
    power[lo..=hi].iter().sum()
}

pub(crate) fn dominant_tone(audio: &[f32], rate: f64) -> (f64, f64) {
    let power = half_spectrum(audio);
    let peak = power
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap();
    let signal = bin_power(&power, peak);
    let rest = (power.iter().sum::<f64>() - signal).max(1e-30);
    (peak as f64 * rate / audio.len() as f64, signal / rest)
}

pub(crate) fn tone_power(audio: &[f32], freq_hz: f64, rate: f64) -> f64 {
    let power = half_spectrum(audio);
    let bin = (freq_hz * audio.len() as f64 / rate).round() as usize;
    bin_power(&power, bin) / power.iter().sum::<f64>().max(1e-30)
}

// Under `cargo xtask sanitize` the decoders run several times slower than the signal they
// decode, so the real-time gates measure the instrumentation instead of the code.
pub(crate) fn realtime_budget(seconds: f64) -> f64 {
    if cfg!(sanitized) {
        f64::INFINITY
    } else {
        seconds
    }
}
