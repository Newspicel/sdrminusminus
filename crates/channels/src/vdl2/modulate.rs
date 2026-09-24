use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::avlc;
use super::demod::{GRAY_FWD, SYMBOL_RATE, UW_DELTAS};
use super::header;
use super::interleave;
use super::scramble::Scrambler;

const RAMP_SYMBOLS: usize = 5;
const ROLLOFF: f64 = 0.6;
const PULSE_SPAN: f64 = 6.0;

fn burst_phases(frames: &[Vec<u8>]) -> Vec<f64> {
    let rs = interleave::vdl2_rs();
    let avlc_bits = avlc::build(frames);
    let tl = u32::try_from(avlc_bits.len()).unwrap();
    let mut bits: Vec<u8> = header::encode(tl).to_vec();
    bits.extend(interleave::interleave(&avlc_bits, &rs).unwrap());
    Scrambler::new().apply(&mut bits);
    while !bits.len().is_multiple_of(3) {
        bits.push(0);
    }
    let mut deltas: Vec<u8> = vec![0; RAMP_SYMBOLS];
    deltas.extend(UW_DELTAS);
    for t in bits.as_chunks::<3>().0 {
        deltas.push(GRAY_FWD[usize::from(t[0] | (t[1] << 1) | (t[2] << 2))]);
    }
    let mut ph = 0.0f64;
    deltas
        .iter()
        .map(|&d| {
            ph += f64::from(d) * PI / 4.0;
            ph
        })
        .collect()
}

pub fn burst_iq(
    frames: &[Vec<u8>],
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let phases = burst_phases(frames);
    let sps = sample_rate / SYMBOL_RATE;
    let nsamples = (phases.len() as f64 * sps).ceil() as usize;
    (0..nsamples)
        .map(|n| {
            let sym = ((n as f64 / sps) as usize).min(phases.len() - 1);
            let p = phases[sym] + TAU * freq_offset_hz * n as f64 / sample_rate;
            Complex::new(p.cos() as f32, p.sin() as f32) * amplitude
        })
        .collect()
}

fn raised_cosine(t: f64) -> f64 {
    let denom = 1.0 - (2.0 * ROLLOFF * t) * (2.0 * ROLLOFF * t);
    let sinc = if t.abs() < 1e-12 {
        1.0
    } else {
        (PI * t).sin() / (PI * t)
    };
    if denom.abs() < 1e-9 {
        ROLLOFF / 2.0 * (PI / (2.0 * ROLLOFF)).sin()
    } else {
        sinc * (PI * ROLLOFF * t).cos() / denom
    }
}

pub fn burst_iq_shaped(
    frames: &[Vec<u8>],
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let phases = burst_phases(frames);
    let sps = sample_rate / SYMBOL_RATE;
    let nsamples = ((phases.len() as f64 + 2.0 * PULSE_SPAN) * sps).ceil() as usize;
    (0..nsamples)
        .map(|n| {
            let t_sym = n as f64 / sps - PULSE_SPAN;
            let lo = (t_sym - PULSE_SPAN).ceil().max(0.0) as usize;
            let hi = (t_sym + PULSE_SPAN).floor().min(phases.len() as f64 - 1.0) as usize;
            let mut acc = Complex::new(0.0f64, 0.0);
            for (k, &phk) in phases.iter().enumerate().take(hi + 1).skip(lo) {
                acc += Complex::new(phk.cos(), phk.sin()) * raised_cosine(t_sym - k as f64);
            }
            let rot = TAU * freq_offset_hz * n as f64 / sample_rate;
            let v = acc * Complex::new(rot.cos(), rot.sin());
            Complex::new(v.re as f32, v.im as f32) * amplitude
        })
        .collect()
}
