//! System Fusion reference transmitter: sync, coded FICH, and all three voice payload layouts.

use blip25_vocoder::fullrate;
use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, crc16_msb, fec::conv};

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const SYNC: u64 = 0x00D4_71C9_634D;

const VFR_INTERLEAVE: [usize; 144] = [
    0, 24, 48, 72, 96, 120, 25, 1, 73, 49, 121, 97, 2, 26, 50, 74, 98, 122, 27, 3, 75, 51, 123, 99,
    4, 28, 52, 76, 100, 124, 29, 5, 77, 53, 125, 101, 6, 30, 54, 78, 102, 126, 31, 7, 79, 55, 127,
    103, 8, 32, 56, 80, 104, 128, 33, 9, 81, 57, 129, 105, 10, 34, 58, 82, 106, 130, 35, 11, 83,
    59, 131, 107, 12, 36, 60, 84, 108, 132, 37, 13, 85, 61, 133, 109, 14, 38, 62, 86, 110, 134, 39,
    15, 87, 63, 135, 111, 16, 40, 64, 88, 112, 136, 41, 17, 89, 65, 137, 113, 18, 42, 66, 90, 114,
    138, 43, 19, 91, 67, 139, 115, 20, 44, 68, 92, 116, 140, 45, 21, 93, 69, 141, 117, 22, 46, 70,
    94, 118, 142, 47, 23, 95, 71, 143, 119,
];

/// Frame information channel fields, as the FICH publishes them.
pub struct Fich {
    /// 0 header, 1 communications, 2 terminator.
    pub frame_type: u8,
    /// 0 V/D mode 1, 1 data FR, 2 V/D mode 2, 3 voice FR.
    pub data_mode: u8,
    pub dg_id: u8,
}

impl Default for Fich {
    fn default() -> Self {
        Self {
            frame_type: 0,
            data_mode: 2,
            dg_id: 7,
        }
    }
}

/// A whole transmission: a header frame, three communication frames and a terminator, each 100
/// ms long, with the lead-in a receiver's clock needs.
#[must_use]
pub fn transmission(fich: &Fich, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(fich, None, rate)
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum Voice<'a> {
    Vd1(&'a [[bool; 72]]),
    Vd2(&'a [[bool; 49]]),
    FullRate(&'a [[bool; 144]]),
}

/// A transmission whose communication frames carry caller-supplied vocoder frames in their
/// natural codec order. Voice-FR also uses two frames in the opening header.
#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    fich: &Fich,
    voice: Voice<'_>,
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(fich, Some(voice), rate)
}

fn transmission_inner(fich: &Fich, voice: Option<Voice<'_>>, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 13));
    let mut voice_at = 0;
    for frame_type in [fich.frame_type, 1, 1, 1, 2] {
        let frame_fich = Fich {
            frame_type,
            ..*fich
        };
        let (payload, consumed) = payload(&frame_fich, voice, voice_at);
        voice_at += consumed;
        symbols.extend(frame(&frame_fich, &payload));
    }
    symbols.extend(dibits(&filler(200, 17)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// One 480-symbol frame.
fn frame(fich: &Fich, payload: &[u8]) -> Vec<u8> {
    let mut out = dibits(&bits(SYNC, 40));
    out.extend(dibits(&fich_bits(fich)));
    out.extend_from_slice(payload);
    assert_eq!(out.len(), 480);
    out
}

fn payload(fich: &Fich, voice: Option<Voice<'_>>, at: usize) -> (Vec<u8>, usize) {
    let needed = match (fich.data_mode, fich.frame_type) {
        (0 | 2, 1) => 5,
        (3, 0) => 2,
        (3, 1) => 5,
        _ => 0,
    };
    let Some(voice) = voice else {
        return (dibits(&filler(720, 23 + u32::from(fich.frame_type))), 0);
    };
    if needed == 0 {
        return (dibits(&filler(720, 29 + u32::from(fich.frame_type))), 0);
    }
    let mut out = Vec::with_capacity(360);
    match voice {
        Voice::Vd1(frames) => {
            assert_eq!(fich.data_mode, 0);
            for (block, frame) in frames[at..at + needed].iter().enumerate() {
                out.extend(dibits(&filler(72, 31 + block as u32)));
                out.extend(dibits(frame));
            }
        }
        Voice::Vd2(frames) => {
            assert_eq!(fich.data_mode, 2);
            for (block, frame) in frames[at..at + needed].iter().enumerate() {
                out.extend(dibits(&filler(40, 37 + block as u32)));
                out.extend(dibits(&vd2_voice(frame)));
            }
        }
        Voice::FullRate(frames) => {
            assert_eq!(fich.data_mode, 3);
            if fich.frame_type == 0 {
                out.extend(dibits(&filler(432, 43)));
            }
            for frame in &frames[at..at + needed] {
                out.extend(dibits(&vfr_voice(frame)));
            }
        }
    }
    assert_eq!(out.len(), 360);
    (out, needed)
}

fn vd2_voice(info: &[bool; 49]) -> [bool; 104] {
    let mut protected = [false; 104];
    for i in 0..27 {
        protected[i * 3..i * 3 + 3].fill(info[i]);
    }
    protected[81..103].copy_from_slice(&info[27..]);
    let mut pn = [false; 104];
    let mut register = 0x1C9u16;
    for bit in &mut pn {
        *bit = register & 1 != 0;
        let feedback = (register ^ (register >> 4)) & 1;
        register = register >> 1 | feedback << 8;
    }
    std::array::from_fn(|i| {
        let source = (i % 4) * 26 + i / 4;
        protected[source] ^ pn[source]
    })
}

fn vfr_voice(annex_h: &[bool; 144]) -> [bool; 144] {
    let dibits: [u8; 72] =
        std::array::from_fn(|i| u8::from(annex_h[i * 2]) << 1 | u8::from(annex_h[i * 2 + 1]));
    let code = fullrate::fec::deinterleave(&dibits);
    let widths = [23usize, 23, 23, 23, 15, 15, 15, 7];
    let mut raw = [false; 144];
    let mut offset = 0;
    for (&word, width) in code.iter().zip(widths) {
        for bit in 0..width {
            raw[offset + bit] = word >> (width - 1 - bit) & 1 == 1;
        }
        offset += width;
    }
    let seed = raw[..12]
        .iter()
        .fold(0u16, |acc, &bit| acc << 1 | u16::from(bit));
    let mut state = seed << 4;
    for bit in &mut raw[23..137] {
        state = state.wrapping_mul(173).wrapping_add(13_849);
        *bit ^= state >> 15 != 0;
    }
    std::array::from_fn(|i| raw[VFR_INTERLEAVE[i]])
}

/// The 100 coded symbols of a FICH: CRC, four Golay blocks, the convolutional code and the
/// interleaver, in the order a transmitter applies them.
fn fich_bits(fich: &Fich) -> Vec<bool> {
    let mut info = [0u8; 6];
    info[0] = fich.frame_type << 6;
    info[2] = fich.data_mode & 0x03;
    info[3] = fich.dg_id & 0x7F;
    let crc = !crc16_msb(0x1021, 0, &info[..4]);
    info[4] = (crc >> 8) as u8;
    info[5] = crc as u8;

    let value = info
        .iter()
        .fold(0u64, |acc, &byte| acc << 8 | u64::from(byte));
    let mut coded = Vec::with_capacity(96);
    for block in 0..4 {
        let word = CyclicCode::GOLAY_24_12.encode((value >> (36 - block * 12)) as u32 & 0x0FFF);
        coded.extend(bits(word, 24));
    }
    // Four flush bits return the encoder to its zero state, which is what lets the decoder
    // keep 96 information bits out of 100 steps.
    coded.extend([false; 4]);
    let mut convolved = Vec::with_capacity(200);
    conv::encode(&coded, &mut convolved);

    let mut interleaved = vec![false; 200];
    for i in 0..100 {
        let n = 2 * (i / 5) + 40 * (i % 5);
        interleaved[n] = convolved[i * 2];
        interleaved[n + 1] = convolved[i * 2 + 1];
    }
    interleaved
}
