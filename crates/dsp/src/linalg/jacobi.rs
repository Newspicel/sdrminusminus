use num_complex::Complex;

use super::{CMat, LinalgError, MAX_ORDER};

const SWEEPS: u32 = 30;
const REL_TOL: f32 = 4.0 * f32::EPSILON;
const ACCEPT_TOL: f32 = 1e-8;

#[derive(Clone, Debug)]
pub struct Eigen {
    order: usize,
    values: [f32; MAX_ORDER],
    vectors: [Complex<f32>; MAX_ORDER * MAX_ORDER],
    sweeps: u32,
}

impl Eigen {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            order: 0,
            values: [0.0; MAX_ORDER],
            vectors: [Complex::new(0.0, 0.0); MAX_ORDER * MAX_ORDER],
            sweeps: 0,
        }
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values[..self.order]
    }

    #[must_use]
    pub fn vector(&self, index: usize) -> &[Complex<f32>] {
        let n = self.order;
        &self.vectors[..n * n][index * n..(index + 1) * n]
    }

    #[must_use]
    pub const fn sweeps(&self) -> u32 {
        self.sweeps
    }
}

impl Default for Eigen {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HermitianEigen {
    order: usize,
    a: CMat,
    v: CMat,
}

impl HermitianEigen {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        Ok(Self {
            order,
            a: CMat::zeros(order)?,
            v: CMat::identity(order)?,
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    pub fn solve(&mut self, matrix: &CMat, out: &mut Eigen) -> Result<(), LinalgError> {
        if matrix.order() != self.order {
            return Err(LinalgError::Order(matrix.order()));
        }
        if !matrix.is_finite() {
            return Err(LinalgError::NonFinite);
        }
        let n = self.order;
        let scale = matrix.max_abs();
        if !scale.is_finite() {
            return Err(LinalgError::NonFinite);
        }
        self.a.clone_from(matrix);
        self.v.resize(n)?;
        for i in 0..n {
            self.v.set(i, i, Complex::new(1.0, 0.0));
        }
        let sweeps = if scale > 0.0 {
            for value in self.a.as_mut_slice() {
                *value /= scale;
            }
            self.diagonalise()?
        } else {
            0
        };
        self.write_sorted(scale, sweeps, out);
        Ok(())
    }

    fn diagonalise(&mut self) -> Result<u32, LinalgError> {
        for sweep in 0..SWEEPS {
            let (off, frob) = self.off_and_frobenius();
            if off <= REL_TOL * REL_TOL * frob {
                return Ok(sweep);
            }
            for p in 0..self.order {
                for q in (p + 1)..self.order {
                    self.rotate(p, q);
                }
            }
        }
        let (off, frob) = self.off_and_frobenius();
        if off > ACCEPT_TOL * frob {
            return Err(LinalgError::NotConverged(SWEEPS));
        }
        Ok(SWEEPS)
    }

    fn off_and_frobenius(&self) -> (f32, f32) {
        let n = self.order;
        let mut off = 0.0f32;
        let mut frob = 0.0f32;
        for p in 0..n {
            for q in 0..n {
                let magnitude = self.a.get(p, q).norm_sqr();
                frob += magnitude;
                if q > p {
                    off += magnitude;
                }
            }
        }
        (off, frob)
    }

    fn rotate(&mut self, p: usize, q: usize) {
        let apq = self.a.get(p, q);
        let magnitude = apq.norm();
        if magnitude == 0.0 {
            return;
        }
        let n = self.order;
        let app = self.a.get(p, p).re;
        let aqq = self.a.get(q, q).re;
        let theta = 0.5 * (2.0 * magnitude).atan2(app - aqq);
        let (s, c) = theta.sin_cos();
        let phase = apq / magnitude;
        let forward = phase * s;
        let backward = phase.conj() * s;
        for row in 0..n {
            let ap = self.a.get(row, p);
            let aq = self.a.get(row, q);
            self.a.set(row, p, ap * c + aq * backward);
            self.a.set(row, q, aq * c - ap * forward);
        }
        for col in 0..n {
            let ap = self.a.get(p, col);
            let aq = self.a.get(q, col);
            self.a.set(p, col, ap * c + aq * forward);
            self.a.set(q, col, aq * c - ap * backward);
        }
        for row in 0..n {
            let vp = self.v.get(row, p);
            let vq = self.v.get(row, q);
            self.v.set(row, p, vp * c + vq * backward);
            self.v.set(row, q, vq * c - vp * forward);
        }
        self.a.set(p, q, Complex::new(0.0, 0.0));
        self.a.set(q, p, Complex::new(0.0, 0.0));
    }

    fn write_sorted(&self, scale: f32, sweeps: u32, out: &mut Eigen) {
        let n = self.order;
        let mut index = [0usize; MAX_ORDER];
        for (slot, value) in index.iter_mut().enumerate().take(n) {
            *value = slot;
        }
        for i in 1..n {
            let mut j = i;
            while j > 0
                && self.a.get(index[j - 1], index[j - 1]).re > self.a.get(index[j], index[j]).re
            {
                index.swap(j - 1, j);
                j -= 1;
            }
        }
        out.order = n;
        out.sweeps = sweeps;
        for (slot, &source) in index.iter().enumerate().take(n) {
            out.values[slot] = scale * self.a.get(source, source).re;
            let column = &mut out.vectors[slot * n..(slot + 1) * n];
            for (row, value) in column.iter_mut().enumerate() {
                *value = self.v.get(row, source);
            }
            let norm = column
                .iter()
                .map(|value| value.norm_sqr())
                .sum::<f32>()
                .sqrt();
            if norm > 0.0 {
                let inverse = norm.recip();
                for value in column.iter_mut() {
                    *value *= inverse;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::linalg::testing::{hermitian, mat_vec, random_hpd};

    fn solve(matrix: &CMat) -> Eigen {
        let mut solver = HermitianEigen::new(matrix.order()).unwrap();
        let mut eigen = Eigen::new();
        solver.solve(matrix, &mut eigen).unwrap();
        eigen
    }

    fn residual(matrix: &CMat, eigen: &Eigen, index: usize) -> f32 {
        let vector = eigen.vector(index);
        let value = eigen.values()[index];
        mat_vec(matrix, vector)
            .iter()
            .zip(vector)
            .map(|(av, v)| (av - v * value).norm_sqr())
            .sum::<f32>()
            .sqrt()
    }

    fn frobenius(matrix: &CMat) -> f32 {
        matrix
            .as_slice()
            .iter()
            .map(|value| value.norm_sqr())
            .sum::<f32>()
            .sqrt()
    }

    fn dense_five() -> CMat {
        hermitian(
            5,
            &[
                (0, 0, 4.0, 0.0),
                (0, 1, 1.0, 2.0),
                (0, 2, -0.5, 0.3),
                (0, 3, 0.2, -0.1),
                (0, 4, 0.9, 0.4),
                (1, 1, 3.0, 0.0),
                (1, 2, 0.7, -1.1),
                (1, 3, -0.4, 0.6),
                (1, 4, 0.1, -0.3),
                (2, 2, -2.0, 0.0),
                (2, 3, 1.3, 0.2),
                (2, 4, -0.8, 0.5),
                (3, 3, 5.0, 0.0),
                (3, 4, 0.6, 0.6),
                (4, 4, 9.0, 0.0),
            ],
        )
    }

    fn scaled(matrix: &CMat, factor: f32) -> CMat {
        let mut out = matrix.clone();
        out.scale(factor);
        out
    }

    #[test]
    fn jacobi_recovers_eigenpairs_of_a_matrix_scaled_to_1e_minus_12() {
        let matrix = scaled(&dense_five(), 1e-12);
        let eigen = solve(&matrix);
        let norm = frobenius(&matrix);
        for index in 0..5 {
            assert!(
                residual(&matrix, &eigen, index) / norm < 1e-4,
                "eigenpair {index}"
            );
        }
        let identity_like = (0..5).all(|index| {
            eigen
                .vector(index)
                .iter()
                .filter(|value| value.norm() > 1e-3)
                .count()
                == 1
        });
        assert!(!identity_like);
        assert!(eigen.sweeps() > 0);
    }

    #[test]
    fn jacobi_is_scale_invariant() {
        let base = dense_five();
        let reference = solve(&base);
        for factor in [1e-9f32, 1e9] {
            let eigen = solve(&scaled(&base, factor));
            for index in 0..5 {
                let want = reference.values()[index] * factor;
                let got = eigen.values()[index];
                assert!((got - want).abs() <= 1e-5 * want.abs(), "{got} vs {want}");
                let overlap: Complex<f32> = reference
                    .vector(index)
                    .iter()
                    .zip(eigen.vector(index))
                    .map(|(a, b)| a.conj() * b)
                    .sum();
                assert!(
                    overlap.norm() > 0.9999,
                    "vector {index}: {}",
                    overlap.norm()
                );
            }
        }
    }

    #[test]
    fn jacobi_zero_matrix_gives_zero_values() {
        let eigen = solve(&CMat::zeros(4).unwrap());
        assert_eq!(eigen.values(), &[0.0; 4]);
        assert_eq!(eigen.vector(2)[2], Complex::new(1.0, 0.0));
    }

    #[test]
    fn a_subnormal_matrix_still_diagonalises() {
        let matrix = scaled(&scaled(&dense_five(), 1e-20), 1e-20);
        let eigen = solve(&matrix);
        let reference = solve(&dense_five());
        for index in 0..5 {
            let want = reference.values()[index] * 1e-40;
            assert!((eigen.values()[index] - want).abs() <= 1e-2 * want.abs());
        }
    }

    #[test]
    fn jacobi_refuses_non_finite_input() {
        let mut matrix = CMat::identity(3).unwrap();
        matrix.set(0, 2, Complex::new(f32::NAN, 0.0));
        let mut solver = HermitianEigen::new(3).unwrap();
        let mut eigen = Eigen::new();
        assert_eq!(
            solver.solve(&matrix, &mut eigen),
            Err(LinalgError::NonFinite)
        );
    }

    #[test]
    fn a_magnitude_beyond_f32_is_refused() {
        let mut matrix = CMat::identity(2).unwrap();
        matrix.set(0, 0, Complex::new(3e38, 3e38));
        let mut solver = HermitianEigen::new(2).unwrap();
        let mut eigen = Eigen::new();
        assert_eq!(
            solver.solve(&matrix, &mut eigen),
            Err(LinalgError::NonFinite)
        );
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn a_vector_past_the_order_is_not_read() {
        let eigen = solve(&CMat::identity(3).unwrap());
        let _ = eigen.vector(3);
    }

    #[test]
    fn jacobi_refuses_a_matrix_of_another_order() {
        let mut solver = HermitianEigen::new(3).unwrap();
        let mut eigen = Eigen::new();
        assert_eq!(
            solver.solve(&CMat::identity(4).unwrap(), &mut eigen),
            Err(LinalgError::Order(4))
        );
        assert!(HermitianEigen::new(0).is_err());
        assert!(HermitianEigen::new(MAX_ORDER + 1).is_err());
    }

    #[test]
    fn a_diagonal_matrix_comes_back_sorted() {
        let matrix = hermitian(3, &[(0, 0, 5.0, 0.0), (1, 1, -2.0, 0.0), (2, 2, 1.0, 0.0)]);
        let eigen = solve(&matrix);
        assert_eq!(eigen.values().len(), 3);
        assert!((eigen.values()[0] + 2.0).abs() < 1e-4);
        assert!((eigen.values()[1] - 1.0).abs() < 1e-4);
        assert!((eigen.values()[2] - 5.0).abs() < 1e-4);
    }

    #[test]
    fn a_known_hermitian_matrix_matches_its_eigenvalues() {
        let matrix = hermitian(2, &[(0, 0, 2.0, 0.0), (0, 1, 0.0, -1.0), (1, 1, 2.0, 0.0)]);
        let eigen = solve(&matrix);
        assert!(
            (eigen.values()[0] - 1.0).abs() < 1e-4,
            "{:?}",
            eigen.values()
        );
        assert!(
            (eigen.values()[1] - 3.0).abs() < 1e-4,
            "{:?}",
            eigen.values()
        );
        for index in 0..2 {
            assert!(residual(&matrix, &eigen, index) < 1e-4);
        }
    }

    #[test]
    fn every_eigenpair_of_a_dense_matrix_satisfies_its_own_equation() {
        let matrix = hermitian(
            4,
            &[
                (0, 0, 4.0, 0.0),
                (0, 1, 1.0, 2.0),
                (0, 2, -0.5, 0.3),
                (0, 3, 0.2, -0.1),
                (1, 1, 3.0, 0.0),
                (1, 2, 0.7, -1.1),
                (1, 3, -0.4, 0.6),
                (2, 2, 2.0, 0.0),
                (2, 3, 1.3, 0.2),
                (3, 3, 5.0, 0.0),
            ],
        );
        let eigen = solve(&matrix);
        for index in 0..4 {
            assert!(
                residual(&matrix, &eigen, index) < 1e-3,
                "eigenpair {index} does not satisfy A v = λ v"
            );
            let length: f32 = eigen.vector(index).iter().map(Complex::norm_sqr).sum();
            assert!(
                (length - 1.0).abs() < 1e-3,
                "eigenvector {index} is not unit"
            );
        }
        assert!(eigen.values().windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn sixteen_lanes_converge_to_orthonormal_eigenpairs() {
        let matrix = random_hpd(16, 99);
        let eigen = solve(&matrix);
        let norm = frobenius(&matrix);
        for index in 0..16 {
            assert!(
                residual(&matrix, &eigen, index) / norm < 1e-5,
                "eigenpair {index}"
            );
            for other in 0..index {
                let overlap: Complex<f32> = eigen
                    .vector(index)
                    .iter()
                    .zip(eigen.vector(other))
                    .map(|(a, b)| a.conj() * b)
                    .sum();
                assert!(overlap.norm() < 1e-5, "vectors {index} and {other}");
            }
        }
        assert!(eigen.sweeps() < SWEEPS);
    }
}
