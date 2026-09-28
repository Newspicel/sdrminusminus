mod cholesky;
mod jacobi;
mod qr;
mod roots;
mod schur;

use num_complex::Complex;

pub use cholesky::Cholesky;
pub use jacobi::{Eigen, HermitianEigen};
pub use qr::Qr;
pub use roots::Roots;
pub use schur::GeneralEigen;

pub const MAX_ORDER: usize = 16;
pub const MAX_SOLVE_ORDER: usize = 512;
pub const MAX_SCHUR_ORDER: usize = 32;
pub const MAX_POLY_DEGREE: usize = 2 * MAX_ORDER - 2;

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum LinalgError {
    #[error("matrix order {0} is outside what this solver handles")]
    Order(usize),
    #[error("matrix is not positive definite at row {0}")]
    NotPositiveDefinite(usize),
    #[error("matrix is rank deficient at column {0}")]
    RankDeficient(usize),
    #[error("matrix holds a non-finite value")]
    NonFinite,
    #[error("no convergence after {0} iterations")]
    NotConverged(u32),
    #[error("polynomial has no roots to find")]
    Degenerate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CMat {
    n: usize,
    data: [Complex<f32>; MAX_ORDER * MAX_ORDER],
}

impl CMat {
    pub fn zeros(n: usize) -> Result<Self, LinalgError> {
        check_order(n)?;
        Ok(Self {
            n,
            data: [Complex::new(0.0, 0.0); MAX_ORDER * MAX_ORDER],
        })
    }

    pub fn identity(n: usize) -> Result<Self, LinalgError> {
        let mut matrix = Self::zeros(n)?;
        for i in 0..n {
            matrix.set(i, i, Complex::new(1.0, 0.0));
        }
        Ok(matrix)
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.n
    }

    #[must_use]
    pub fn get(&self, row: usize, col: usize) -> Complex<f32> {
        debug_assert!(row < self.n && col < self.n);
        self.data[row * self.n + col]
    }

    pub fn set(&mut self, row: usize, col: usize, value: Complex<f32>) {
        debug_assert!(row < self.n && col < self.n);
        self.data[row * self.n + col] = value;
    }

    pub fn add(&mut self, row: usize, col: usize, value: Complex<f32>) {
        debug_assert!(row < self.n && col < self.n);
        self.data[row * self.n + col] += value;
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Complex<f32>] {
        &self.data[..self.n * self.n]
    }

    pub fn as_mut_slice(&mut self) -> &mut [Complex<f32>] {
        &mut self.data[..self.n * self.n]
    }

    pub fn resize(&mut self, n: usize) -> Result<(), LinalgError> {
        check_order(n)?;
        self.n = n;
        self.data.fill(Complex::new(0.0, 0.0));
        Ok(())
    }

    pub fn fill_zero(&mut self) {
        self.as_mut_slice().fill(Complex::new(0.0, 0.0));
    }

    #[must_use]
    pub fn trace_re(&self) -> f32 {
        (0..self.n).map(|i| self.get(i, i).re).sum()
    }

    #[must_use]
    pub fn max_abs(&self) -> f32 {
        self.as_slice()
            .iter()
            .map(|value| value.norm())
            .fold(0.0, f32::max)
    }

    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.as_slice().iter().all(|value| value.is_finite())
    }

    pub fn scale(&mut self, factor: f32) {
        for value in self.as_mut_slice() {
            *value *= factor;
        }
    }

    #[must_use]
    pub fn quad(&self, a: &[Complex<f32>]) -> f32 {
        let n = self.n;
        let a = &a[..n];
        let mut diagonal = 0.0f32;
        let mut cross = Complex::new(0.0f32, 0.0);
        for (i, &ai) in a.iter().enumerate() {
            let row = &self.data[i * n..(i + 1) * n];
            diagonal += row[i].re * ai.norm_sqr();
            let mut sum = Complex::new(0.0f32, 0.0);
            for (&m, &aj) in row[i + 1..].iter().zip(&a[i + 1..]) {
                sum += m * aj;
            }
            cross += ai.conj() * sum;
        }
        diagonal + 2.0 * cross.re
    }

    pub fn copy_block(&self, start: usize, len: usize, out: &mut CMat) -> Result<(), LinalgError> {
        if len == 0 || start.checked_add(len).is_none_or(|end| end > self.n) {
            return Err(LinalgError::Order(len));
        }
        out.resize(len)?;
        for row in 0..len {
            for col in 0..len {
                out.set(row, col, self.get(start + row, start + col));
            }
        }
        Ok(())
    }

    pub fn permuted(&self, map: &[usize], out: &mut CMat) -> Result<(), LinalgError> {
        if let Some(&bad) = map.iter().find(|&&index| index >= self.n) {
            return Err(LinalgError::Order(bad));
        }
        out.resize(map.len())?;
        for (row, &from_row) in map.iter().enumerate() {
            for (col, &from_col) in map.iter().enumerate() {
                out.set(row, col, self.get(from_row, from_col));
            }
        }
        Ok(())
    }
}

const fn check_order(n: usize) -> Result<(), LinalgError> {
    if n == 0 || n > MAX_ORDER {
        return Err(LinalgError::Order(n));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod testing {
    use num_complex::Complex;

    use super::CMat;
    use crate::testutil::XorShift32;

    pub(crate) fn hermitian(order: usize, entries: &[(usize, usize, f32, f32)]) -> CMat {
        let mut matrix = CMat::zeros(order).unwrap();
        for &(row, col, re, im) in entries {
            matrix.set(row, col, Complex::new(re, im));
            matrix.set(col, row, Complex::new(re, -im));
        }
        matrix
    }

    pub(crate) fn random_vector(rng: &mut XorShift32, len: usize) -> Vec<Complex<f32>> {
        (0..len)
            .map(|_| Complex::new(rng.next_f32(), rng.next_f32()))
            .collect()
    }

    pub(crate) fn outer_sum(order: usize, vectors: &[Vec<Complex<f32>>]) -> CMat {
        let mut matrix = CMat::zeros(order).unwrap();
        for x in vectors {
            for row in 0..order {
                for col in 0..order {
                    matrix.add(row, col, x[row] * x[col].conj());
                }
            }
        }
        matrix
    }

    pub(crate) fn random_hpd(order: usize, seed: u32) -> CMat {
        let mut rng = XorShift32(seed);
        let vectors: Vec<_> = (0..order + 3)
            .map(|_| random_vector(&mut rng, order))
            .collect();
        outer_sum(order, &vectors)
    }

    pub(crate) fn mat_vec(matrix: &CMat, x: &[Complex<f32>]) -> Vec<Complex<f32>> {
        let n = matrix.order();
        (0..n)
            .map(|row| (0..n).map(|col| matrix.get(row, col) * x[col]).sum())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::testing::{hermitian, mat_vec, random_hpd};
    use super::*;

    #[test]
    fn an_order_outside_the_matrix_is_refused() {
        assert_eq!(CMat::zeros(0), Err(LinalgError::Order(0)));
        assert_eq!(
            CMat::zeros(MAX_ORDER + 1),
            Err(LinalgError::Order(MAX_ORDER + 1))
        );
        let mut matrix = CMat::identity(3).unwrap();
        assert_eq!(matrix.resize(17), Err(LinalgError::Order(17)));
        assert_eq!(matrix.order(), 3);
    }

    #[test]
    fn identity_has_the_order_as_trace() {
        let matrix = CMat::identity(5).unwrap();
        assert_eq!(matrix.trace_re(), 5.0);
        assert_eq!(matrix.as_slice().len(), 25);
        assert_eq!(matrix.max_abs(), 1.0);
    }

    #[test]
    fn quad_matches_the_explicit_product() {
        let matrix = random_hpd(6, 7);
        let a: Vec<_> = (0..6)
            .map(|i| Complex::new(0.3 * i as f32 - 0.7, 0.2 - 0.1 * i as f32))
            .collect();
        let product = mat_vec(&matrix, &a);
        let want: f32 = a
            .iter()
            .zip(&product)
            .map(|(ai, pi)| (ai.conj() * pi).re)
            .sum();
        assert!((matrix.quad(&a) - want).abs() < 1e-4 * want.abs());
    }

    #[test]
    fn copy_block_and_permuted_pick_the_right_entries() {
        let matrix = hermitian(
            4,
            &[
                (0, 0, 1.0, 0.0),
                (1, 1, 2.0, 0.0),
                (2, 2, 3.0, 0.0),
                (3, 3, 4.0, 0.0),
                (1, 2, 0.5, 0.25),
                (0, 3, -1.0, 2.0),
            ],
        );
        let mut block = CMat::zeros(1).unwrap();
        matrix.copy_block(1, 2, &mut block).unwrap();
        assert_eq!(block.order(), 2);
        assert_eq!(block.get(0, 1), Complex::new(0.5, 0.25));
        assert_eq!(block.get(1, 1), Complex::new(3.0, 0.0));
        assert_eq!(
            matrix.copy_block(3, 2, &mut block),
            Err(LinalgError::Order(2))
        );
        assert_eq!(
            matrix.copy_block(usize::MAX, 2, &mut block),
            Err(LinalgError::Order(2))
        );
        let mut swapped = CMat::zeros(1).unwrap();
        matrix.permuted(&[3, 0], &mut swapped).unwrap();
        assert_eq!(swapped.get(0, 0), Complex::new(4.0, 0.0));
        assert_eq!(swapped.get(1, 0), Complex::new(-1.0, 2.0));
        assert_eq!(
            matrix.permuted(&[0, 4], &mut swapped),
            Err(LinalgError::Order(4))
        );
    }

    #[test]
    fn non_finite_entries_are_seen() {
        let mut matrix = CMat::identity(2).unwrap();
        assert!(matrix.is_finite());
        matrix.set(1, 0, Complex::new(f32::NAN, 0.0));
        assert!(!matrix.is_finite());
    }
}
