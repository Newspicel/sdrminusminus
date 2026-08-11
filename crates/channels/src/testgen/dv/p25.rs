//! P25 Phase 1 reference transmitter: sync, a BCH-coded network identifier with the status
//! symbols a transmitter interleaves into the frame, and filler for the rest.

use num_complex::Complex;
use sdrmm_dsp::CyclicCode;

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
    let mut symbols = dibits(&filler(400, 31));
    for duid in [0x0u8, 0x5, 0xA, 0x3] {
        symbols.extend(frame(nac, duid));
    }
    symbols.extend(dibits(&filler(200, 37)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// A single trunking block frame, which is what a control channel transmits continuously.
#[must_use]
pub fn trunking(nac: u16, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 41));
    symbols.extend(frame(nac, 0x7));
    symbols.extend(dibits(&filler(200, 43)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// One frame: sync, network identifier, payload — with a status di-bit inserted after every 35
/// payload di-bits, starting at bit 70.
fn frame(nac: u16, duid: u8) -> Vec<u8> {
    let mut body = bits(SYNC, 48);
    body.extend(bits(
        CyclicCode::BCH_63_16.encode(u32::from(nac) << 4 | u32::from(duid & 0x0F)),
        64,
    ));
    let total = frame_bits(duid);
    body.extend(filler(
        total.saturating_sub(body.len()),
        47 + u32::from(duid),
    ));

    let mut out = Vec::with_capacity(total);
    let mut written = 0;
    for bit in body {
        if written >= 70 && (written - 70) % 72 == 0 {
            // Status di-bit 01: "unknown, use the default".
            out.push(false);
            out.push(true);
            written += 2;
        }
        out.push(bit);
        written += 1;
    }
    dibits(&out)
}
