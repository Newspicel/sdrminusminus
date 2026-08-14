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
    transmission_inner(shape, rf_channel, outbound, None, rate)
}

#[must_use]
#[allow(dead_code)]
pub(crate) fn transmission_with_voice(
    shape: &Shape,
    rf_channel: u8,
    outbound: bool,
    voice: &[[[bool; 72]; 4]; 5],
    rate: f64,
) -> Vec<Complex<f32>> {
    transmission_inner(shape, rf_channel, outbound, Some(voice), rate)
}

fn transmission_inner(
    shape: &Shape,
    rf_channel: u8,
    outbound: bool,
    voice: Option<&[[[bool; 72]; 4]; 5]>,
    rate: f64,
) -> Vec<Complex<f32>> {
    let mut symbols = dibits(&filler(400, 53));
    for (index, functional) in [0u8, 2, 2, 2, 0].into_iter().enumerate() {
        symbols.extend(frame(
            rf_channel,
            functional,
            outbound,
            voice.map(|frames| &frames[index]),
        ));
    }
    symbols.extend(dibits(&filler(200, 59)));
    c4fm(&symbols, rate, shape.baud, shape.deviation_hz, RRC_ALPHA)
}

/// One 384-bit frame.
fn frame(
    rf_channel: u8,
    functional: u8,
    outbound: bool,
    voice: Option<&[[bool; 72]; 4]>,
) -> Vec<u8> {
    let mut lich = bits(u64::from(rf_channel) & 0x03, 2);
    lich.extend(bits(u64::from(functional) & 0x03, 2));
    lich.extend(bits(3, 2));
    lich.push(outbound);
    let ones = lich.iter().filter(|b| **b).count();
    lich.push(ones % 2 == 0);
    let mut out = dibits(&bits(FSW, 20));
    let mut post: Vec<u8> = lich.into_iter().map(|bit| u8::from(bit) << 1).collect();
    post.extend(dibits(&filler(60, 61 + u32::from(functional))));
    if let Some(voice) = voice {
        for frame in voice {
            post.extend(dibits(frame));
        }
    } else {
        post.extend(dibits(&filler(288, 67 + u32::from(functional))));
    }
    assert_eq!(post.len(), 182);
    let mut register = 0xE4u16;
    for dibit in &mut post {
        let pn = register & 1 != 0;
        let feedback = (register ^ (register >> 4)) & 1;
        register = register >> 1 | feedback << 8;
        if pn {
            *dibit ^= 0b10;
        }
    }
    out.extend(post);
    out
}
