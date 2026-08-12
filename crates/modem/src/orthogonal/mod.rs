//! Noncoherent orthogonal M-FSK (MODEM-PLAN §3.1 `orthogonal/`): M tones, one per symbol, read
//! as energies by a bank of matched filters — no carrier recovery anywhere in the chain.
//!
//! What separates this engine from the `cpm/` one, which also carries FSK: `cpm/` reads the
//! *frequency* of a waveform through a discriminator or a phase trellis, and its alphabet is a
//! set of levels on one axis. This engine reads M *orthogonal signals*, compares their energies,
//! and never forms a level at all. The consequences are the entry: it needs no phase and no
//! timing loop, it degrades to the exact noncoherent orthogonal closed form (`ber::theory`),
//! its soft output is an energy demapping rather than a slice, and it scales to alphabets a
//! discriminator cannot slice (FT8 is 8 tones at a 1.4 dB operating point).
//!
//! The pieces:
//!
//! - [`MfskParams`] — the tone plan, in the unit where orthogonality is checkable.
//! - [`MfskMod`] — the transmitter, in both phase policies; the continuous one *is* the crate's
//!   CPM modulator, since continuous-phase M-FSK is CPFSK at `h = spacing`.
//! - [`ToneBank`] — M symbol-long matched filters, normalised so a noise-only bin reads N0.
//! - [`MfskDemod`] — the feedforward burst timing, the argmax, and per-bit LLRs through the
//!   crate's one energy demapper.
//!
//! Framing is not here: which symbols are sync, how a burst is found, and what the bits mean
//! belong to the attachment (`ber::catalog::orthogonal` frames the measured chains; a protocol
//! would frame its own).

mod demod;
mod filterbank;
mod modulator;
mod params;

pub use demod::{MfskDemod, noise_var_from_energies};
pub use filterbank::ToneBank;
pub use modulator::{MfskMod, TonePhase};
pub use params::{MAX_TONES, MfskParams};
