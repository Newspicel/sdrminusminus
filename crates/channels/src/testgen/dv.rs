pub mod dmr;
pub mod dpmr;
pub mod dstar;
pub mod m17;
pub mod nxdn;
pub mod p25;
pub mod ysf;

use num_complex::Complex;
use sdrmm_modem::cpm::CpmMod;

use crate::dv::c4fm_params;

#[must_use]
pub fn c4fm(
    dibits: &[u8],
    rate: f64,
    baud: f64,
    deviation_hz: f64,
    alpha: f64,
) -> Vec<Complex<f32>> {
    let mut tx = CpmMod::new(c4fm_params(rate, baud, deviation_hz, alpha));
    let mut out = Vec::new();
    tx.modulate(dibits, &mut out);
    tx.flush(&mut out);
    out
}

#[must_use]
pub fn c4fm_keyed(
    symbols: &[Option<u8>],
    rate: f64,
    baud: f64,
    deviation_hz: f64,
    alpha: f64,
) -> Vec<Complex<f32>> {
    CpmMod::new(c4fm_params(rate, baud, deviation_hz, alpha)).keyed(symbols)
}

#[must_use]
pub fn dibits(bits: &[bool]) -> Vec<u8> {
    bits.chunks(2)
        .map(|pair| u8::from(pair[0]) << 1 | u8::from(*pair.get(1).unwrap_or(&false)))
        .collect()
}

#[must_use]
pub fn bits(value: u64, len: usize) -> Vec<bool> {
    (0..len).rev().map(|i| value >> i & 1 == 1).collect()
}

#[must_use]
pub fn filler(len: usize, seed: u32) -> Vec<bool> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state & 1 == 1
        })
        .collect()
}
