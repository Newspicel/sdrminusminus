mod bank;
mod phase_mode;
mod transform;

use num_complex::Complex;

use crate::linalg::{CMat, LinalgError, MAX_ORDER};
use crate::special::SpecialError;

pub use bank::CovarianceBank;
pub use phase_mode::{BESSEL_FLOOR, PhaseMode};
pub use transform::{forward_backward, load_diagonal, smooth, smooth_diagonal};

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum CovarianceError {
    #[error(transparent)]
    Linalg(#[from] LinalgError),
    #[error(transparent)]
    Special(#[from] SpecialError),
    #[error("needs a line or circle")]
    NotStructured,
    #[error("FB needs a symmetric array")]
    NotSymmetric,
    #[error("too few elements for smoothing")]
    TooFewForSmoothing,
    #[error("Bessel null here")]
    BesselNull,
    #[error("fft size {0} is not a power of two in 64..=8192")]
    FftSize(usize),
    #[error("hop {0} is outside 1..=fft size")]
    Hop(usize),
    #[error("bins {0}..{1} are outside the bank")]
    Bins(usize, usize),
    #[error("lanes differ in length")]
    LaneLength,
}

pub struct SampleCovariance {
    order: usize,
    sum: [Complex<f64>; MAX_ORDER * MAX_ORDER],
    weight: f64,
    weight_sq: f64,
}

impl SampleCovariance {
    pub fn new(order: usize) -> Result<Self, CovarianceError> {
        if order == 0 || order > MAX_ORDER {
            return Err(LinalgError::Order(order).into());
        }
        Ok(Self {
            order,
            sum: [Complex::new(0.0, 0.0); MAX_ORDER * MAX_ORDER],
            weight: 0.0,
            weight_sq: 0.0,
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    pub fn reset(&mut self) {
        self.sum.fill(Complex::new(0.0, 0.0));
        self.weight = 0.0;
        self.weight_sq = 0.0;
    }

    pub fn decay(&mut self, factor: f32) {
        let factor = f64::from(factor);
        for value in &mut self.sum {
            *value *= factor;
        }
        self.weight *= factor;
        self.weight_sq *= factor * factor;
    }

    pub fn accumulate(&mut self, lanes: &[&[Complex<f32>]]) -> usize {
        if lanes.len() != self.order {
            return 0;
        }
        let len = lanes.iter().map(|lane| lane.len()).min().unwrap_or(0);
        let n = self.order;
        for (i, left) in lanes.iter().enumerate() {
            for (j, right) in lanes.iter().enumerate().skip(i) {
                self.sum[i * n + j] += cross_sum(&left[..len], &right[..len]);
            }
        }
        self.weight += len as f64;
        self.weight_sq += len as f64;
        len
    }

    #[must_use]
    pub const fn weight(&self) -> f64 {
        self.weight
    }

    #[must_use]
    pub fn effective_snapshots(&self) -> f64 {
        if self.weight_sq > 0.0 {
            self.weight * self.weight / self.weight_sq
        } else {
            0.0
        }
    }

    pub fn matrix(&self, out: &mut CMat) -> bool {
        let n = self.order;
        if out.resize(n).is_err() {
            return false;
        }
        if self.weight <= 0.0 {
            for i in 0..n {
                out.set(i, i, Complex::new(1.0, 0.0));
            }
            return false;
        }
        let scale = 1.0 / self.weight;
        for i in 0..n {
            for j in i..n {
                let value = self.sum[i * n + j] * scale;
                let value = Complex::new(value.re as f32, value.im as f32);
                out.set(i, j, value);
                out.set(j, i, value.conj());
            }
        }
        true
    }
}

fn cross_sum(left: &[Complex<f32>], right: &[Complex<f32>]) -> Complex<f64> {
    let mut partial = [Complex::new(0.0f32, 0.0); 4];
    let (left_blocks, left_tail) = left.as_chunks::<4>();
    let (right_blocks, right_tail) = right.as_chunks::<4>();
    for (a, b) in left_blocks.iter().zip(right_blocks) {
        for ((sum, x), y) in partial.iter_mut().zip(a).zip(b) {
            *sum += x * y.conj();
        }
    }
    let tail: Complex<f32> = left_tail
        .iter()
        .zip(right_tail)
        .map(|(a, b)| a * b.conj())
        .sum();
    partial
        .iter()
        .chain(std::iter::once(&tail))
        .map(|value| Complex::new(f64::from(value.re), f64::from(value.im)))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::XorShift32;

    fn random_lanes(lanes: usize, len: usize, seed: u32) -> Vec<Vec<Complex<f32>>> {
        let mut rng = XorShift32(seed);
        (0..lanes)
            .map(|_| {
                (0..len)
                    .map(|_| Complex::new(rng.next_f32(), rng.next_f32()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn upper_triangle_accumulation_matches_the_outer_product() {
        let lanes = random_lanes(5, 1003, 11);
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut covariance = SampleCovariance::new(5).unwrap();
        assert_eq!(covariance.accumulate(&views[..3]), 0);
        assert_eq!(covariance.accumulate(&views), 1003);
        let mut matrix = CMat::zeros(1).unwrap();
        assert!(covariance.matrix(&mut matrix));
        for i in 0..5 {
            for j in 0..5 {
                let want: Complex<f64> = lanes[i]
                    .iter()
                    .zip(&lanes[j])
                    .map(|(a, b)| {
                        let p = a * b.conj();
                        Complex::new(f64::from(p.re), f64::from(p.im))
                    })
                    .sum::<Complex<f64>>()
                    / 1003.0;
                let got = matrix.get(i, j);
                let got = Complex::new(f64::from(got.re), f64::from(got.im));
                assert!(
                    (got - want).norm() <= 1e-4 * want.norm().max(1e-3),
                    "{i},{j}"
                );
            }
        }
    }

    #[test]
    fn effective_snapshots_follow_decay() {
        let lanes = random_lanes(2, 1000, 3);
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut covariance = SampleCovariance::new(2).unwrap();
        covariance.accumulate(&views);
        assert!((covariance.effective_snapshots() - 1000.0).abs() < 1e-9);
        covariance.decay(0.5);
        covariance.accumulate(&views);
        assert!((covariance.weight() - 1500.0).abs() < 1e-9);
        assert!((covariance.effective_snapshots() - 1800.0).abs() < 1e-9);
    }

    #[test]
    fn an_empty_average_reports_the_identity() {
        let mut covariance = SampleCovariance::new(3).unwrap();
        let mut matrix = CMat::zeros(1).unwrap();
        assert!(!covariance.matrix(&mut matrix));
        assert_eq!(matrix, CMat::identity(3).unwrap());
        assert_eq!(covariance.effective_snapshots(), 0.0);
        let lanes = random_lanes(3, 16, 5);
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        covariance.accumulate(&views);
        covariance.reset();
        assert!(!covariance.matrix(&mut matrix));
        assert!(SampleCovariance::new(0).is_err());
        assert!(SampleCovariance::new(17).is_err());
    }
}
