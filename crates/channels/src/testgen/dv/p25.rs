//! P25 Phase 1 reference transmitter: sync, a BCH-coded network identifier with the status
//! symbols a transmitter interleaves into the frame, and filler for the rest.

use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, ParityCode, crc16_msb, rs64_encode};

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const SYNC: u64 = 0x5575_F5FF_77FF;

/// Frame lengths in bits, status symbols included (TIA-102.BAAA §7.3): a header, the two voice
/// frames, a terminator and a trunking block.
fn frame_bits(duid: u8) -> usize {
    match duid {
        0x0 => 792,
        0x3 => 144,
        0x7 => 720,
        0xF => 432,
        _ => 1_728,
    }
}

/// A transmission: header, two voice frames, terminator.
#[must_use]
pub fn transmission(nac: u16, rate: f64) -> Vec<Complex<f32>> {
    transmission_inner(nac, None, 0x80, rate)
}

/// A transmission whose two LDUs contain caller-supplied Annex-H IMBE frames.
#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    nac: u16,
    voice: &[[[bool; 144]; 9]; 2],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(nac, Some(voice), 0x80, rate)
}

#[must_use]
pub fn encrypted_transmission(
    nac: u16,
    voice: &[[[bool; 144]; 9]; 2],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(nac, Some(voice), 0x84, rate)
}

fn transmission_inner(
    nac: u16,
    voice: Option<&[[[bool; 144]; 9]; 2]>,
    algorithm: u8,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 31));
    for (index, duid) in [0x0u8, 0x5, 0xA, 0x3].into_iter().enumerate() {
        let voice = match index {
            1 => voice.map(|frames| &frames[0]),
            2 => voice.map(|frames| &frames[1]),
            _ => None,
        };
        if duid == 0x3 {
            // Conventional transmitters may leave a short unkeyed/idle interval before the
            // terminator; it also prevents the reference waveform's final LDU tail from being
            // the only acquisition lead-in the terminator test sees.
            symbols.extend(dibits(&filler(400, 89)));
        }
        symbols.extend(frame(nac, duid, voice, algorithm));
    }
    symbols.extend(dibits(&filler(200, 37)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// A single trunking block frame, which is what a control channel transmits continuously.
#[must_use]
pub fn trunking(nac: u16, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 41));
    symbols.extend(frame(nac, 0x7, None, 0x80));
    symbols.extend(dibits(&filler(200, 43)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// One frame: sync, network identifier, payload — with a status di-bit inserted after every 35
/// payload di-bits, starting at bit 70.
fn frame(nac: u16, duid: u8, voice: Option<&[[bool; 144]; 9]>, algorithm: u8) -> Vec<u8> {
    const IMBE_OFFSETS: [usize; 9] = [0, 72, 164, 256, 348, 440, 532, 624, 712];
    let total = frame_bits(duid);
    let status_count = total.saturating_sub(70).div_ceil(72);
    let logical_len = total - status_count * 2;
    let mut body = bits(SYNC, 48);
    body.extend(bits(
        CyclicCode::BCH_63_16.encode(u32::from(nac) << 4 | u32::from(duid & 0x0F)),
        64,
    ));
    body.extend(filler(
        logical_len.saturating_sub(body.len()),
        47 + u32::from(duid),
    ));
    if let Some(voice) = voice {
        for (&offset, frame) in IMBE_OFFSETS.iter().zip(voice) {
            let start = 48 + 64 + offset * 2;
            body[start..start + 144].copy_from_slice(frame);
        }
    }
    match duid {
        0x0 => insert_hdu(&mut body, algorithm),
        0x5 => insert_signalling(&mut body, &ldu1_symbols(algorithm != 0x80)),
        0xA => insert_signalling(&mut body, &ldu2_symbols(algorithm)),
        0x7 => insert_tsbk(&mut body),
        _ => {}
    }

    let mut out = Vec::with_capacity(total);
    let mut read = 0;
    while out.len() < total {
        if out.len() >= 70 && (out.len() - 70) % 72 == 0 {
            // Status di-bit 01: "unknown, use the default".
            out.push(false);
            out.push(true);
        } else {
            out.push(body[read]);
            read += 1;
        }
    }
    assert_eq!(read, body.len());
    dibits(&out)
}

fn insert_hdu(frame: &mut [bool], algorithm: u8) {
    let mut info = Vec::new();
    info.extend(bits(0x0123_4567_89AB_CDEF, 64));
    info.extend(bits(0x12, 8));
    info.extend(bits(0, 8));
    info.extend(bits(u64::from(algorithm), 8));
    info.extend(bits(0x1234, 16));
    info.extend(bits(0x1201, 16));
    let data = six_bit_symbols(&info);
    let codeword = rs64_encode(&data, 16);
    let mut coded = Vec::with_capacity(648);
    for symbol in codeword {
        coded.extend(bits(CyclicCode::GOLAY_18_6.encode(u32::from(symbol)), 18));
    }
    frame[112..112 + coded.len()].copy_from_slice(&coded);
}

fn ldu1_symbols(encrypted: bool) -> Vec<u8> {
    let mut lc = bits(0, 16);
    lc.extend(bits(u64::from(encrypted) << 6, 8));
    lc.extend(bits(0, 8));
    lc.extend(bits(0x1201, 16));
    lc.extend(bits(0xABCDEF, 24));
    rs64_encode(&six_bit_symbols(&lc), 12)
}

fn ldu2_symbols(algorithm: u8) -> Vec<u8> {
    let mut sync = bits(0x0123_4567_89AB_CDEF, 64);
    sync.extend(bits(0x12, 8));
    sync.extend(bits(u64::from(algorithm), 8));
    sync.extend(bits(0x1234, 16));
    rs64_encode(&six_bit_symbols(&sync), 8)
}

fn insert_signalling(frame: &mut [bool], symbols: &[u8]) {
    const OFFSETS: [usize; 9] = [0, 72, 164, 256, 348, 440, 532, 624, 712];
    let mut coded = Vec::with_capacity(240);
    for &symbol in symbols {
        let mut word = [false; 10];
        for (index, bit) in word[..6].iter_mut().enumerate() {
            *bit = symbol >> (5 - index) & 1 != 0;
        }
        ParityCode::HAMMING_10_6.encode(&mut word);
        coded.extend(word);
    }
    for (chunk, voice_index) in coded.as_slice().as_chunks::<40>().0.iter().zip(1usize..=6) {
        let start = 112 + (OFFSETS[voice_index] + 72) * 2;
        frame[start..start + 40].copy_from_slice(chunk);
    }
}

fn insert_tsbk(frame: &mut [bool]) {
    let mut payload = bits(2, 2);
    payload.extend(bits(0, 6));
    payload.extend(bits(0, 8));
    payload.extend(bits(0, 8));
    payload.extend(bits(0x1234, 16));
    payload.extend(bits(0x1201, 16));
    payload.extend(bits(0xABCDEF, 24));
    let bytes: Vec<u8> = payload
        .as_slice()
        .as_chunks::<8>()
        .0
        .iter()
        .map(|byte| {
            byte.iter()
                .fold(0u8, |value, &bit| value << 1 | u8::from(bit))
        })
        .collect();
    let crc = crc16_msb(0x1021, 0, &bytes) ^ 0xFFFF;
    payload.extend(bits(u64::from(crc), 16));
    let coded = data_interleave(&trellis_encode(&payload));
    frame[112..308].copy_from_slice(&coded);
}

fn trellis_encode(payload: &[bool]) -> Vec<bool> {
    const OUTPUT: [[[u8; 2]; 4]; 4] = [
        [[0, 2], [3, 0], [0, 1], [3, 3]],
        [[3, 2], [0, 0], [3, 1], [0, 3]],
        [[2, 1], [1, 3], [2, 2], [1, 0]],
        [[1, 1], [2, 3], [1, 2], [2, 0]],
    ];
    let mut state = 0usize;
    let mut coded = Vec::with_capacity(196);
    for input in payload
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from(pair[0]) << 1 | u8::from(pair[1]))
        .chain(std::iter::once(0))
    {
        for dibit in OUTPUT[state][usize::from(input)] {
            coded.push(dibit & 2 != 0);
            coded.push(dibit & 1 != 0);
        }
        state = usize::from(input);
    }
    coded
}

fn data_interleave(input: &[bool]) -> Vec<bool> {
    assert_eq!(input.len(), 196);
    let mut output = vec![false; 196];
    let mut source = 0;
    for row in 0..12 {
        for base in [0, 52, 100, 148] {
            output[base + row * 4..base + row * 4 + 4].copy_from_slice(&input[source..source + 4]);
            source += 4;
        }
    }
    output[48..52].copy_from_slice(&input[source..]);
    output
}

fn six_bit_symbols(data: &[bool]) -> Vec<u8> {
    data.as_chunks::<6>()
        .0
        .iter()
        .map(|symbol| {
            symbol
                .iter()
                .fold(0u8, |value, &bit| value << 1 | u8::from(bit))
        })
        .collect()
}
