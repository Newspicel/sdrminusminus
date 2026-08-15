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

#[must_use]
pub fn transmission(destination: &str, source: &str, rate: f64) -> Vec<Complex<f32>> {
    const CODEC2_SILENCE_PAIR: [u8; 16] = [
        0x01, 0x00, 0x09, 0x43, 0x9C, 0xE4, 0x21, 0x08, 0x01, 0x00, 0x09, 0x43, 0x9C, 0xE4, 0x21,
        0x08,
    ];
    let voice = [CODEC2_SILENCE_PAIR; 3];
    transmission_with_voice(destination, source, &voice, rate)
}

#[must_use]
pub(crate) fn transmission_with_voice(
    destination: &str,
    source: &str,
    voice: &[[u8; 16]],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_with_type(destination, source, voice, 0b0101, rate)
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice_data(
    destination: &str,
    source: &str,
    payloads: &[[u8; 16]],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_with_type(destination, source, payloads, 0b0111, rate)
}

fn transmission_with_type(
    destination: &str,
    source: &str,
    voice: &[[u8; 16]],
    stream_type: u16,
    rate: f64,
) -> Vec<Complex<f32>> {
    stream_transmission(destination, source, voice, stream_type, true, rate)
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn late_entry(destination: &str, source: &str, rate: f64) -> Vec<Complex<f32>> {
    const CODEC2_SILENCE_PAIR: [u8; 16] = [
        0x01, 0x00, 0x09, 0x43, 0x9C, 0xE4, 0x21, 0x08, 0x01, 0x00, 0x09, 0x43, 0x9C, 0xE4, 0x21,
        0x08,
    ];
    stream_transmission(
        destination,
        source,
        &[CODEC2_SILENCE_PAIR; 8],
        0b0101,
        false,
        rate,
    )
}

fn stream_transmission(
    destination: &str,
    source: &str,
    voice: &[[u8; 16]],
    stream_type: u16,
    link_setup_frame: bool,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 79));
    let lsf = lsf_bits(destination, source, stream_type);
    if link_setup_frame {
        symbols.extend(dibits(&bits(SYNC_LSF, 16)));
        symbols.extend(dibits(&link_setup(&lsf)));
    }
    for (i, payload) in voice.iter().enumerate() {
        symbols.extend(dibits(&bits(SYNC_STREAM, 16)));
        symbols.extend(dibits(&stream_frame(&lsf, i as u16, payload)));
    }
    symbols.extend(dibits(&bits(SYNC_EOT, 16)));
    symbols.extend(dibits(&filler(200, 89)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

fn lsf_bits(destination: &str, source: &str, stream_type: u16) -> Vec<bool> {
    let mut lsf = bits(callsign(destination), 48);
    lsf.extend(bits(callsign(source), 48));
    lsf.extend(bits(u64::from(stream_type), 16));
    lsf.extend(std::iter::repeat_n(false, 112));
    let bytes: Vec<u8> = lsf
        .as_chunks::<8>()
        .0
        .iter()
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| acc << 1 | u8::from(b)))
        .collect();
    lsf.extend(bits(u64::from(crc16_msb(0x5935, 0xFFFF, &bytes)), 16));
    lsf
}

fn link_setup(lsf: &[bool]) -> Vec<bool> {
    let mut lsf = lsf.to_vec();
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

fn stream_frame(lsf: &[bool], number: u16, payload: &[u8; 16]) -> Vec<bool> {
    let chunk = usize::from(number % 6);
    let mut lich = lsf[chunk * 40..(chunk + 1) * 40].to_vec();
    lich.extend(bits(chunk as u64, 3));
    lich.extend([false; 5]);
    let mut combined = Vec::with_capacity(PAYLOAD_BITS);
    for block in lich.as_chunks::<12>().0 {
        let value = block
            .iter()
            .fold(0u32, |acc, &bit| acc << 1 | u32::from(bit));
        combined.extend(bits(sdrmm_dsp::CyclicCode::GOLAY_24_12.encode(value), 24));
    }

    let mut contents = bits(u64::from(number), 16);
    for &byte in payload {
        contents.extend(bits(u64::from(byte), 8));
    }
    contents.extend([false; 4]);
    let mut coded = Vec::with_capacity(296);
    conv::encode(&contents, &mut coded);
    combined.extend(
        coded
            .into_iter()
            .enumerate()
            .filter_map(|(i, bit)| (i % 12 != 11).then_some(bit)),
    );
    assert_eq!(combined.len(), PAYLOAD_BITS);

    let mut interleaved = vec![false; PAYLOAD_BITS];
    for (i, slot) in interleaved.iter_mut().enumerate() {
        *slot = combined[(45 * i + 92 * i * i) % PAYLOAD_BITS];
        if RANDOMIZER[i / 8] >> (7 - i % 8) & 1 == 1 {
            *slot = !*slot;
        }
    }
    interleaved
}

fn callsign(call: &str) -> u64 {
    if call == "ALL" {
        return 0xFFFF_FFFF_FFFF;
    }
    call.bytes()
        .rev()
        .filter_map(|c| CALLSIGN_ALPHABET.iter().position(|&a| a == c))
        .fold(0u64, |acc, index| acc * 40 + index as u64)
}
