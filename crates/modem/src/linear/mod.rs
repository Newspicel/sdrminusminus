//! The linear engine ( §3.1 `linear/`) — one implementation behind every
//! amplitude/phase modulation in the catalog: OOK and M-ASK, M-PAM, BPSK through M-PSK, the DPSK
//! family, OQPSK and π/2-BPSK, π/4-DQPSK, square and cross QAM, star-QAM, hierarchical QAM and
//! APSK. What distinguishes one from another is [`LinearParams`] — a point table, an amplitude
//! pulse, an oversampling, a per-symbol rotation, a quadrature stagger — and nothing else. A
//! `match` on a specific standard inside this module is a defect (§3.3, §8).
//!
//! **Three detection tiers, and what each is the answer to.**
//!
//! | Tier | Type | The question it answers |
//! |---|---|---|
//! | Coherent | [`LinearDemod`] + [`CarrierLoop`] | everything with a phase reference to recover |
//! | Differential | [`LinearDemod`] + [`differential_detect`] | phase ambiguity, at ~3 dB |
//! | Noncoherent envelope | [`EnvelopeDemod`] | no carrier recovery at all |
//!
//! The tiers are not a ladder with one winner. A DPSK entry has no coherent tier to be beaten
//! by, because its data *is* the phase difference; an OOK entry's envelope tier is what a keyed
//! transmitter and an unlocked receiver actually do. Where two tiers do detect the same entry,
//! §5 item 2 requires the second to be measured against the first, and the catalog records the
//! margin.
//!
//! **Chain** (coherent tier): `unstagger → matched filter → SymbolSync → blind power
//! normalisation → de-rotation → carrier loop`, one complex soft symbol per symbol period on the
//! constellation's own scale. [`demap`](crate::constellation::demap) turns those into LLRs — the
//! one demapper, shared with every other engine. [`LinearMod`] is the matching reference
//! transmitter, and `testgen` builds the demodulator's test signals from it so the two can never
//! drift apart (§1.2).
//!
//! **The known-symbol hook** (§3.4) is [`PhaseAnchor`]: "positions i..j carry known sequence S"
//! becomes a least-squares complex gain and frequency offset. It is the linear counterpart of
//! the CPM engine's [`KnownSymbols`](crate::cpm::KnownSymbols), and it is what resolves the
//! M-fold phase ambiguity no blind loop can, removes the blind normaliser's √(1 + 1/SNR) scale
//! bias, and gives a short burst a frequency estimate its loop had no time to acquire.
//!
//! **Two timing tiers.** [`LinearDemod`] tracks the clock with `sdrmm_dsp::SymbolSync` — the one
//! timing stack (§3.2), scheduled here and never reimplemented — which is the answer for a
//! continuously-keyed stream. [`FeedforwardTiming`] estimates a whole burst's offset in one shot
//! from the square-law spectral line, which is the answer for a burst and the only one the
//! high-order QAM rows can be measured through: at the tracking loop's best bandwidth their
//! waterfalls hit a wall at 1e-4 (64-QAM) and 8e-3 (256-QAM), and the wall is timing jitter.
//! Both drive the same Farrow kernel, so a comparison between them reads the estimator.

mod anchor;
mod carrier;
mod demod;
mod differential;
mod envelope;
mod modulator;
mod params;
mod timing;

pub use anchor::{AnchorError, PhaseAnchor};
pub use carrier::{CarrierLoop, PhaseDetector};
pub use demod::{
    LinearBurstDemod, LinearDemod, LinearTiming, TIMING_BW_BURST, TIMING_BW_CONTINUOUS,
};
pub use differential::{DifferentialDetector, differential_detect};
pub use envelope::{EnvelopeDemod, EnvelopeTiming, slice_amplitude};
pub use modulator::LinearMod;
pub use params::{LinearError, LinearParams};
pub use timing::{FeedforwardTiming, MIN_SPS, resample_at, square_law_offset};
