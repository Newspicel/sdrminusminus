//! The multicarrier waveforms beyond CP-OFDM (MODEM-PLAN §3.1 `multicarrier/`, §7 phase 9): four
//! frameworks that keep OFDM's "turn one hard channel into many easy ones" and each give up one
//! of the things that makes OFDM cheap.
//!
//! **A framework is not a peer of a mapper** (§3.3). Nothing here knows what a QAM point is: each
//! modulator takes points and each demodulator returns them, `constellation/` supplies the table
//! and the demapper, and the same rows the linear engine measures on a bare carrier are measured
//! again here — which is the acceptance, since a framework that does not land on its
//! constellation's own oracle is measuring its own defects.
//!
//! | Entry | Gives up | Buys | Reference |
//! |---|---|---|---|
//! | [`gfdm`] | orthogonality | one prefix per *block*, and a spectrum that rolls off | committed (both tiers) |
//! | [`ufmc`] | the prefix | out-of-band leakage, without a filter across the whole band | its constellation's oracle |
//! | [`fbmc`] | complex orthogonality | no prefix at all, and the sharpest spectrum of the four | its constellation's oracle |
//! | [`otfs`] | nothing — it is a precoder | diversity: every symbol rides every subcarrier | its constellation's oracle |
//!
//! **Three of the four are transparent under AWGN, and that is the point.** UFMC, FBMC and OTFS
//! are (near-)unitary maps from points to samples, so under thermal noise alone they can be
//! neither better nor worse than the constellation they carry — the same argument `spread/` makes
//! about a spreader. Every dB one of their curves sits from its oracle is framing overhead or a
//! defect, and the entries are gated on which. What the three are actually *for* shows up on the
//! axes AWGN cannot see: out-of-band leakage, prefix length, and frequency selectivity.
//!
//! **GFDM is the exception and it is the interesting one.** Its subcarriers overlap by
//! construction, so it has no unitary reading at all: a zero-forcing receiver removes the
//! self-interference and amplifies the noise, a matched one does neither, and the two curves cross.
//! Both are committed.
//!
//! **OTFS is a precoder, not a carrier** ([`otfs`]), which is why it is the one entry here that
//! consumes another: it spreads a delay–Doppler grid across the time–frequency grid an
//! [`ofdm`](crate::ofdm) frame already carries. Phase 6 measured that an uncoded one-tap equaliser
//! loses a nulled subcarrier outright; this is the transform that stops that being one symbol's
//! entire loss, and the entry measures exactly that.

pub mod fbmc;
pub mod gfdm;
pub mod otfs;
pub mod transform;
pub mod ufmc;

pub use fbmc::{FbmcDemod, FbmcMod, FbmcParams};
pub use gfdm::{GfdmDemod, GfdmDetector, GfdmMod, GfdmParams};
pub use otfs::{OtfsGrid, OtfsPrecoder};
pub use transform::Dft;
pub use ufmc::{UfmcDemod, UfmcMod, UfmcParams};
