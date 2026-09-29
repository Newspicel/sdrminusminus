use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_modem::pulse::{self, Norm};

use crate::dect::{
    burst::{DEVIATION_HZ, FRAME_SAMPLES, INPUT_RATE_HZ, PP_SYNC, RFP_SYNC, SLOTS_PER_FRAME, SPS},
    mac::append_r_crc,
};

const S_FIELD_BITS: usize = 32;
const A_FIELD_BITS: usize = 64;
const B_FIELD_BITS: usize = 324;
const GAUSSIAN_BT: f64 = 0.5;
const GAUSSIAN_SPAN: usize = 3;

pub const SLOT_SAMPLES: u64 = FRAME_SAMPLES / SLOTS_PER_FRAME;

#[derive(Clone, Copy, Debug)]
pub struct Station {
    pub rfpi: u64,
    pub slot: u8,
    pub carrier: u8,
    pub slot_pair: u8,
    pub rf_carriers: u16,
    pub transceivers: u8,
    pub pscn: u8,
    pub capabilities: u64,
}

impl Default for Station {
    fn default() -> Self {
        Self {
            rfpi: 0x0001_2345_6780,
            slot: 2,
            carrier: 3,
            slot_pair: 2,
            rf_carriers: 0x3FF,
            transceivers: 0,
            pscn: 5,
            capabilities: 0,
        }
    }
}

#[must_use]
pub fn header(ta: u8, ba: u8) -> u8 {
    ((ta & 0x07) << 5) | ((ba & 0x07) << 1)
}

#[must_use]
pub fn a_field(header: u8, tail: u64) -> u64 {
    append_r_crc((u64::from(header) << 56) | ((tail & ((1u64 << 40) - 1)) << 16))
}

#[must_use]
pub fn nt(rfpi: u64) -> u64 {
    a_field(header(3, 7), rfpi)
}

#[must_use]
pub fn qt_static(station: &Station) -> u64 {
    let tail = (u64::from(station.slot_pair & 0x0F) << 32)
        | (u64::from(station.transceivers & 0x03) << 27)
        | (u64::from(station.rf_carriers & 0x3FF) << 16)
        | (u64::from(station.carrier & 0x3F) << 8)
        | u64::from(station.pscn & 0x3F);
    a_field(header(4, 7), tail)
}

#[must_use]
pub fn qt_capabilities(capabilities: u64) -> u64 {
    a_field(
        header(4, 7),
        (3u64 << 36) | (capabilities & ((1u64 << 36) - 1)),
    )
}

#[must_use]
pub fn capability_bits(offsets: &[usize]) -> u64 {
    offsets
        .iter()
        .fold(0u64, |acc, &offset| acc | (1u64 << (47 - offset)))
}

#[must_use]
pub fn mt_encryption(command: u8, phase: u8, fmid: u16, pmid: u32) -> u64 {
    let tail = (5u64 << 36)
        | (u64::from(command & 0x03) << 34)
        | (u64::from(phase & 0x03) << 32)
        | (u64::from(fmid & 0x0FFF) << 20)
        | u64::from(pmid & 0x000F_FFFF);
    a_field(header(6, 7), tail)
}

fn packet_bits(from_rfp: bool, a_field: u64) -> Vec<bool> {
    let sync = if from_rfp { RFP_SYNC } else { PP_SYNC };
    let mut bits = Vec::with_capacity(S_FIELD_BITS + A_FIELD_BITS + B_FIELD_BITS);
    for index in 0..S_FIELD_BITS {
        bits.push((sync >> (S_FIELD_BITS - 1 - index)) & 1 == 1);
    }
    for index in 0..A_FIELD_BITS {
        bits.push((a_field >> (A_FIELD_BITS - 1 - index)) & 1 == 1);
    }
    let mut state = 0x5A5Au32;
    for _ in 0..B_FIELD_BITS {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bits.push(state >> 31 == 1);
    }
    bits
}

#[must_use]
pub fn modulate(from_rfp: bool, a_field: u64) -> Vec<Complex<f32>> {
    let bits = packet_bits(from_rfp, a_field);
    let shape = pulse::gaussian_freq(SPS as f64, GAUSSIAN_BT, GAUSSIAN_SPAN, Norm::Area);
    let mut freq = vec![0.0f32; bits.len() * SPS + shape.len()];
    for (index, &bit) in bits.iter().enumerate() {
        let level = if bit { 1.0 } else { -1.0 };
        for (tap, &h) in shape.iter().enumerate() {
            freq[index * SPS + tap] += level * h;
        }
    }
    let gain = TAU * DEVIATION_HZ * SPS as f64 / INPUT_RATE_HZ;
    let mut phase = 0.0f64;
    freq.iter()
        .map(|&value| {
            phase += gain * f64::from(value);
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

fn place(canvas: &mut [Complex<f32>], at: usize, burst: &[Complex<f32>]) {
    let end = (at + burst.len()).min(canvas.len());
    if at >= end {
        return;
    }
    canvas[at..end].copy_from_slice(&burst[..end - at]);
}

#[must_use]
pub fn dummy_bearer(station: &Station, frames: usize) -> Vec<Complex<f32>> {
    let mut canvas = vec![Complex::default(); frames * FRAME_SAMPLES as usize];
    let lead = station.slot as u64 * SLOT_SAMPLES;
    for frame in 0..frames {
        let a_field = match frame % 3 {
            0 => nt(station.rfpi),
            1 => qt_static(station),
            _ => qt_capabilities(station.capabilities),
        };
        let at = (frame as u64 * FRAME_SAMPLES + lead) as usize;
        place(&mut canvas, at, &modulate(true, a_field));
    }
    canvas
}

#[must_use]
pub fn with_burst(
    station: &Station,
    frames: usize,
    at_frame: usize,
    from_rfp: bool,
    a_field: u64,
) -> Vec<Complex<f32>> {
    let mut canvas = dummy_bearer(station, frames);
    let lead = station.slot as u64 * SLOT_SAMPLES;
    let at = (at_frame as u64 * FRAME_SAMPLES + lead) as usize;
    place(&mut canvas, at, &modulate(from_rfp, a_field));
    canvas
}
