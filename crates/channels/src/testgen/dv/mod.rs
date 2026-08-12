//! Reference modulators for the digital-voice decoders (PLAN §14).
//!
//! The shaping lives here and the framing lives in the per-mode submodules, because that is how
//! the modes actually differ: they all put four levels on a carrier the same way, and then each
//! spends its bits differently.
//!
//! What these generators do *not* produce is voice. Every mode's payload here is filler — the
//! most decoders read only the signalling around it, so their generated bursts carry a
//! deterministic pattern where a radio would carry AMBE or Codec2 frames. DMR's focused audio
//! test supplies real encoded vocoder sockets through its transmitter's payload seam.

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

/// A C4FM transmitter: one symbol per dibit through the modulation library's CPM modulator,
/// parameterised exactly as the receiving front end is (`dv::c4fm_params` — the shared dibit
/// table, RRC frequency pulse, h from the outer deviation), so transmitter and demodulator can
/// never drift apart. Continuously keyed: unit envelope, pulse tail flushed.
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

/// The same for a transmitter that keys off between bursts, as a TDMA radio does: `None` is a
/// symbol period it neither modulates nor radiates. `CpmMod::keyed` carries the transmit
/// judgments this generator used to hand-roll: a burst's shaping decays into silence with the
/// pulse tails a matched filter is built around, and the amplifier ramps over a symbol rather
/// than stepping.
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
