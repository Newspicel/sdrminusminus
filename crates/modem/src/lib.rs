//! `sdrmm-modem` — the modulation library (MODEM-PLAN §1). Every modulation is implemented
//! exactly once, parameterised, and characterised by measurement. Engines compose the
//! numerical primitives in `sdrmm-dsp` (filters, loops, `SymbolSync`); protocol channels in
//! `sdrmm-channels` attach parameters and framing on top. The test harness in [`ber`] is the
//! universal consumer: an entry without its committed correctness curve, limits table and
//! performance baseline does not exist as far as this crate is concerned.
//!
//! Two conventions are locked here, before anything depends on them (MODEM-PLAN §8):
//!
//! - **Soft-decision sign: positive means logical 1.** Every soft symbol, soft bit and LLR
//!   this crate produces carries confidence for a transmitted 1 as a positive value — the
//!   convention `sdrmm_dsp::fec::conv` already defines, kept rather than fought. A true LLR
//!   additionally requires the noise variance the synchroniser estimates; a value without one
//!   is a confidence on an arbitrary scale, and the type system keeps the two apart.
//!
//! - **Pulse normalisation: unit energy.** Every discrete pulse the library designs satisfies
//!   `Σ h[n]² = 1`, asserted by test, so that a symbol's energy is its constellation point's
//!   squared magnitude and every Eb/N0 in [`ber`] means the same thing across entries.
//!
//! Measurement accounting (MODEM-PLAN §4.1): Eb/N0 is per *information* bit unless a curve
//! states otherwise; TDMA dead time is excluded from the energy accounting; uncoded SER/BER,
//! post-FEC BER, frame error rate and undetected-error rate are separate numbers, never mixed;
//! every run is seeded and reproducible.

pub mod ber;
pub mod constellation;
pub mod soft;
