use blip25_vocoder::fullrate;
use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, crc16_msb, fec::conv};

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const SYNC: u64 = 0x00D4_71C9_634D;
const WHITENING: [u8; 20] = [
    0x93, 0xD7, 0x51, 0x21, 0x9C, 0x2F, 0x6C, 0xD0, 0xEF, 0x0F, 0xF8, 0x3D, 0xF1, 0x73, 0x20, 0x94,
    0xED, 0x1E, 0x7C, 0xD8,
];

const VFR_INTERLEAVE: [usize; 144] = [
    0, 24, 48, 72, 96, 120, 25, 1, 73, 49, 121, 97, 2, 26, 50, 74, 98, 122, 27, 3, 75, 51, 123, 99,
    4, 28, 52, 76, 100, 124, 29, 5, 77, 53, 125, 101, 6, 30, 54, 78, 102, 126, 31, 7, 79, 55, 127,
    103, 8, 32, 56, 80, 104, 128, 33, 9, 81, 57, 129, 105, 10, 34, 58, 82, 106, 130, 35, 11, 83,
    59, 131, 107, 12, 36, 60, 84, 108, 132, 37, 13, 85, 61, 133, 109, 14, 38, 62, 86, 110, 134, 39,
    15, 87, 63, 135, 111, 16, 40, 64, 88, 112, 136, 41, 17, 89, 65, 137, 113, 18, 42, 66, 90, 114,
    138, 43, 19, 91, 67, 139, 115, 20, 44, 68, 92, 116, 140, 45, 21, 93, 69, 141, 117, 22, 46, 70,
    94, 118, 142, 47, 23, 95, 71, 143, 119,
];

pub struct Fich {
    pub frame_type: u8,
    pub frame_number: u8,
    pub frame_total: u8,
    pub data_mode: u8,
    pub dg_id: u8,
}

impl Default for Fich {
    fn default() -> Self {
        Self {
            frame_type: 0,
            frame_number: 0,
            frame_total: 5,
            data_mode: 2,
            dg_id: 7,
        }
    }
}

pub struct Call {
    pub destination: String,
    pub source: String,
    pub downlink: String,
    pub uplink: String,
}

impl Default for Call {
    fn default() -> Self {
        Self {
            destination: "ALL".to_owned(),
            source: "DL1ABC".to_owned(),
            downlink: "DB0ABC".to_owned(),
            uplink: "DB0XYZ".to_owned(),
        }
    }
}

#[must_use]
pub fn transmission(fich: &Fich, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(fich, None, None, false, rate)
}

#[must_use]
pub fn transmission_with_callsigns(fich: &Fich, call: &Call, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(fich, None, Some(call), true, rate)
}

#[must_use]
pub fn late_entry_transmission(fich: &Fich, call: &Call, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(fich, None, Some(call), false, rate)
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum Voice<'a> {
    Vd1(&'a [[bool; 72]]),
    Vd2(&'a [[bool; 49]]),
    FullRate(&'a [[bool; 144]]),
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    fich: &Fich,
    voice: Voice<'_>,
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(fich, Some(voice), None, false, rate)
}

fn transmission_inner(
    fich: &Fich,
    voice: Option<Voice<'_>>,
    call: Option<&Call>,
    calls_in_header: bool,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 13));
    let mut voice_at = 0;
    for (index, frame_type) in [fich.frame_type, 1, 1, 1, 2].into_iter().enumerate() {
        let frame_fich = Fich {
            frame_type,
            frame_number: if frame_type == 1 { index as u8 - 1 } else { 0 },
            ..*fich
        };
        let (payload, consumed) = payload(&frame_fich, voice, call, calls_in_header, voice_at);
        voice_at += consumed;
        symbols.extend(frame(&frame_fich, &payload));
    }
    symbols.extend(dibits(&filler(200, 17)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

fn frame(fich: &Fich, payload: &[u8]) -> Vec<u8> {
    let mut out = dibits(&bits(SYNC, 40));
    out.extend(dibits(&fich_bits(fich)));
    out.extend_from_slice(payload);
    assert_eq!(out.len(), 480);
    out
}

fn payload(
    fich: &Fich,
    voice: Option<Voice<'_>>,
    call: Option<&Call>,
    calls_in_header: bool,
    at: usize,
) -> (Vec<u8>, usize) {
    if calls_in_header
        && matches!(fich.frame_type, 0 | 2)
        && let Some(call) = call
    {
        return (callsign_payload(call), 0);
    }
    if fich.frame_type == 1
        && let Some(call) = call
        && let Some(payload) = communication_callsign_payload(fich, call)
    {
        return (payload, 0);
    }
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

fn fich_bits(fich: &Fich) -> Vec<bool> {
    let mut info = [0u8; 6];
    info[0] = fich.frame_type << 6 | 2 << 4;
    info[1] = (fich.frame_number & 0x07) << 3 | fich.frame_total & 0x07;
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

fn callsign_payload(call: &Call) -> Vec<u8> {
    let (first, second) = call_data(call);
    let first = dch_large(&first);
    let second = dch_large(&second);
    let mut payload = Vec::with_capacity(360);
    for block in 0..5 {
        payload.extend(dibits(&first[block * 72..block * 72 + 72]));
        payload.extend(dibits(&second[block * 72..block * 72 + 72]));
    }
    payload
}

fn communication_callsign_payload(fich: &Fich, call: &Call) -> Option<Vec<u8>> {
    let (first, second) = call_data(call);
    match fich.data_mode {
        0 if fich.frame_number == 0 => Some(vd1_payload(&dch_large(&first))),
        1 if fich.frame_number == 0 => Some(callsign_payload(call)),
        2 => {
            let data = match fich.frame_number {
                0 => &first[..10],
                1 => &first[10..],
                2 => &second[..10],
                3 => &second[10..],
                _ => return None,
            };
            Some(vd2_payload(&dch_small(data)))
        }
        _ => None,
    }
}

fn call_data(call: &Call) -> ([u8; 20], [u8; 20]) {
    let destination = call_field(&call.destination);
    let source = call_field(&call.source);
    let downlink = call_field(&call.downlink);
    let uplink = call_field(&call.uplink);
    (
        std::array::from_fn(|i| {
            if i < 10 {
                destination[i]
            } else {
                source[i - 10]
            }
        }),
        std::array::from_fn(|i| if i < 10 { downlink[i] } else { uplink[i - 10] }),
    )
}

fn vd1_payload(dch: &[bool]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(360);
    for block in 0..5 {
        payload.extend(dibits(&dch[block * 72..block * 72 + 72]));
        payload.extend(dibits(&filler(72, 61 + block as u32)));
    }
    payload
}

fn vd2_payload(dch: &[bool]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(360);
    for block in 0..5 {
        payload.extend(dibits(&dch[block * 40..block * 40 + 40]));
        payload.extend(dibits(&filler(104, 71 + block as u32)));
    }
    payload
}

fn call_field(value: &str) -> [u8; 10] {
    assert!(value.is_ascii() && value.len() <= 10);
    std::array::from_fn(|i| value.as_bytes().get(i).copied().unwrap_or(b' '))
}

fn dch_large(data: &[u8; 20]) -> Vec<bool> {
    let mut encoded = [0u8; 22];
    for i in 0..20 {
        encoded[i] = data[i] ^ WHITENING[i];
    }
    let crc = !crc16_msb(0x1021, 0, &encoded[..20]);
    encoded[20..].copy_from_slice(&crc.to_be_bytes());
    let mut input = encoded
        .iter()
        .flat_map(|&byte| bits(u64::from(byte), 8))
        .collect::<Vec<_>>();
    input.extend([false; 4]);
    let mut convolved = Vec::with_capacity(360);
    conv::encode(&input, &mut convolved);
    let mut interleaved = vec![false; 360];
    for i in 0..180 {
        let n = 2 * (i / 9) + 40 * (i % 9);
        interleaved[n] = convolved[i * 2];
        interleaved[n + 1] = convolved[i * 2 + 1];
    }
    interleaved
}

fn dch_small(data: &[u8]) -> Vec<bool> {
    let mut encoded = [0u8; 12];
    for i in 0..10 {
        encoded[i] = data[i] ^ WHITENING[i];
    }
    let crc = !crc16_msb(0x1021, 0, &encoded[..10]);
    encoded[10..].copy_from_slice(&crc.to_be_bytes());
    let mut input = encoded
        .iter()
        .flat_map(|&byte| bits(u64::from(byte), 8))
        .collect::<Vec<_>>();
    input.extend([false; 4]);
    let mut convolved = Vec::with_capacity(200);
    conv::encode(&input, &mut convolved);
    let mut interleaved = vec![false; 200];
    for i in 0..100 {
        let n = 2 * (i / 5) + 40 * (i % 5);
        interleaved[n] = convolved[i * 2];
        interleaved[n + 1] = convolved[i * 2 + 1];
    }
    interleaved
}
