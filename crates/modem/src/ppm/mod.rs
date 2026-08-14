//! M-PPM ( §3.1 `ppm/`): a symbol is *when* the transmitter radiated, so the whole
//! engine is a comparison between M slots — and everything hard about it is the boundary
//! between them.
//!
//! The three parts, each with its own module:
//!
//! - [`SlotGrid`] — where the slots fall, when a slot is not a whole number of samples and the
//!   transmitter's clock owes the receiver's sample grid no phase. This is the piece
//!   `channels::adsb` proved in the field: at 2.048 Msps a Mode S half-chip is 1.024 samples,
//!   a fixed stride drifts a whole sample across a frame, and a single-sample peak cannot say
//!   which slot owned a band-limited pulse.
//! - [`PpmMod`] / [`SlotWaveform`] — the keyed-slot transmitter, rendered with the same
//!   sub-sample honesty so a test signal is never secretly aligned to the receiver's own
//!   windows (a green suite once hid a decoder that decoded nothing off the grid).
//! - [`PpmDemod`] — the two detector tiers, the argmax, and the soft output, with the
//!   calibration difference between them carried by [`Llr`](crate::soft::Llr) versus
//!   [`SoftBit`](crate::soft::SoftBit) rather than by a comment.
//!
//! The engine holds no protocol: which slots are preamble, how a burst is found, and what the
//! bits mean afterwards belong to the attachment. Mode S is exactly that — its preamble
//! correlation and CRC live in `channels::adsb`, and the slot arithmetic under them is this.

mod demod;
mod grid;
mod modulator;

pub use demod::{MAX_SLOT_BITS, MAX_SLOTS, PpmDemod, SlotDetector, llrs, soft_bits};
pub use grid::{SlotGrid, magnitudes};
pub use modulator::{OVERSAMPLE, PpmMod, SlotWaveform, slot_taps};
