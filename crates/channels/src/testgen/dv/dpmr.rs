//! dPMR reference transmitter (ETSI TS 102 490): a header frame with both copies of the header
//! information and the colour code between them, then a superframe and an end frame.

use num_complex::Complex;

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 2_400.0;
const DEVIATION_HZ: f64 = 1_050.0;
const RRC_ALPHA: f64 = 0.2;

const FS1: u64 = 0x57FF_5F75_D577;
const FS2: u64 = 0x5F_F77D;
const FS3: u64 = 0x7D_DFF5;

/// One call as a dPMR radio keys it.
pub struct Call {
    pub colour_code: u16,
    pub called: u32,
    pub own: u32,
    /// Communication mode: 0 is an individual call, anything else a group.
    pub mode: u8,
}

impl Default for Call {
    fn default() -> Self {
        Self {
            colour_code: 0x0A5,
            called: 0x00_FFFF,
            own: 0x12_3456,
            mode: 1,
        }
    }
}

/// Preamble, header frame, two superframe halves and an end frame.
#[must_use]
pub fn transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    transmission_with_voice(call, &[[false; 72]; 32], rate)
}

/// Build a call with carrier-interleaved AMBE+2 frames. Every 16 frames form one dPMR
/// superframe; a partial final superframe is padded with quiet code words.
#[must_use]
pub fn transmission_with_voice(call: &Call, voice: &[[bool; 72]], rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 67));
    symbols.extend(dibits(&bits(FS1, 48)));
    symbols.extend(dibits(&header_info(call)));
    symbols.extend(dibits(&colour_code(call.colour_code)));
    symbols.extend(dibits(&header_info(call)));

    for frames in voice.chunks(16) {
        symbols.extend(dibits(&bits(FS2, 24)));
        symbols.extend(superframe(frames));
    }
    symbols.extend(dibits(&bits(FS3, 24)));
    symbols.extend(dibits(&filler(400, 73)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

fn superframe(frames: &[[bool; 72]]) -> Vec<u8> {
    let mut padded = [[false; 72]; 16];
    padded[..frames.len()].copy_from_slice(frames);
    let mut out = Vec::with_capacity(756);
    for section in 0..4 {
        out.extend(dibits(&filler(72, 71 + section as u32)));
        for frame in &padded[section * 4..section * 4 + 4] {
            out.extend(dibits(frame));
        }
        if section == 1 {
            out.extend(dibits(&bits(FS2, 24)));
        } else if section != 3 {
            out.extend(dibits(&colour_code(0)));
        }
    }
    debug_assert_eq!(out.len(), 756);
    out
}

fn colour_code(value: u16) -> Vec<bool> {
    let mut out = Vec::with_capacity(24);
    for i in (0..12).rev() {
        out.push(value >> i & 1 == 1);
        out.push(true);
    }
    out
}

/// The 72-bit header information through its CRC, Hamming blocks, interleaver and scrambler.
fn header_info(call: &Call) -> Vec<bool> {
    let mut info = bits(0, 4);
    info.extend(bits(u64::from(call.called), 24));
    info.extend(bits(u64::from(call.own), 24));
    info.extend(bits(u64::from(call.mode) & 0x07, 3));
    info.extend(bits(0, 4 + 2 + 11));

    let mut bytes: Vec<u8> = info
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)))
        .collect();
    bytes.push(crc8(&bytes));

    // Shortened Hamming(12,8): systematic, so the four parity bits sit after the byte. The
    // decoder reads only the information half, so the parity is generated but never relied on.
    let mut blocks = Vec::with_capacity(120);
    for &byte in &bytes {
        blocks.extend(bits(u64::from(byte), 8));
        blocks.extend(hamming_parity(byte));
    }
    // Interleave: ten 12-bit blocks written down the columns, read out along the rows.
    let mut interleaved = vec![false; 120];
    for r in 0..12 {
        for c in 0..10 {
            interleaved[r * 10 + c] = blocks[c * 12 + r];
        }
    }
    let mut register = 0x1FFu16;
    interleaved
        .into_iter()
        .map(|bit| {
            let feedback = (register >> 8 ^ register >> 4) & 1;
            register = (register << 1 | feedback) & 0x1FF;
            bit ^ (feedback == 1)
        })
        .collect()
}

fn hamming_parity(byte: u8) -> Vec<bool> {
    let bit = |i: u8| byte >> (7 - i) & 1 == 1;
    let sum = |taps: &[u8]| taps.iter().fold(false, |acc, &t| acc ^ bit(t));
    vec![
        sum(&[0, 1, 2, 3]),
        sum(&[1, 2, 3, 4]),
        sum(&[0, 1, 2, 4]),
        sum(&[0, 2, 3, 4]),
    ]
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                crc << 1 ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}
