#![allow(clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    constellation::tables,
    linear::{LinearMod, LinearParams},
    pulse::{self, Norm},
};

use crate::psk::VARICODE;

const RATE: f64 = 8_000.0;
const PREAMBLE_BITS: usize = 32;
const POSTAMBLE_BITS: usize = 16;

#[must_use]
pub fn transmission(text: &str, baud: f64) -> Vec<Complex<f32>> {
    let mut bits = vec![false; PREAMBLE_BITS];
    for byte in text.bytes() {
        bits.extend(VARICODE[usize::from(byte)].bytes().map(|bit| bit == b'1'));
        bits.extend([false, false]);
    }
    bits.extend(std::iter::repeat_n(true, POSTAMBLE_BITS));

    let mut state = 0u32;
    let symbols: Vec<u32> = std::iter::once(state)
        .chain(bits.into_iter().map(|bit| {
            if !bit {
                state ^= 1;
            }
            state
        }))
        .collect();
    let sps = (RATE / baud).round() as usize;
    let pulse = pulse::root_raised_cosine(sps as f64, 1.0, 4, Norm::Energy);
    let params = LinearParams::new(tables::psk(2).expect("BPSK table"), pulse, sps)
        .expect("PSK fixture waveform");
    LinearMod::transmission(&params, &symbols)
}
