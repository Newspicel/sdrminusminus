//! CP-OFDM and DMT ( §3.1 `ofdm/`, §7 phase 6): the framework that turns a
//! frequency-selective channel into a bank of flat ones, so that every constellation the linear
//! engine already measures can be carried over a channel none of them survive alone.
//!
//! **A framework is not a peer of a mapper** (§3.3). Nothing here knows what a QAM point is: the
//! modulator takes points and the demodulator returns them, `constellation/` supplies the table
//! and the demapper, and the same four rows the linear engine measures on a bare carrier —
//! BPSK, QPSK, 16-QAM, 64-QAM — are measured again here on a subcarrier, against the same closed
//! forms. That is the acceptance: an OFDM curve that does not land on its subcarrier's own
//! oracle is measuring the framework's defects.
//!
//! The four parts, each with its module:
//!
//! - [`OfdmParams`] — the waveform as data: transform size, prefix, which bins carry what, the
//!   preamble's repeats, the pilot pattern, and the Hermitian flag that makes the same engine a
//!   real-baseband DMT transmitter.
//! - [`OfdmMod`] — points onto subcarriers, inverse transform, cyclic prefix, preamble. Unitary
//!   in both directions, which is what makes a per-subcarrier Eb/N0 the same quantity as the
//!   time-domain one the sweep runner sets.
//! - [`PreambleSync`] — where the burst is and how far off its carrier is, from the preamble's
//!   two repetition structures: coarse and unambiguous from the short one, fine from the long.
//! - [`ChannelEstimate`] / [`PilotTracker`] / [`OfdmDemod`] — least squares on known symbols,
//!   interpolation where the training is a comb, one tap per bin, and a per-symbol line through
//!   the pilots for what drifts inside a frame.
//!
//! The engine holds no protocol. 802.11's PLCP, its SIGNAL field and its rates are out of scope
//! by decision (§6): what is in scope is the waveform, measured on synthetic vectors at the
//! 64/16/48+4 geometry that standard made canonical.

mod demod;
mod equalize;
mod modulator;
mod params;
mod sync;

pub use demod::OfdmDemod;
pub use equalize::{
    ChannelEstimate, ChannelEstimator, PilotFit, PilotTracker, interpolate, noise_var_from_repeats,
};
pub use modulator::{OfdmMod, long_training_time};
pub use params::{
    Domain, OfdmError, OfdmParams, PilotPattern, Preamble, Subcarrier, SubcarrierMap,
};
pub use sync::{Acquisition, PreambleSync};
