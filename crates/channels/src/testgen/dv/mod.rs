//! Reference modulators for the digital-voice decoders (PLAN §14).
//!
//! The shaping lives here and the framing lives in the per-mode submodules, because that is how
//! the modes actually differ: they all put four levels on a carrier the same way, and then each
//! spends its bits differently.
//!
//! What these generators do *not* produce is voice. Every mode's payload here is filler — the
//! decoders read the signalling around it and there is no vocoder to feed — so a generated
//! burst carries a deterministic pattern where a radio would carry AMBE or Codec2 frames.

pub mod dmr;
pub mod dpmr;
pub mod dstar;
pub mod m17;
pub mod nxdn;
pub mod p25;
pub mod ysf;

use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{RealDecimator, design_rrc, fsk4};

/// Matched-filter span the receivers use, and so the span a transmitter must shape to for the
/// cascade to be a Nyquist pulse.
const RRC_SPAN: usize = 8;

/// A C4FM transmitter: one symbol per dibit through a root-raised-cosine shaping filter, then
/// frequency modulation at `deviation_hz` for the outer levels.
#[must_use]
pub fn c4fm(
    dibits: &[u8],
    rate: f64,
    baud: f64,
    deviation_hz: f64,
    alpha: f64,
) -> Vec<Complex<f32>> {
    let sps = rate / baud;
    let taps = design_rrc(sps, alpha, RRC_SPAN);
    let mut impulses = vec![0.0f32; dibits.len() * sps as usize + taps.len()];
    for (i, &dibit) in dibits.iter().enumerate() {
        impulses[i * sps as usize] = fsk4::level(dibit) / 3.0 * sps as f32;
    }
    let mut shaped = Vec::new();
    RealDecimator::new(&taps, 1).process(&impulses, &mut shaped);
    let mut phase = 0.0f64;
    shaped
        .iter()
        .map(|&s| {
            phase += TAU * f64::from(s) * deviation_hz / rate;
            Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

/// Split bits into the dibits a 4FSK symbol carries, most significant bit first.
#[must_use]
pub fn dibits(bits: &[bool]) -> Vec<u8> {
    bits.chunks(2)
        .map(|pair| u8::from(pair[0]) << 1 | u8::from(*pair.get(1).unwrap_or(&false)))
        .collect()
}

/// The `len` low bits of `value`, most significant first — how every field in these
/// specifications is written down.
#[must_use]
pub fn bits(value: u64, len: usize) -> Vec<bool> {
    (0..len).rev().map(|i| value >> i & 1 == 1).collect()
}

/// Deterministic filler where a radio would put vocoder frames.
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
