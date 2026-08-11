//! System Fusion reference transmitter: sync, a coded FICH, and filler where the vocoder and
//! data channel would be.

use num_complex::Complex;
use sdrmm_dsp::{CyclicCode, crc16_msb, fec::conv};

use super::{bits, c4fm, dibits, filler};

const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const SYNC: u64 = 0x00D4_71C9_634D;

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
    let mut symbols = dibits(&filler(400, 13));
    for frame_type in [fich.frame_type, 1, 1, 1, 2] {
        symbols.extend(frame(&Fich {
            frame_type,
            ..*fich
        }));
    }
    symbols.extend(dibits(&filler(200, 17)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// One 480-symbol frame.
fn frame(fich: &Fich) -> Vec<u8> {
    let mut out = dibits(&bits(SYNC, 40));
    out.extend(dibits(&fich_bits(fich)));
    // Everything after the FICH is the data and voice channel, which carries no signalling
    // this decoder reads.
    out.extend(dibits(&filler(720, 23)));
    out
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
