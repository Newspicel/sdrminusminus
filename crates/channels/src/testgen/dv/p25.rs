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
    transmission_inner(nac, None, rate)
}

/// A transmission whose two LDUs contain caller-supplied Annex-H IMBE frames.
#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    nac: u16,
    voice: &[[[bool; 144]; 9]; 2],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(nac, Some(voice), rate)
}

fn transmission_inner(
    nac: u16,
    voice: Option<&[[[bool; 144]; 9]; 2]>,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 31));
    for (index, duid) in [0x0u8, 0x5, 0xA, 0x3].into_iter().enumerate() {
        let voice = match index {
            1 => voice.map(|frames| &frames[0]),
            2 => voice.map(|frames| &frames[1]),
            _ => None,
        };
        symbols.extend(frame(nac, duid, voice));
    }
    symbols.extend(dibits(&filler(200, 37)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// A single trunking block frame, which is what a control channel transmits continuously.
#[must_use]
pub fn trunking(nac: u16, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 41));
    symbols.extend(frame(nac, 0x7, None));
    symbols.extend(dibits(&filler(200, 43)));
    c4fm(&symbols, rate, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// One frame: sync, network identifier, payload — with a status di-bit inserted after every 35
/// payload di-bits, starting at bit 70.
fn frame(nac: u16, duid: u8, voice: Option<&[[bool; 144]; 9]>) -> Vec<u8> {
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
