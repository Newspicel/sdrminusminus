use sdrmm_dsp::fec::conv;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoftBit(pub f32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Llr(pub f32);

pub const LLR_SATURATION: f32 = 8.0;

impl SoftBit {
    #[must_use]
    pub fn bit(self) -> bool {
        self.0 > 0.0
    }

    #[must_use]
    pub fn is_erasure(self) -> bool {
        self.0 == 0.0
    }

    #[must_use]
    pub fn to_fec(self) -> conv::Soft {
        let full = f32::from(conv::CONFIDENT);
        (self.0 * full).clamp(-full, full) as conv::Soft
    }
}

impl Llr {
    #[must_use]
    pub fn bit(self) -> bool {
        self.0 > 0.0
    }

    #[must_use]
    pub fn is_erasure(self) -> bool {
        self.0 == 0.0
    }

    #[must_use]
    pub fn to_fec(self) -> conv::Soft {
        let full = f32::from(conv::CONFIDENT);
        (self.0 * full / LLR_SATURATION).clamp(-full, full) as conv::Soft
    }
}

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
        assert_eq!(SoftBit(37.0).to_fec(), CONFIDENT);
        assert_eq!(SoftBit(-2.5).to_fec(), -CONFIDENT);
    }

    #[test]
    fn llr_to_fec_saturates_at_the_stated_point() {
        assert_eq!(Llr(LLR_SATURATION).to_fec(), CONFIDENT);
        assert_eq!(Llr(-LLR_SATURATION).to_fec(), -CONFIDENT);
        assert_eq!(Llr(100.0).to_fec(), CONFIDENT);
        assert_eq!(Llr(LLR_SATURATION / 2.0).to_fec(), CONFIDENT / 2);
        assert_eq!(Llr(0.125).to_fec(), 1);
        assert_eq!(Llr(0.06).to_fec(), 0);
    }

    #[test]
    fn llr_demotes_to_softbit_without_changing_the_number() {
        let soft: SoftBit = Llr(-2.75).into();
        assert_eq!(soft, SoftBit(-2.75));
    }
}
