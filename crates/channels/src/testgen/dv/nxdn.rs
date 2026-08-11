//! NXDN reference transmitter: frame sync word, link information channel, filler payload.

use num_complex::Complex;

use super::{bits, c4fm, dibits, filler};

const RRC_ALPHA: f64 = 0.2;
const FSW: u64 = 0x000C_DF59;

/// Which NXDN width the generator is transmitting.
pub struct Shape {
    pub baud: f64,
    pub deviation_hz: f64,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            baud: 2_400.0,
            deviation_hz: 1_050.0,
        }
    }
}

/// A transmission: a signalling frame, three traffic frames and a closing signalling frame.
#[must_use]
pub fn transmission(shape: &Shape, rf_channel: u8, outbound: bool, rate: f64) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 53));
    for functional in [0u8, 2, 2, 2, 0] {
        symbols.extend(frame(rf_channel, functional, outbound));
    }
    symbols.extend(dibits(&filler(200, 59)));
    c4fm(&symbols, rate, shape.baud, shape.deviation_hz, RRC_ALPHA)
}

/// One 384-bit frame.
fn frame(rf_channel: u8, functional: u8, outbound: bool) -> Vec<u8> {
    let mut lich = bits(u64::from(rf_channel) & 0x03, 2);
    lich.extend(bits(u64::from(functional) & 0x03, 2));
    // Option, then direction.
    lich.extend(bits(0, 2));
    lich.push(outbound);
    // Odd parity over the seven bits before it.
    let ones = lich.iter().filter(|b| **b).count();
    lich.push(ones % 2 == 0);
    // The eight bits the specification reserves for the channel's own use.
    lich.extend(bits(0, 8));

    let mut out = dibits(&bits(FSW, 20));
    out.extend(dibits(&lich));
    out.extend(dibits(&filler(348, 61 + u32::from(functional))));
    out
}
