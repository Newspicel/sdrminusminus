//! M17 reference transmitter: a link setup frame through the same chain the specification
//! defines — CRC, convolutional code, puncturing, interleaving, randomising — then stream
//! frames and an end-of-transmission marker.

use num_complex::Complex;
use sdrmm_dsp::{crc16_msb, fec::conv};

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 2_400.0;
const RRC_ALPHA: f64 = 0.5;

const SYNC_LSF: u64 = 0x55F7;
const SYNC_STREAM: u64 = 0xFF5D;
const SYNC_EOT: u64 = 0x555D;

const PAYLOAD_BITS: usize = 368;
const LSF_CODED_BITS: usize = 488;

const RANDOMIZER: [u8; 46] = [
    0xD6, 0xB5, 0xE2, 0x30, 0x82, 0xFF, 0x84, 0x62, 0xBA, 0x4E, 0x96, 0x90, 0xD8, 0x98, 0xDD, 0x5D,
    0x0C, 0xC8, 0x52, 0x43, 0x91, 0x1D, 0xF8, 0x6E, 0x68, 0x2F, 0x35, 0xDA, 0x14, 0xEA, 0xCD, 0x76,
    0x19, 0x8D, 0xD5, 0x80, 0xD1, 0x33, 0x87, 0x13, 0x57, 0x18, 0x2D, 0x29, 0x78, 0xC3,
];

const CALLSIGN_ALPHABET: &[u8; 40] = b" ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-/.";

/// A transmission: link setup, three stream frames, end of transmission.
#[must_use]
pub fn transmission(destination: &str, source: &str, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 79));
    symbols.extend(dibits(&bits(SYNC_LSF, 16)));
    symbols.extend(dibits(&link_setup(destination, source)));
    for i in 0..3 {
        symbols.extend(dibits(&bits(SYNC_STREAM, 16)));
        symbols.extend(dibits(&filler(PAYLOAD_BITS, 83 + i)));
    }
    symbols.extend(dibits(&bits(SYNC_EOT, 16)));
    symbols.extend(dibits(&filler(200, 89)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// The 368 payload bits of a link setup frame.
fn link_setup(destination: &str, source: &str) -> Vec<bool> {
    let mut lsf = bits(callsign(destination), 48);
    lsf.extend(bits(callsign(source), 48));
    // Stream type: voice over a stream, unencrypted.
    lsf.extend(bits(0b0000_0000_0000_0101, 16));
    // 112 bits of metadata, which a plain voice call leaves empty.
    lsf.extend(std::iter::repeat_n(false, 112));
    let bytes: Vec<u8> = lsf
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)))
        .collect();
    lsf.extend(bits(u64::from(crc16_msb(0x5935, 0xFFFF, &bytes)), 16));

    // Four flush bits, then the rate-1/2 code and its puncturing pattern.
    lsf.extend([false; 4]);
    let mut coded = Vec::with_capacity(LSF_CODED_BITS);
    conv::encode(&lsf, &mut coded);
    let punctured: Vec<bool> = coded
        .into_iter()
        .enumerate()
        .filter(|(i, _)| (i % 61) % 4 != 2)
        .map(|(_, bit)| bit)
        .collect();
    assert_eq!(punctured.len(), PAYLOAD_BITS);

    let mut interleaved = vec![false; PAYLOAD_BITS];
    for (i, slot) in interleaved.iter_mut().enumerate() {
        *slot = punctured[(45 * i + 92 * i * i) % PAYLOAD_BITS];
    }
    for (i, bit) in interleaved.iter_mut().enumerate() {
        if RANDOMIZER[i / 8] >> (7 - i % 8) & 1 == 1 {
            *bit = !*bit;
        }
    }
    interleaved
}

/// Base-40 callsign encoding (M17 spec §2.3.1).
fn callsign(call: &str) -> u64 {
    if call == "ALL" {
        return 0xFFFF_FFFF_FFFF;
    }
    call.bytes()
        .rev()
        .filter_map(|c| CALLSIGN_ALPHABET.iter().position(|&a| a == c))
        .fold(0u64, |acc, index| acc * 40 + index as u64)
}
