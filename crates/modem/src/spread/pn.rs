use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PnError {
    NotBipolar(i8),
    Empty,
    NoBarkerWord(usize),
    NoPrimitivePolynomial(u32),
}

impl fmt::Display for PnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBipolar(chip) => write!(f, "chip {chip} is not ±1"),
            Self::Empty => write!(f, "an empty spreading sequence spreads nothing"),
            Self::NoBarkerWord(n) => write!(f, "no Barker word of length {n} is known"),
            Self::NoPrimitivePolynomial(degree) => {
                write!(f, "no primitive polynomial of degree {degree} is tabulated")
            }
        }
    }
}

impl std::error::Error for PnError {}

pub const MAX_LFSR_DEGREE: u32 = 16;

const PRIMITIVE_TAPS: [u32; (MAX_LFSR_DEGREE - 1) as usize] = [
    0b11,
    0b011,
    0b0011,
    0b00101,
    0b000011,
    0b0001001,
    0b00011101,
    0b000010001,
    0b0000001001,
    0b00000000101,
    0b000001010011,
    0b0000000011011,
    0b00010001000011,
    0b000000000000011,
    0b0110100000000001,
];

const BARKER: [(usize, &[i8]); 7] = [
    (13, &[1, 1, 1, 1, 1, -1, -1, 1, 1, -1, 1, -1, 1]),
    (11, &[1, -1, 1, 1, -1, 1, 1, 1, -1, -1, -1]),
    (7, &[1, 1, 1, -1, -1, 1, -1]),
    (5, &[1, 1, 1, -1, 1]),
    (4, &[1, 1, -1, 1]),
    (3, &[1, 1, -1]),
    (2, &[1, -1]),
];

#[derive(Clone, Debug, PartialEq)]
pub struct PnSequence {
    chips: Vec<f32>,
}

impl PnSequence {
    pub fn from_chips(chips: &[i8]) -> Result<Self, PnError> {
        if chips.is_empty() {
            return Err(PnError::Empty);
        }
        for &chip in chips {
            if chip != 1 && chip != -1 {
                return Err(PnError::NotBipolar(chip));
            }
        }
        Ok(Self {
            chips: chips.iter().map(|&c| f32::from(c)).collect(),
        })
    }

    pub fn barker(n: usize) -> Result<Self, PnError> {
        let word = BARKER
            .iter()
            .find(|(len, _)| *len == n)
            .ok_or(PnError::NoBarkerWord(n))?;
        Self::from_chips(word.1)
    }

    pub fn maximal_length(degree: u32) -> Result<Self, PnError> {
        if !(2..=MAX_LFSR_DEGREE).contains(&degree) {
            return Err(PnError::NoPrimitivePolynomial(degree));
        }
        let taps = PRIMITIVE_TAPS[(degree - 2) as usize];
        let mask = (1u32 << degree) - 1;
        let period = (1usize << degree) - 1;
        let mut state = mask;
        let mut chips = Vec::with_capacity(period);
        for _ in 0..period {
            let out = state & 1;
            chips.push(if out == 0 { 1.0 } else { -1.0 });
            let feedback = (state & taps).count_ones() & 1;
            state = (state >> 1) | (feedback << (degree - 1));
            state &= mask;
        }
        Ok(Self { chips })
    }

    #[must_use]
    pub fn chips(&self) -> &[f32] {
        &self.chips
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.chips.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chips.is_empty()
    }

    #[must_use]
    pub fn processing_gain_db(&self) -> f64 {
        10.0 * (self.len() as f64).log10()
    }

    #[must_use]
    pub fn aperiodic_autocorrelation(&self, shift: isize) -> f64 {
        let n = self.len() as isize;
        if shift.abs() >= n {
            return 0.0;
        }
        let (lo, hi) = if shift >= 0 {
            (0, n - shift)
        } else {
            (-shift, n)
        };
        (lo..hi)
            .map(|i| {
                let a = f64::from(self.chips[i as usize]);
                let b = f64::from(self.chips[(i + shift) as usize]);
                a * b
            })
            .sum()
    }

    #[must_use]
    pub fn periodic_autocorrelation(&self, shift: isize) -> f64 {
        let n = self.len() as isize;
        let shift = shift.rem_euclid(n);
        (0..n)
            .map(|i| {
                let a = f64::from(self.chips[i as usize]);
                let b = f64::from(self.chips[((i + shift) % n) as usize]);
                a * b
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_barker_word_has_unit_sidelobes() {
        for n in [2usize, 3, 4, 5, 7, 11, 13] {
            let pn = PnSequence::barker(n).unwrap();
            assert_eq!(pn.len(), n);
            assert!((pn.aperiodic_autocorrelation(0) - n as f64).abs() < 1e-12);
            for shift in 1..n as isize {
                let side = pn.aperiodic_autocorrelation(shift).abs();
                assert!(side <= 1.0 + 1e-12, "Barker-{n} sidelobe {side} at {shift}");
                let mirrored = pn.aperiodic_autocorrelation(-shift);
                assert!((mirrored - pn.aperiodic_autocorrelation(shift)).abs() < 1e-12);
            }
        }
        assert_eq!(PnSequence::barker(6).unwrap_err(), PnError::NoBarkerWord(6));
        assert_eq!(
            PnSequence::barker(31).unwrap_err(),
            PnError::NoBarkerWord(31)
        );
    }

    #[test]
    fn barker11_is_the_802_11_ordering() {
        let pn = PnSequence::barker(11).unwrap();
        let want = [
            1.0f32, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0,
        ];
        assert_eq!(pn.chips(), want);
        let textbook = PnSequence::from_chips(&[1, 1, 1, -1, -1, -1, 1, -1, -1, 1, -1]).unwrap();
        let negated_reverse: Vec<f32> = textbook.chips().iter().rev().map(|c| -c).collect();
        assert_eq!(pn.chips(), negated_reverse.as_slice());
        assert!((pn.processing_gain_db() - 10.414).abs() < 1e-3);
    }

    #[test]
    fn every_m_sequence_has_full_period_and_flat_periodic_autocorrelation() {
        for degree in 2..=MAX_LFSR_DEGREE {
            let pn = PnSequence::maximal_length(degree).unwrap();
            let period = (1usize << degree) - 1;
            assert_eq!(pn.len(), period, "degree {degree}");
            assert!((pn.periodic_autocorrelation(0) - period as f64).abs() < 1e-9);
            for shift in 1..period as isize {
                let side = pn.periodic_autocorrelation(shift);
                assert!(
                    (side + 1.0).abs() < 1e-9,
                    "degree {degree}: periodic ACF {side} at shift {shift}"
                );
            }
            let sum: f64 = pn.chips().iter().map(|&c| f64::from(c)).sum();
            assert!((sum + 1.0).abs() < 1e-9, "degree {degree} sum {sum}");
        }
        assert_eq!(
            PnSequence::maximal_length(1).unwrap_err(),
            PnError::NoPrimitivePolynomial(1)
        );
        assert_eq!(
            PnSequence::maximal_length(17).unwrap_err(),
            PnError::NoPrimitivePolynomial(17)
        );
    }

    #[test]
    fn tabulated_degrees_give_distinct_sequences() {
        let mut seen: Vec<Vec<f32>> = Vec::new();
        for degree in 2..=MAX_LFSR_DEGREE {
            let chips = PnSequence::maximal_length(degree).unwrap().chips().to_vec();
            assert!(
                !seen.contains(&chips),
                "degree {degree} duplicates a table row"
            );
            seen.push(chips);
        }
    }

    #[test]
    fn bad_tables_are_rejected_with_the_right_error() {
        assert_eq!(PnSequence::from_chips(&[]).unwrap_err(), PnError::Empty);
        assert_eq!(
            PnSequence::from_chips(&[1, 0, -1]).unwrap_err(),
            PnError::NotBipolar(0)
        );
        assert_eq!(
            PnSequence::from_chips(&[1, 3]).unwrap_err(),
            PnError::NotBipolar(3)
        );
    }

    #[test]
    fn correlation_outside_the_overlap_is_zero() {
        let pn = PnSequence::barker(7).unwrap();
        assert!(pn.aperiodic_autocorrelation(7).abs() < 1e-12);
        assert!(pn.aperiodic_autocorrelation(-9).abs() < 1e-12);
        assert!((pn.periodic_autocorrelation(7) - 7.0).abs() < 1e-12);
        assert!((pn.periodic_autocorrelation(-7) - 7.0).abs() < 1e-12);
    }
}
