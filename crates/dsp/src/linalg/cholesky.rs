use num_complex::Complex;

use super::{CMat, LinalgError, MAX_ORDER, MAX_SOLVE_ORDER};

const LOADING_FLOOR: f32 = 1e-6;
const LOADING_STEPS: i32 = 6;
const PIVOT_TOL: f32 = 16.0 * f32::EPSILON;

#[derive(Clone, Debug)]
pub struct Cholesky {
    capacity: usize,
    order: usize,
    l: Vec<Complex<f32>>,
}

impl Cholesky {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        if order == 0 || order > MAX_SOLVE_ORDER {
            return Err(LinalgError::Order(order));
        }
        Ok(Self {
            capacity: order,
            order,
            l: vec![Complex::new(0.0, 0.0); order * order],
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    pub fn factor(&mut self, matrix: &[Complex<f32>]) -> Result<(), LinalgError> {
        let n = matrix.len().isqrt();
        if n * n != matrix.len() {
            return Err(LinalgError::Order(matrix.len()));
        }
        self.decompose(n, |row, col| matrix[row * n + col], 0.0)
    }

    pub fn factor_cmat(&mut self, matrix: &CMat) -> Result<(), LinalgError> {
        self.decompose(matrix.order(), |row, col| matrix.get(row, col), 0.0)
    }

    pub fn factor_loaded(&mut self, matrix: &CMat, base: f32) -> Result<f32, LinalgError> {
        let n = matrix.order();
        let mean = matrix.trace_re() / n as f32;
        if !mean.is_finite() {
            return Err(LinalgError::NonFinite);
        }
        let first = base * mean;
        let floor = first.max(LOADING_FLOOR * mean);
        let mut last = LinalgError::NotPositiveDefinite(0);
        for step in 0..=LOADING_STEPS {
            let loading = if step == 0 {
                first
            } else {
                floor * 10f32.powi(step)
            };
            match self.decompose(n, |row, col| matrix.get(row, col), loading) {
                Ok(()) => return Ok(loading),
                Err(LinalgError::NotPositiveDefinite(row)) => {
                    last = LinalgError::NotPositiveDefinite(row);
                }
                Err(other) => return Err(other),
            }
        }
        Err(last)
    }

    pub fn solve(&self, b: &mut [Complex<f32>]) {
        let b = &mut b[..self.order];
        self.forward(b);
        self.backward(b);
    }

    pub fn solve_into(&self, b: &[Complex<f32>], out: &mut [Complex<f32>]) {
        let n = self.order;
        out[..n].copy_from_slice(&b[..n]);
        self.solve(out);
    }

    #[must_use]
    pub fn quad_inverse(&self, a: &[Complex<f32>], scratch: &mut [Complex<f32>]) -> f32 {
        let n = self.order;
        let y = &mut scratch[..n];
        y.copy_from_slice(&a[..n]);
        self.forward(y);
        y.iter().map(|value| value.norm_sqr()).sum()
    }

    pub fn inverse_into(
        &self,
        out: &mut CMat,
        scratch: &mut [Complex<f32>],
    ) -> Result<(), LinalgError> {
        let n = self.order;
        if n > MAX_ORDER {
            return Err(LinalgError::Order(n));
        }
        out.resize(n)?;
        let column = &mut scratch[..n];
        for col in 0..n {
            column.fill(Complex::new(0.0, 0.0));
            column[col] = Complex::new(1.0, 0.0);
            self.solve(column);
            for (row, &value) in column.iter().enumerate() {
                out.set(row, col, value);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn log_det(&self) -> f32 {
        2.0 * self.pivots().map(f32::ln).sum::<f32>()
    }

    #[must_use]
    pub fn pivot_ratio(&self) -> f32 {
        let (low, high) = self
            .pivots()
            .fold((f32::INFINITY, 0.0f32), |(low, high), pivot| {
                (low.min(pivot), high.max(pivot))
            });
        if high > 0.0 { low / high } else { 0.0 }
    }

    fn pivots(&self) -> impl Iterator<Item = f32> + '_ {
        let n = self.order;
        (0..n).map(move |i| self.l[i * n + i].re)
    }

    fn forward(&self, y: &mut [Complex<f32>]) {
        let n = self.order;
        for i in 0..n {
            let row = &self.l[i * n..i * n + i];
            let mut sum = y[i];
            for (l, solved) in row.iter().zip(y[..i].iter()) {
                sum -= l * solved;
            }
            y[i] = sum / self.l[i * n + i].re;
        }
    }

    fn backward(&self, x: &mut [Complex<f32>]) {
        let n = self.order;
        for i in (0..n).rev() {
            let mut sum = x[i];
            for (k, solved) in x[i + 1..].iter().enumerate() {
                sum -= self.l[(i + 1 + k) * n + i].conj() * solved;
            }
            x[i] = sum / self.l[i * n + i].re;
        }
    }

    fn decompose(
        &mut self,
        n: usize,
        entry: impl Fn(usize, usize) -> Complex<f32>,
        loading: f32,
    ) -> Result<(), LinalgError> {
        if n == 0 || n > self.capacity {
            return Err(LinalgError::Order(n));
        }
        self.order = n;
        let l = &mut self.l[..n * n];
        l.fill(Complex::new(0.0, 0.0));
        for i in 0..n {
            for j in 0..=i {
                let value = entry(i, j);
                if !value.is_finite() {
                    return Err(LinalgError::NonFinite);
                }
                let mut sum = value;
                for k in 0..j {
                    sum -= l[i * n + k] * l[j * n + k].conj();
                }
                if i == j {
                    let diagonal = value.re + loading;
                    let pivot = sum.re + loading;
                    if !pivot.is_finite() {
                        return Err(LinalgError::NonFinite);
                    }
                    if pivot <= PIVOT_TOL * diagonal {
                        return Err(LinalgError::NotPositiveDefinite(i));
                    }
                    l[i * n + i] = Complex::new(pivot.sqrt(), 0.0);
                } else {
                    l[i * n + j] = sum / l[j * n + j].re;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::linalg::testing::{hermitian, mat_vec, outer_sum, random_hpd, random_vector};
    use crate::testutil::XorShift32;

    fn factored(matrix: &CMat) -> Cholesky {
        let mut chol = Cholesky::new(matrix.order()).unwrap();
        chol.factor_cmat(matrix).unwrap();
        chol
    }

    #[test]
    fn cholesky_solves_a_system_it_factored() {
        let matrix = hermitian(
            3,
            &[
                (0, 0, 4.0, 0.0),
                (0, 1, 1.0, 1.0),
                (0, 2, 0.5, -0.2),
                (1, 1, 3.0, 0.0),
                (1, 2, 0.3, 0.4),
                (2, 2, 2.5, 0.0),
            ],
        );
        let want = [
            Complex::new(1.0f32, -0.5),
            Complex::new(-2.0, 0.25),
            Complex::new(0.75, 1.5),
        ];
        let mut b = mat_vec(&matrix, &want);
        let mut chol = Cholesky::new(3).unwrap();
        chol.factor(matrix.as_slice()).unwrap();
        chol.solve(&mut b);
        for (index, (got, expected)) in b.iter().zip(&want).enumerate() {
            assert!(
                (got - expected).norm() < 1e-4,
                "x[{index}]: {got} vs {expected}"
            );
        }
        let mut out = [Complex::new(0.0f32, 0.0); 3];
        chol.solve_into(&mat_vec(&matrix, &want), &mut out);
        assert!(
            out.iter()
                .zip(&want)
                .all(|(got, expected)| (got - expected).norm() < 1e-4)
        );
    }

    #[test]
    fn a_matrix_that_is_not_positive_definite_is_refused() {
        let matrix = hermitian(2, &[(0, 0, 1.0, 0.0), (0, 1, 4.0, 0.0), (1, 1, 1.0, 0.0)]);
        let mut chol = Cholesky::new(2).unwrap();
        assert!(matches!(
            chol.factor_cmat(&matrix),
            Err(LinalgError::NotPositiveDefinite(1))
        ));
    }

    #[test]
    fn an_order_outside_the_supported_range_is_refused() {
        assert!(matches!(Cholesky::new(0), Err(LinalgError::Order(0))));
        assert!(matches!(
            Cholesky::new(MAX_SOLVE_ORDER + 1),
            Err(LinalgError::Order(_))
        ));
        let mut chol = Cholesky::new(2).unwrap();
        assert_eq!(
            chol.factor_cmat(&CMat::identity(3).unwrap()),
            Err(LinalgError::Order(3))
        );
        assert_eq!(
            chol.factor(&[Complex::new(1.0, 0.0); 3]),
            Err(LinalgError::Order(3))
        );
    }

    #[test]
    fn a_smaller_matrix_reuses_the_storage() {
        let mut chol = Cholesky::new(6).unwrap();
        let matrix = random_hpd(4, 5);
        chol.factor_cmat(&matrix).unwrap();
        assert_eq!(chol.order(), 4);
        let want = [Complex::new(0.5f32, -1.0); 4];
        let mut b = mat_vec(&matrix, &want);
        chol.solve(&mut b);
        assert!(b.iter().all(|got| (got - want[0]).norm() < 1e-4));
    }

    #[test]
    fn a_non_finite_entry_is_refused() {
        let mut matrix = CMat::identity(3).unwrap();
        matrix.set(2, 1, Complex::new(f32::INFINITY, 0.0));
        let mut chol = Cholesky::new(3).unwrap();
        assert_eq!(chol.factor_cmat(&matrix), Err(LinalgError::NonFinite));
        assert_eq!(
            chol.factor_loaded(&matrix, 0.1),
            Err(LinalgError::NonFinite)
        );
    }

    #[test]
    fn cholesky_inverse_times_matrix_is_identity() {
        let matrix = random_hpd(5, 11);
        let chol = factored(&matrix);
        let mut inverse = CMat::zeros(1).unwrap();
        let mut scratch = [Complex::new(0.0f32, 0.0); 5];
        chol.inverse_into(&mut inverse, &mut scratch).unwrap();
        for row in 0..5 {
            for col in 0..5 {
                let product: Complex<f32> = (0..5)
                    .map(|k| matrix.get(row, k) * inverse.get(k, col))
                    .sum();
                let want = if row == col { 1.0 } else { 0.0 };
                assert!(
                    (product - Complex::new(want, 0.0)).norm() < 1e-4,
                    "{row},{col}"
                );
            }
        }
    }

    #[test]
    fn cholesky_loaded_factor_rescues_a_rank_one_matrix() {
        let mut rng = XorShift32(3);
        let matrix = outer_sum(5, &[random_vector(&mut rng, 5)]);
        let mut chol = Cholesky::new(5).unwrap();
        assert!(matches!(
            chol.factor_cmat(&matrix),
            Err(LinalgError::NotPositiveDefinite(_))
        ));
        let loading = chol.factor_loaded(&matrix, 0.0).unwrap();
        assert!(loading > 0.0);
        assert!(loading <= 1e-3 * matrix.trace_re());
        assert!(chol.pivot_ratio() > 0.0);
    }

    #[test]
    fn loading_that_works_first_time_is_reported_as_is() {
        let matrix = random_hpd(4, 13);
        let mut chol = Cholesky::new(4).unwrap();
        assert_eq!(chol.factor_loaded(&matrix, 0.0), Ok(0.0));
        let base = chol.factor_loaded(&matrix, 0.01).unwrap();
        assert!((base - 0.01 * matrix.trace_re() / 4.0).abs() < 1e-6 * base);
    }

    #[test]
    fn a_zero_matrix_cannot_be_loaded() {
        let mut chol = Cholesky::new(3).unwrap();
        assert!(matches!(
            chol.factor_loaded(&CMat::zeros(3).unwrap(), 0.1),
            Err(LinalgError::NotPositiveDefinite(0))
        ));
    }

    #[test]
    fn quad_inverse_matches_explicit_inverse() {
        let matrix = random_hpd(5, 17);
        let chol = factored(&matrix);
        let mut rng = XorShift32(23);
        let a = random_vector(&mut rng, 5);
        let mut scratch = [Complex::new(0.0f32, 0.0); 5];
        let fast = chol.quad_inverse(&a, &mut scratch);
        let mut inverse = CMat::zeros(1).unwrap();
        chol.inverse_into(&mut inverse, &mut scratch).unwrap();
        let slow = inverse.quad(&a);
        assert!((fast - slow).abs() < 1e-4 * slow.abs(), "{fast} vs {slow}");
    }

    #[test]
    fn log_det_and_pivot_ratio_follow_the_diagonal() {
        let matrix = hermitian(3, &[(0, 0, 4.0, 0.0), (1, 1, 1.0, 0.0), (2, 2, 9.0, 0.0)]);
        let chol = factored(&matrix);
        assert!((chol.log_det() - 36f32.ln()).abs() < 1e-5);
        assert!((chol.pivot_ratio() - 1.0 / 3.0).abs() < 1e-6);
    }
}
