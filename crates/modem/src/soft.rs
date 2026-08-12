//! The soft-decision vocabulary of the boundary `IQ → demod → soft symbols → demapper → LLRs →
//! FEC` (MODEM-PLAN §3.3). Both types carry the crate-root sign convention — positive means
//! logical 1 — and both are one `f32`. What separates them is calibration: a [`SoftBit`]'s
//! magnitude is confidence on whatever scale its producer used, while an [`Llr`] is the actual
//! log-likelihood ratio `ln(P(bit = 1 | y) / P(bit = 0 | y))`, which can only be computed with
//! the noise variance in hand. FEC that merely ranks hypotheses (Viterbi) is indifferent to a
//! constant scale and eats either; anything that *combines* likelihoods across observations
//! (Chase, turbo iteration, LLR summation over repeated bits) is only correct on true LLRs —
//! and the two kinds of number look identical as bare floats. The newtypes exist so that
//! mistake is a type error, not a decibel lost quietly in a curve.
//!
//! Every conversion below is one-directional in information: the doc of each states exactly
//! what is lost, because a conversion that looks free but drops calibration or resolution is
//! how soft-decision chains degrade without any test noticing.

use sdrmm_dsp::fec::conv;

/// A soft bit on an arbitrary confidence scale: the sign is the bit (positive = 1), the
/// magnitude means "more sure" but in no particular unit. Producers in this crate emit ±1.0
/// for a clean full-confidence symbol — the "clean symbol reaches full scale" convention the
/// phase-0 four-level front end set and [`crate::cpm::Mapping::soft_bits`] carries on — but the
/// type promises nothing beyond the sign.
///
/// Exactly 0.0 is an erasure: the absence of a vote, matching `fec::conv::ERASURE`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBit(pub f32);

/// A true log-likelihood ratio, `ln(P(1|y) / P(0|y))` in nats. Computing one requires the
/// noise variance (see `constellation::demap`), and holding this type is the claim that a
/// calibrated variance went in — magnitudes are comparable across symbols, across bursts, and
/// against probabilities (`P(wrong) = 1 / (1 + e^|llr|)`). Nothing arbitrary-scale may be
/// wrapped in it; that is the whole point of the type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Llr(pub f32);

/// |LLR| that [`Llr::to_fec`] maps to full Viterbi confidence. At 8 nats the bit is wrong with
/// probability 1/(1+e⁸) ≈ 3.4e-4, and one quantisation step of the i16 scale then spans
/// 8/64 = 0.125 nats. Saturating higher would spend the 7-bit range resolving certainty
/// differences no metric comparison can act on; lower would clip the region where soft
/// decisions actually out-vote each other.
pub const LLR_SATURATION: f32 = 8.0;

impl SoftBit {
    /// The hard decision. Loses the confidence entirely — and the erasure/0 case with it: an
    /// erasure slices to 0 here, so callers to whom "no vote" differs from "voted 0" must
    /// check [`Self::is_erasure`] first.
    #[must_use]
    pub fn bit(self) -> bool {
        self.0 > 0.0
    }

    /// Exactly zero — no vote either way, the analogue of `fec::conv::ERASURE`.
    #[must_use]
    pub fn is_erasure(self) -> bool {
        self.0 == 0.0
    }

    /// To the Viterbi's i16 scale: ±1.0 (a clean symbol) maps to ±`CONFIDENT`, and the clamp is
    /// why the scale is bounded at all — an over-confident value must not out-vote the rest of
    /// the frame. Lost: everything beyond unit magnitude saturates, and what survives is
    /// quantised to 1/64 of full scale.
    #[must_use]
    pub fn to_fec(self) -> conv::Soft {
        let full = f32::from(conv::CONFIDENT);
        (self.0 * full).clamp(-full, full) as conv::Soft
    }
}

impl Llr {
    /// The hard decision; same information loss and erasure caveat as [`SoftBit::bit`].
    #[must_use]
    pub fn bit(self) -> bool {
        self.0 > 0.0
    }

    /// Exactly zero — both hypotheses equally likely, which as a vote is an erasure.
    #[must_use]
    pub fn is_erasure(self) -> bool {
        self.0 == 0.0
    }

    /// To the Viterbi's i16 scale: ±[`LLR_SATURATION`] nats and beyond map to ±`CONFIDENT`.
    /// Lost: certainty past the saturation point, and resolution below one step of
    /// [`LLR_SATURATION`]/`CONFIDENT` = 0.125 nats.
    #[must_use]
    pub fn to_fec(self) -> conv::Soft {
        let full = f32::from(conv::CONFIDENT);
        (self.0 * full / LLR_SATURATION).clamp(-full, full) as conv::Soft
    }
}

/// The hard decision of a *bank* of detection statistics — the argmax an orthogonal receiver
/// makes where a linear one slices. One definition, because both engines that make it (the
/// M-FSK filterbank's tone energies, M-PPM's slot statistics) must agree on the one thing an
/// argmax leaves open:
///
/// **Ties resolve to the later index.** They carry no information — a dead window reads all
/// zeros, and equal statistics are equal evidence — so what matters is that the rule is fixed
/// and stated rather than emergent. `channels::adsb` has always had it: "a 1 is energy in the
/// *first* half of the bit", so two equal halves are not a 1.
///
/// # Panics
/// If `statistics` is empty — an argmax over nothing is not 0, it is a caller bug.
#[must_use]
pub fn argmax(statistics: &[f32]) -> u8 {
    assert!(!statistics.is_empty(), "no statistics, no decision");
    let mut best = 0usize;
    for (k, &s) in statistics.iter().enumerate() {
        if s >= statistics[best] {
            best = k;
        }
    }
    best as u8
}

/// Numerically free — an LLR is a perfectly good confidence. Lost: the calibration claim.
/// The `SoftBit` no longer promises its magnitude is in nats, and there is deliberately no
/// conversion back.
impl From<Llr> for SoftBit {
    fn from(llr: Llr) -> Self {
        Self(llr.0)
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::fec::conv::{CONFIDENT, ERASURE};

    use super::*;

    #[test]
    fn sign_convention_positive_is_one() {
        assert!(SoftBit(0.3).bit());
        assert!(!SoftBit(-0.3).bit());
        assert!(Llr(5.0).bit());
        assert!(!Llr(-5.0).bit());
    }

    #[test]
    fn zero_is_an_erasure_and_slices_to_zero() {
        assert!(SoftBit(0.0).is_erasure());
        assert!(Llr(0.0).is_erasure());
        assert!(!SoftBit(1e-9).is_erasure());
        assert_eq!(SoftBit(0.0).to_fec(), ERASURE);
        assert_eq!(Llr(0.0).to_fec(), ERASURE);
    }

    #[test]
    fn softbit_to_fec_scales_and_saturates() {
        assert_eq!(SoftBit(1.0).to_fec(), CONFIDENT);
        assert_eq!(SoftBit(-1.0).to_fec(), -CONFIDENT);
        assert_eq!(SoftBit(0.5).to_fec(), CONFIDENT / 2);
        // Beyond unit confidence saturates instead of out-voting the frame.
        assert_eq!(SoftBit(37.0).to_fec(), CONFIDENT);
        assert_eq!(SoftBit(-2.5).to_fec(), -CONFIDENT);
    }

    #[test]
    fn llr_to_fec_saturates_at_the_stated_point() {
        assert_eq!(Llr(LLR_SATURATION).to_fec(), CONFIDENT);
        assert_eq!(Llr(-LLR_SATURATION).to_fec(), -CONFIDENT);
        assert_eq!(Llr(100.0).to_fec(), CONFIDENT);
        assert_eq!(Llr(LLR_SATURATION / 2.0).to_fec(), CONFIDENT / 2);
        // One representable step is 0.125 nats.
        assert_eq!(Llr(0.125).to_fec(), 1);
        assert_eq!(Llr(0.06).to_fec(), 0);
    }

    #[test]
    fn llr_demotes_to_softbit_without_changing_the_number() {
        let soft: SoftBit = Llr(-2.75).into();
        assert_eq!(soft, SoftBit(-2.75));
    }
}
