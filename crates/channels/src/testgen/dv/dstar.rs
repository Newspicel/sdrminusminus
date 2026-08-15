use num_complex::Complex;
use sdrmm_dsp::crc16_x25;
use sdrmm_modem::{
    cpm::{CpmMod, CpmParams, Mapping},
    pulse::{self, Norm},
};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_200.0;
const BT: f64 = 0.5;
const PULSE_SPAN: usize = 3;
const SYNC: u32 = 0x0055_2D16;
const FRAME_BITS: usize = 96;
const HEADER_BYTES: usize = 41;
const SCRAMBLER: [u8; 3] = [0x70, 0x4F, 0x93];
const AMBE_NULL: [u8; 9] = [0x9E, 0x8D, 0x32, 0x88, 0x26, 0x1A, 0x3F, 0x61, 0xE8];

pub struct Call {
    pub urcall: String,
    pub mycall: String,
    pub repeater: String,
    pub text: String,
}

impl Default for Call {
    fn default() -> Self {
        Self {
            urcall: "CQCQCQ".to_owned(),
            mycall: "DL1ABC".to_owned(),
            repeater: "DB0ABC B".to_owned(),
            text: "hello from sdr--".to_owned(),
        }
    }
}

#[must_use]
pub fn transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    let header = header(call);
    let mut slow = Vec::new();
    for chunk in header.chunks(5) {
        slow.push(build_packet(0x50 | chunk.len() as u8, chunk));
    }
    let text = format!("{:<20}", call.text);
    for (i, chunk) in text.as_bytes().chunks(4).take(4).enumerate() {
        slow.push(build_packet(0x40 | i as u8, chunk));
    }

    let mut bits = Vec::new();
    for i in 0..64 {
        bits.push(i % 2 == 0);
    }
    for superframe in 0..2 {
        bits.extend(voice_frame(&sync_data(), superframe));
        for frame in 1..21 {
            let index = superframe * 10 + (frame - 1) / 2;
            let source = slow.get(index).copied().unwrap_or([0x66; 6]);
            let half = (frame - 1) % 2 * 3;
            let bytes = [source[half], source[half + 1], source[half + 2]];
            bits.extend(voice_frame(&scramble(bytes), superframe * 21 + frame));
        }
    }
    gmsk(&bits, rate)
}

fn gmsk(bits: &[bool], rate: f64) -> Vec<Complex<f32>> {
    let sps = rate / BAUD;
    let mut tx = CpmMod::new(CpmParams::from_deviation(
        Mapping::natural(2),
        DEVIATION_HZ,
        BAUD,
        pulse::gaussian_freq(sps, BT, PULSE_SPAN, Norm::Area),
        sps,
    ));
    let symbols: Vec<u8> = bits.iter().map(|&b| u8::from(b)).collect();
    let mut iq = Vec::new();
    tx.modulate(&symbols, &mut iq);
    tx.flush(&mut iq);
    iq
}

fn sync_data() -> [u8; 3] {
    [(SYNC >> 16) as u8, (SYNC >> 8) as u8, SYNC as u8]
}

fn scramble(data: [u8; 3]) -> [u8; 3] {
    [
        data[0] ^ SCRAMBLER[0],
        data[1] ^ SCRAMBLER[1],
        data[2] ^ SCRAMBLER[2],
    ]
}

fn build_packet(header: u8, payload: &[u8]) -> [u8; 6] {
    let mut packet = [0x66u8; 6];
    packet[0] = header;
    for (slot, &byte) in packet[1..].iter_mut().zip(payload) {
        *slot = byte;
    }
    packet
}

fn voice_frame(data: &[u8; 3], _seed: usize) -> Vec<bool> {
    let mut bits = Vec::with_capacity(FRAME_BITS);
    for &byte in &AMBE_NULL {
        for i in (0..8).rev() {
            bits.push(byte >> i & 1 == 1);
        }
    }
    for &byte in data {
        for i in (0..8).rev() {
            bits.push(byte >> i & 1 == 1);
        }
    }
    bits
}

fn header(call: &Call) -> Vec<u8> {
    let field = |text: &str, len: usize| {
        let mut bytes = text.as_bytes().to_vec();
        bytes.resize(len, b' ');
        bytes
    };
    let mut header = vec![0u8, 0, 0];
    header.extend(field(&call.repeater, 8));
    header.extend(field(&call.repeater, 8));
    header.extend(field(&call.urcall, 8));
    header.extend(field(&call.mycall, 8));
    header.extend(field("SDR", 4));
    let crc = crc16_x25(&header);
    header.extend(crc.to_le_bytes());
    assert_eq!(header.len(), HEADER_BYTES);
    header
}
