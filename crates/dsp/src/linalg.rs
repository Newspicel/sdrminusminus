use num_complex::Complex;

/// The largest array this solver is built for. Direction finding and clutter cancellation work on
/// a handful of lanes or delay taps, and a fixed ceiling keeps the whole thing on one cache line's
/// worth of thinking rather than pulling in a general linear-algebra dependency.
pub const MAX_ORDER: usize = 16;

/// Cholesky is asked for much larger systems than the eigensolver: a clutter canceller solves for
/// one weight per delay tap and Doppler hypothesis, and a hundred of those is an ordinary setting.
pub const MAX_SOLVE_ORDER: usize = 512;

const SWEEPS: usize = 30;

#[derive(Debug, thiserror::Error)]
pub enum LinalgError {
    #[error("matrix order {0} is outside what this solver handles")]
    Order(usize),
    #[error("matrix is not positive definite at row {0}")]
    NotPositiveDefinite(usize),
}

/// A Hermitian matrix's eigenvalues in ascending order and the matching eigenvectors.
///
/// Ascending is deliberate: the noise subspace MUSIC needs is the front of the list, and a
/// signal count is a cut from the back.
#[derive(Clone, Debug, Default)]
pub struct Eigen {
    pub order: usize,
    pub values: Vec<f32>,
    /// Column `k` holds eigenvector `k`, so element `i` is `vectors[k * order + i]`.
    pub vectors: Vec<Complex<f32>>,
}

impl Eigen {
    #[must_use]
    pub fn vector(&self, index: usize) -> &[Complex<f32>] {
        let start = index * self.order;
        &self.vectors[start..start + self.order]
    }
}

/// Cyclic Jacobi for complex Hermitian matrices.
///
/// Each sweep rotates away the largest remaining off-diagonal element; the rotation that does it
/// is unitary, so the eigenvectors come out of the same accumulation for free.
pub struct HermitianEigen {
    order: usize,
    a: Vec<Complex<f32>>,
    v: Vec<Complex<f32>>,
}

impl HermitianEigen {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        if order == 0 || order > MAX_ORDER {
            return Err(LinalgError::Order(order));
        }
        Ok(Self {
            order,
            a: vec![Complex::default(); order * order],
            v: vec![Complex::default(); order * order],
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    /// Decomposes `matrix`, given row-major with element `(row, col)` at `row * order + col`.
    pub fn solve(&mut self, matrix: &[Complex<f32>], out: &mut Eigen) {
        let n = self.order;
        self.a.copy_from_slice(&matrix[..n * n]);
        self.v.fill(Complex::default());
        for i in 0..n {
            self.v[i * n + i] = Complex::new(1.0, 0.0);
        }
        for _ in 0..SWEEPS {
            let mut off = 0.0f32;
            for p in 0..n {
                for q in (p + 1)..n {
                    off += self.a[p * n + q].norm_sqr();
                }
            }
            if off <= f32::EPSILON * f32::EPSILON {
                break;
            }
            for p in 0..n {
                for q in (p + 1)..n {
                    self.rotate(p, q);
                }
            }
        }
        out.order = n;
        out.values.clear();
        out.values.extend((0..n).map(|i| self.a[i * n + i].re));
        out.vectors.clear();
        out.vectors.resize(n * n, Complex::default());
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|&x, &y| out.values[x].total_cmp(&out.values[y]));
        let unsorted = out.values.clone();
        for (slot, &source) in order.iter().enumerate() {
            out.values[slot] = unsorted[source];
            for row in 0..n {
                out.vectors[slot * n + row] = self.v[row * n + source];
            }
        }
    }

    fn rotate(&mut self, p: usize, q: usize) {
        let n = self.order;
        let apq = self.a[p * n + q];
        let magnitude = apq.norm();
        if magnitude <= f32::MIN_POSITIVE {
            return;
        }
        let app = self.a[p * n + p].re;
        let aqq = self.a[q * n + q].re;
        let theta = 0.5 * (2.0 * magnitude).atan2(app - aqq);
        let (s, c) = theta.sin_cos();
        let phase = apq / magnitude;
        for row in 0..n {
            let ap = self.a[row * n + p];
            let aq = self.a[row * n + q];
            self.a[row * n + p] = ap * c + aq * phase.conj() * s;
            self.a[row * n + q] = aq * c - ap * phase * s;
        }
        for col in 0..n {
            let ap = self.a[p * n + col];
            let aq = self.a[q * n + col];
            self.a[p * n + col] = ap * c + aq * phase * s;
            self.a[q * n + col] = aq * c - ap * phase.conj() * s;
        }
        for row in 0..n {
            let vp = self.v[row * n + p];
            let vq = self.v[row * n + q];
            self.v[row * n + p] = vp * c + vq * phase.conj() * s;
            self.v[row * n + q] = vq * c - vp * phase * s;
        }
        self.a[p * n + q] = Complex::default();
        self.a[q * n + p] = Complex::default();
    }
}

/// In-place Cholesky factorisation of a Hermitian positive-definite matrix, kept alongside the
/// solve that uses it because nothing here ever wants one without the other.
pub struct Cholesky {
    order: usize,
    l: Vec<Complex<f32>>,
}

impl Cholesky {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        if order == 0 || order > MAX_SOLVE_ORDER {
            return Err(LinalgError::Order(order));
        }
        Ok(Self {
            order,
            l: vec![Complex::default(); order * order],
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    pub fn factor(&mut self, matrix: &[Complex<f32>]) -> Result<(), LinalgError> {
        let n = self.order;
        self.l.fill(Complex::default());
        for i in 0..n {
            for j in 0..=i {
                let mut sum = matrix[i * n + j];
                for k in 0..j {
                    sum -= self.l[i * n + k] * self.l[j * n + k].conj();
                }
                if i == j {
                    if sum.re <= 0.0 {
                        return Err(LinalgError::NotPositiveDefinite(i));
                    }
                    self.l[i * n + j] = Complex::new(sum.re.sqrt(), 0.0);
                } else {
                    self.l[i * n + j] = sum / self.l[j * n + j];
                }
            }
        }
        Ok(())
    }

    /// Solves `A x = b` for the matrix last handed to `factor`, in place.
    pub fn solve(&self, b: &mut [Complex<f32>]) {
        let n = self.order;
        for i in 0..n {
            let mut sum = b[i];
            for (k, solved) in b[..i].iter().enumerate() {
                sum -= self.l[i * n + k] * solved;
            }
            b[i] = sum / self.l[i * n + i];
        }
        for i in (0..n).rev() {
            let mut sum = b[i];
            for (k, solved) in b[i + 1..].iter().enumerate() {
                sum -= self.l[(i + 1 + k) * n + i].conj() * solved;
            }
            b[i] = sum / self.l[i * n + i];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hermitian(order: usize, entries: &[(usize, usize, f32, f32)]) -> Vec<Complex<f32>> {
        let mut matrix = vec![Complex::default(); order * order];
        for &(row, col, re, im) in entries {
            matrix[row * order + col] = Complex::new(re, im);
            matrix[col * order + row] = Complex::new(re, -im);
        }
        matrix
    }

    fn residual(matrix: &[Complex<f32>], eigen: &Eigen, index: usize) -> f32 {
        let n = eigen.order;
        let vector = eigen.vector(index);
        let value = eigen.values[index];
        (0..n)
            .map(|row| {
                let mut sum = Complex::<f32>::default();
                for col in 0..n {
                    sum += matrix[row * n + col] * vector[col];
                }
                (sum - vector[row] * value).norm()
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_diagonal_matrix_comes_back_sorted() {
        let matrix = hermitian(3, &[(0, 0, 5.0, 0.0), (1, 1, -2.0, 0.0), (2, 2, 1.0, 0.0)]);
        let mut solver = HermitianEigen::new(3).expect("order");
        let mut eigen = Eigen::default();
        solver.solve(&matrix, &mut eigen);
        assert_eq!(eigen.values.len(), 3);
        assert!((eigen.values[0] + 2.0).abs() < 1e-4);
        assert!((eigen.values[1] - 1.0).abs() < 1e-4);
        assert!((eigen.values[2] - 5.0).abs() < 1e-4);
    }

    #[test]
    fn a_known_hermitian_matrix_matches_its_eigenvalues() {
        let matrix = hermitian(2, &[(0, 0, 2.0, 0.0), (0, 1, 0.0, -1.0), (1, 1, 2.0, 0.0)]);
        let mut solver = HermitianEigen::new(2).expect("order");
        let mut eigen = Eigen::default();
        solver.solve(&matrix, &mut eigen);
        assert!((eigen.values[0] - 1.0).abs() < 1e-4, "{:?}", eigen.values);
        assert!((eigen.values[1] - 3.0).abs() < 1e-4, "{:?}", eigen.values);
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
        let mut solver = HermitianEigen::new(4).expect("order");
        let mut eigen = Eigen::default();
        solver.solve(&matrix, &mut eigen);
        for index in 0..4 {
            assert!(
                residual(&matrix, &eigen, index) < 1e-3,
                "eigenpair {index} does not satisfy A v = λ v"
            );
            let norm: f32 = eigen.vector(index).iter().map(Complex::norm_sqr).sum();
            assert!((norm - 1.0).abs() < 1e-3, "eigenvector {index} is not unit");
        }
        assert!(eigen.values.windows(2).all(|pair| pair[0] <= pair[1]));
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
        let mut b = [Complex::<f32>::default(); 3];
        for row in 0..3 {
            for col in 0..3 {
                b[row] += matrix[row * 3 + col] * want[col];
            }
        }
        let mut chol = Cholesky::new(3).expect("order");
        chol.factor(&matrix).expect("positive definite");
        chol.solve(&mut b);
        for (index, (got, expected)) in b.iter().zip(&want).enumerate() {
            assert!(
                (got - expected).norm() < 1e-4,
                "x[{index}]: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn a_matrix_that_is_not_positive_definite_is_refused() {
        let matrix = hermitian(2, &[(0, 0, 1.0, 0.0), (0, 1, 4.0, 0.0), (1, 1, 1.0, 0.0)]);
        let mut chol = Cholesky::new(2).expect("order");
        assert!(matches!(
            chol.factor(&matrix),
            Err(LinalgError::NotPositiveDefinite(_))
        ));
    }

    #[test]
    fn an_order_outside_the_supported_range_is_refused() {
        assert!(matches!(HermitianEigen::new(0), Err(LinalgError::Order(0))));
        assert!(matches!(
            HermitianEigen::new(MAX_ORDER + 1),
            Err(LinalgError::Order(_))
        ));
        assert!(matches!(Cholesky::new(0), Err(LinalgError::Order(0))));
    }
}
