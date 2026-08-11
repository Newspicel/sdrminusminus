//! D-Star reference transmitter: GMSK voice frames whose slow-data channel repeats the header,
//! which is the path a receiver joining a call in progress actually reads.

use num_complex::Complex;
use sdrmm_dsp::crc16_x25;

use super::filler;
use crate::testgen::fsk;

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_200.0;
const SYNC: u32 = 0x0055_2D16;
const FRAME_BITS: usize = 96;
const HEADER_BYTES: usize = 41;
const SCRAMBLER: [u8; 3] = [0x70, 0x4F, 0x93];

/// One call, as the header names it.
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

/// A transmission long enough for the header to come round twice in the slow-data channel.
#[must_use]
pub fn transmission(call: &Call, rate: f64) -> Vec<Complex<f32>> {
    let header = header(call);
    let mut slow = Vec::new();
    // Header segments, five bytes at a time, then the text message in four-character packets.
    for chunk in header.chunks(5) {
        slow.push(build_packet(0x50 | chunk.len() as u8, chunk));
    }
    let text = format!("{:<20}", call.text);
    for (i, chunk) in text.as_bytes().chunks(4).take(4).enumerate() {
        slow.push(build_packet(0x40 | i as u8, chunk));
    }

    let mut bits = Vec::new();
    // A lead-in of alternating bits: the bit sync every D-Star transmitter opens with.
    for i in 0..64 {
        bits.push(i % 2 == 0);
    }
    // Ten slow-data packets fit in a superframe: frames 1 and 2 carry the first, 3 and 4 the
    // second, and so on. Two superframes are enough for the header to come round once.
    for superframe in 0..2 {
        // Frame 0 of a superframe carries the sync in its data field.
        bits.extend(voice_frame(&sync_data(), superframe));
        for frame in 1..21 {
            let index = superframe * 10 + (frame - 1) / 2;
            let source = slow.get(index).copied().unwrap_or([0x66; 6]);
            let half = (frame - 1) % 2 * 3;
            let bytes = [source[half], source[half + 1], source[half + 2]];
            bits.extend(voice_frame(&scramble(bytes), superframe * 21 + frame));
        }
    }
    fsk(&bits, BAUD, DEVIATION_HZ, rate)
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

/// 72 bits of vocoder filler and 24 bits of data.
fn voice_frame(data: &[u8; 3], seed: usize) -> Vec<bool> {
    let mut bits = filler(FRAME_BITS - 24, 97 + seed as u32);
    for &byte in data {
        for i in (0..8).rev() {
            bits.push(byte >> i & 1 == 1);
        }
    }
    bits
}

/// The 41-byte header: flags, the four callsigns, and a CRC over the rest.
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
