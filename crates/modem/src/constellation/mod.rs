//! Constellations as tables (MODEM-PLAN §3.3): a point set is *data* — complex points plus
//! per-point bit labels — never match arms. Every linear and orthogonal entry demaps through
//! the one generic demapper in [`demap`], and the exotic tables in [`tables`] (cross-QAM,
//! star-QAM, non-uniform QAM, APSK) exist precisely to prove nothing special-cases "the"
//! constellation. This module therefore contains no specific standard's table: the
//! PAM/PSK/QAM/APSK generators are *functions returning* [`Constellation`], not new types.
//!
//! Construction normalises the table to mean symbol energy Es = 1 (the crate-root pulse
//! convention's counterpart: with unit-energy pulses and Es = 1, an Eb/N0 in `ber` means the
//! same thing for every entry). Callers hand in whatever integer grid is convenient — ±1/±3
//! for PAM, ring radii for APSK — and the stored table is the scaled one.

pub mod demap;
pub mod tables;

use std::fmt;

use num_complex::Complex;

/// Why a table was rejected. Construction happens at setup time, so this is a `Result`, not a
/// panic — a bad table from a config file must surface as an error, never take the engine down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstellationError {
    /// Point count must be 2^k with k ≥ 1 (one bit) and k ≤ 32 (labels are `u32`).
    SizeNotPowerOfTwo(usize),
    /// One label per point, in the same order.
    LabelCountMismatch { points: usize, labels: usize },
    /// A label repeats or exceeds k bits. Labels must be a permutation of 0..2^k — that is
    /// what guarantees every bit position has points on both sides, which is what makes the
    /// demapper total (see [`demap`]).
    BadLabel(u32),
    /// A table of all-zero points has no energy to normalise to.
    ZeroPower,
    /// A [`tables`] generator was asked for an order its family does not define — square QAM
    /// at an odd number of bits, cross-QAM outside {32, 128}, a star with a non-power-of-two
    /// ring count. Carries the family name because the order alone rarely says what was wrong.
    UnsupportedOrder { family: &'static str, m: u32 },
}

impl fmt::Display for ConstellationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SizeNotPowerOfTwo(n) => {
                write!(f, "constellation size {n} is not 2^k with 1 <= k <= 32")
            }
            Self::LabelCountMismatch { points, labels } => {
                write!(f, "{points} points but {labels} labels")
            }
            Self::BadLabel(l) => write!(f, "label {l:#b} repeats or does not fit the bit width"),
            Self::ZeroPower => write!(f, "all-zero constellation cannot be normalised"),
            Self::UnsupportedOrder { family, m } => {
                write!(f, "{family} is not defined at order {m}")
            }
        }
    }
}

impl std::error::Error for ConstellationError {}

/// A validated, energy-normalised point table: `points[i]` is transmitted for the bit pattern
/// `labels[i]`, and mean |point|² = 1. Immutable once built, so every invariant checked at
/// construction holds for the demapper's whole lifetime.
#[derive(Clone, Debug, PartialEq)]
pub struct Constellation {
    points: Vec<Complex<f32>>,
    labels: Vec<u32>,
    bits_per_symbol: usize,
}

impl Constellation {
    /// Builds from an arbitrary table. Validates size (2^k points), labels (a permutation of
    /// 0..2^k), and energy; then scales all points by one common factor so mean Es = 1 —
    /// scaling is uniform, so the table's *shape* (relative distances, ring ratios) is
    /// exactly the caller's. A table already at Es = 1 passes through unchanged up to f32
    /// rounding.
    ///
    /// # Errors
    /// See [`ConstellationError`].
    pub fn from_points(
        points: Vec<Complex<f32>>,
        labels: Vec<u32>,
    ) -> Result<Self, ConstellationError> {
        let n = points.len();
        if !n.is_power_of_two() || n < 2 || n.trailing_zeros() > 32 {
            return Err(ConstellationError::SizeNotPowerOfTwo(n));
        }
        if labels.len() != n {
            return Err(ConstellationError::LabelCountMismatch {
                points: n,
                labels: labels.len(),
            });
        }
        let mut seen = vec![false; n];
        for &label in &labels {
            match seen.get_mut(label as usize) {
                Some(slot) if !*slot => *slot = true,
                _ => return Err(ConstellationError::BadLabel(label)),
            }
        }
        // f64 accounting for the energy sum: a 1024-point table summed in f32 would lose the
        // low bits the tolerance tests read.
        let power = points
            .iter()
            .map(|p| f64::from(p.re) * f64::from(p.re) + f64::from(p.im) * f64::from(p.im))
            .sum::<f64>()
            / n as f64;
        // NaN (from an infinite input point) must land here too, hence the explicit form
        // rather than `power <= 0.0`.
        if !(power.is_finite() && power > 0.0) {
            return Err(ConstellationError::ZeroPower);
        }
        let scale = power.sqrt().recip();
        let points = points
            .into_iter()
            .map(|p| {
                Complex::new(
                    (f64::from(p.re) * scale) as f32,
                    (f64::from(p.im) * scale) as f32,
                )
            })
            .collect();
        Ok(Self {
            points,
            labels,
            bits_per_symbol: n.trailing_zeros() as usize,
        })
    }

    #[must_use]
    pub fn points(&self) -> &[Complex<f32>] {
        &self.points
    }

    #[must_use]
    pub fn labels(&self) -> &[u32] {
        &self.labels
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.bits_per_symbol
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        // Construction guarantees >= 2 points; here for the conventional len/is_empty pair.
        self.points.is_empty()
    }

    /// The hard decision: the label of the nearest point in Euclidean distance. Ties go to
    /// the earlier table entry — on a valid constellation a tie is a measure-zero event, so
    /// which side wins is not worth a tiebreak rule.
    #[must_use]
    pub fn hard_slice(&self, y: Complex<f32>) -> u32 {
        let mut best = 0usize;
        let mut best_d2 = f64::INFINITY;
        for (i, p) in self.points.iter().enumerate() {
            let dr = f64::from(y.re) - f64::from(p.re);
            let di = f64::from(y.im) - f64::from(p.im);
            let d2 = dr * dr + di * di;
            if d2 < best_d2 {
                best_d2 = d2;
                best = i;
            }
        }
        self.labels[best]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gray-labelled 4-PAM on the real axis, handed in as the ±1/±3 integer grid. The one
    /// specific table these tests use, and it lives here, not in the library (§3.3).
    fn gray_4pam() -> Constellation {
        Constellation::from_points(
            vec![
                Complex::new(-3.0, 0.0),
                Complex::new(-1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(3.0, 0.0),
            ],
            vec![0b00, 0b01, 0b11, 0b10],
        )
        .unwrap()
    }

    #[test]
    fn construction_normalises_to_unit_mean_energy() {
        let c = gray_4pam();
        let power = c
            .points()
            .iter()
            .map(|p| f64::from(p.re) * f64::from(p.re) + f64::from(p.im) * f64::from(p.im))
            .sum::<f64>()
            / c.len() as f64;
        assert!((power - 1.0).abs() < 1e-6, "mean Es {power}");
        // ±1/±3 with mean Es 5 scales by 1/√5.
        let a = 5f64.sqrt().recip();
        assert!((f64::from(c.points()[3].re) - 3.0 * a).abs() < 1e-7);
        assert_eq!(c.bits_per_symbol(), 2);
    }

    #[test]
    fn an_already_normalised_table_passes_through() {
        let x = std::f32::consts::FRAC_1_SQRT_2;
        let c = Constellation::from_points(
            vec![
                Complex::new(x, x),
                Complex::new(-x, x),
                Complex::new(-x, -x),
                Complex::new(x, -x),
            ],
            vec![0, 1, 3, 2],
        )
        .unwrap();
        assert!((c.points()[0].re - x).abs() < 1e-7);
    }

    #[test]
    fn hard_slice_picks_the_nearest_label() {
        let c = gray_4pam();
        // Decision boundaries sit at 0 and ±2/√5 ≈ ±0.894.
        assert_eq!(c.hard_slice(Complex::new(-2.0, 0.0)), 0b00);
        assert_eq!(c.hard_slice(Complex::new(-0.5, 0.1)), 0b01);
        assert_eq!(c.hard_slice(Complex::new(0.5, -0.1)), 0b11);
        assert_eq!(c.hard_slice(Complex::new(0.95, 0.0)), 0b10);
    }

    #[test]
    fn bad_tables_are_rejected_with_the_right_error() {
        let p = |n: usize| vec![Complex::new(1.0f32, 0.0); n];
        assert_eq!(
            Constellation::from_points(p(3), vec![0, 1, 2]).unwrap_err(),
            ConstellationError::SizeNotPowerOfTwo(3)
        );
        assert_eq!(
            Constellation::from_points(p(1), vec![0]).unwrap_err(),
            ConstellationError::SizeNotPowerOfTwo(1)
        );
        assert_eq!(
            Constellation::from_points(p(4), vec![0, 1]).unwrap_err(),
            ConstellationError::LabelCountMismatch {
                points: 4,
                labels: 2
            }
        );
        assert_eq!(
            Constellation::from_points(p(4), vec![0, 1, 2, 2]).unwrap_err(),
            ConstellationError::BadLabel(2)
        );
        assert_eq!(
            Constellation::from_points(p(4), vec![0, 1, 2, 4]).unwrap_err(),
            ConstellationError::BadLabel(4)
        );
        assert_eq!(
            Constellation::from_points(vec![Complex::new(0.0, 0.0); 2], vec![0, 1]).unwrap_err(),
            ConstellationError::ZeroPower
        );
    }
}
