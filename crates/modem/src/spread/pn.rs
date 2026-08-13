//! Spreading sequences as data (MODEM-PLAN §3.3, the rule that makes cross-QAM a table rather
//! than a match arm, applied to the chip domain): a [`PnSequence`] is a list of ±1 chips, and the
//! correlator in [`dsss`](super::dsss) never asks which family produced it.
//!
//! Two families are generated here because two properties are wanted and no one sequence has
//! both:
//!
//! - **Barker words** ([`PnSequence::barker`]) have the best *aperiodic* autocorrelation any
//!   binary sequence of their length can: every off-peak sidelobe is at most 1 in magnitude. That
//!   is the property an acquisition search reads — a receiver hunting a burst correlates a
//!   partial, unaligned window, not a full period — which is why 802.11b spreads its 1 and 2
//!   Mbit/s rates with the length-11 word rather than something longer.
//! - **Maximal-length sequences** ([`PnSequence::maximal_length`]) have the best *periodic*
//!   autocorrelation: exactly −1 at every nonzero shift, flat by construction rather than by
//!   search, at every length 2^k − 1. That is what a continuously-keyed spread link reads, and it
//!   is the only family that scales — Barker words are conjectured not to exist past 13.
//!
//! Both properties are asserted by test over every length this module generates, which is what
//! makes them the module's specification rather than its folklore.
//!
//! **Chips are ±1 reals.** A complex spreading code (the QPSK-spread family) would be a
//! different table with the same interface; nothing here forbids one, and the correlator is
//! written against `f32` chips so adding it later touches this file only. The catalog's entries
//! spread bipolar, which is what DSSS means in every standard §6 names.

use std::fmt;

/// Why a sequence was refused. Construction is setup-time, so this is a `Result`: a bad
/// sequence out of a configuration file must surface as an error, never take an engine down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PnError {
    /// A chip that is neither +1 nor −1. The correlator's processing gain is `N` only because
    /// every chip carries unit energy; a 0 or a 2 would silently make the stated gain a lie.
    NotBipolar(i8),
    /// An empty sequence spreads nothing.
    Empty,
    /// No Barker word of this length is known (they exist only at 2, 3, 4, 5, 7, 11 and 13, and
    /// none longer has ever been found).
    NoBarkerWord(usize),
    /// [`PnSequence::maximal_length`] outside the degrees this module carries a primitive
    /// polynomial for.
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

/// Degrees [`PnSequence::maximal_length`] carries a primitive polynomial for. The upper end is
/// where a period stops being a spreading factor and starts being a frame.
pub const MAX_LFSR_DEGREE: u32 = 16;

/// Primitive polynomials over GF(2) as tap masks, indexed by `degree − 2`. **Bit `i` of a mask is
/// the coefficient of `x^i`**, the `x^d` term implicit — so the register's recurrence is
/// `x_{n+d} = Σ_i mask_i · x_{n+i}` and a mask reads directly as the polynomial's lower terms.
/// (The convention is worth spelling out: reading the same bits as *stage numbers counted from
/// the other end* turns x³+x+1 into x³+x²+x, which is reducible and produces a period-4 sequence
/// that still looks pseudo-random.)
///
/// These are the conventional minimum-weight primitives (Peterson & Weldon, *Error-Correcting
/// Codes*, App. C). Their primitivity is not asserted from the table but *measured*, by the
/// full-period and periodic-autocorrelation tests below — which is the only kind of trust a
/// transcribed table of constants can earn.
const PRIMITIVE_TAPS: [u32; (MAX_LFSR_DEGREE - 1) as usize] = [
    0b11,               // x² + x + 1
    0b011,              // x³ + x + 1
    0b0011,             // x⁴ + x + 1
    0b00101,            // x⁵ + x² + 1
    0b000011,           // x⁶ + x + 1
    0b0001001,          // x⁷ + x³ + 1
    0b00011101,         // x⁸ + x⁴ + x³ + x² + 1
    0b000010001,        // x⁹ + x⁴ + 1
    0b0000001001,       // x¹⁰ + x³ + 1
    0b00000000101,      // x¹¹ + x² + 1
    0b000001010011,     // x¹² + x⁶ + x⁴ + x + 1
    0b0000000011011,    // x¹³ + x⁴ + x³ + x + 1
    0b00010001000011,   // x¹⁴ + x¹⁰ + x⁶ + x + 1
    0b000000000000011,  // x¹⁵ + x + 1
    0b0110100000000001, // x¹⁶ + x¹⁴ + x¹³ + x¹¹ + 1
];

/// The Barker words, longest first. Length 11 is written in IEEE 802.11's own order (§18.4.6.3),
/// which is the negated reverse of the textbook word — both are Barker, since negation and
/// reversal preserve an aperiodic autocorrelation's magnitudes, and the standard's ordering is
/// the one a reader will be comparing against.
const BARKER: [(usize, &[i8]); 7] = [
    (13, &[1, 1, 1, 1, 1, -1, -1, 1, 1, -1, 1, -1, 1]),
    (11, &[1, -1, 1, 1, -1, 1, 1, 1, -1, -1, -1]),
    (7, &[1, 1, 1, -1, -1, 1, -1]),
    (5, &[1, 1, 1, -1, 1]),
    (4, &[1, 1, -1, 1]),
    (3, &[1, 1, -1]),
    (2, &[1, -1]),
];

/// A validated bipolar spreading sequence. Immutable once built, so the ±1 invariant every
/// processing-gain statement rests on holds for the correlator's whole lifetime.
#[derive(Clone, Debug, PartialEq)]
pub struct PnSequence {
    chips: Vec<f32>,
}

impl PnSequence {
    /// From an explicit ±1 chip list — the arbitrary-table constructor (§3.1: "arbitrary PN +
    /// correlator").
    ///
    /// # Errors
    /// [`PnError::Empty`], [`PnError::NotBipolar`].
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

    /// The Barker word of length `n`, for `n` ∈ {2, 3, 4, 5, 7, 11, 13}.
    ///
    /// # Errors
    /// [`PnError::NoBarkerWord`] — and the list is complete: no Barker sequence longer than 13
    /// has ever been found, and none of odd length is possible past it.
    pub fn barker(n: usize) -> Result<Self, PnError> {
        let word = BARKER
            .iter()
            .find(|(len, _)| *len == n)
            .ok_or(PnError::NoBarkerWord(n))?;
        Self::from_chips(word.1)
    }

    /// A maximal-length sequence of period 2^`degree` − 1, from a Fibonacci LFSR run through a
    /// full cycle and mapped `0 → +1`, `1 → −1`.
    ///
    /// The all-zero state is the register's one fixed point and is unreachable from any other,
    /// so the run starts from all-ones; every nonzero start produces a cyclic shift of the same
    /// sequence, which is why the constructor takes no seed.
    ///
    /// # Errors
    /// [`PnError::NoPrimitivePolynomial`] outside `2..=`[`MAX_LFSR_DEGREE`].
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
            // The output is the register's low bit; feedback is the parity of the tapped stages.
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
        // Construction guarantees at least one chip; here for the conventional len/is_empty pair.
        self.chips.is_empty()
    }

    /// The spreading factor's processing gain in dB, `10·log₁₀(N)` — MODEM-PLAN §6's stated
    /// reference for this entry. It is the ratio by which despreading improves the
    /// signal-to-interference ratio against interference narrow compared with the chip rate, and
    /// `tests/spread.rs` measures it against exactly this number rather than quoting it.
    #[must_use]
    pub fn processing_gain_db(&self) -> f64 {
        10.0 * (self.len() as f64).log10()
    }

    /// Aperiodic autocorrelation at `shift`: `Σ c[n]·c[n + shift]` over the overlap only, the
    /// quantity a *partial* correlation reads. This is the one a burst search sees, and the one
    /// Barker words minimise.
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

    /// Periodic autocorrelation at `shift`: `Σ c[n]·c[(n + shift) mod N]`, the quantity a
    /// receiver already locked to the period reads. Maximal-length sequences make this exactly
    /// −1 at every nonzero shift.
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

    /// The Barker property, which *is* the family's definition: every aperiodic sidelobe is at
    /// most 1 in magnitude. Asserted for every word the module generates, so a transcription
    /// slip in the table is caught here and not three modules downstream.
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

    /// 802.11b's word, chip for chip (§18.4.6.3), and its relation to the textbook one: the
    /// standard's is the negated reverse, which is why both are Barker.
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

    /// The maximal-length property, and the two halves of it that matter: the period is the full
    /// 2^k − 1 (so the tabulated polynomial really is primitive) and the periodic autocorrelation
    /// is exactly −1 everywhere off the peak.
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
            // Balance follows from the same property and is what makes the sequence spectrally
            // flat: one more −1 than +1 over a period.
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

    /// An m-sequence is a shift of itself under the shift-and-add property; more usefully here,
    /// no two tabulated degrees produce the same sequence, so the table has no duplicate row.
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

    /// Beyond a sequence's own length there is nothing left to overlap, and the aperiodic form
    /// must say zero rather than index out of bounds.
    #[test]
    fn correlation_outside_the_overlap_is_zero() {
        let pn = PnSequence::barker(7).unwrap();
        assert!(pn.aperiodic_autocorrelation(7).abs() < 1e-12);
        assert!(pn.aperiodic_autocorrelation(-9).abs() < 1e-12);
        // The periodic form wraps instead, so a whole-period shift is the peak again.
        assert!((pn.periodic_autocorrelation(7) - 7.0).abs() < 1e-12);
        assert!((pn.periodic_autocorrelation(-7) - 7.0).abs() < 1e-12);
    }
}
