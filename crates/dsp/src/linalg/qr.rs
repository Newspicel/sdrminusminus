use num_complex::Complex;

use super::{CMat, LinalgError, MAX_ORDER};

const RANK_TOL: f32 = 1e-6;

pub struct Qr {
    rows: usize,
    cols: usize,
    q: CMat,
    r: CMat,
}

impl Qr {
    pub fn new(rows: usize, cols: usize) -> Result<Self, LinalgError> {
        if rows > MAX_ORDER {
            return Err(LinalgError::Order(rows));
        }
        if cols == 0 || cols > rows {
            return Err(LinalgError::Order(cols));
        }
        Ok(Self {
            rows,
            cols,
            q: CMat::identity(rows)?,
            r: CMat::zeros(rows)?,
        })
    }

    pub fn factor(&mut self, a: &[Complex<f32>]) -> Result<(), LinalgError> {
        let (rows, cols) = (self.rows, self.cols);
        if a.len() != rows * cols {
            return Err(LinalgError::Order(a.len()));
        }
        if !a.iter().all(|value| value.is_finite()) {
            return Err(LinalgError::NonFinite);
        }
        self.q.resize(rows)?;
        self.r.resize(rows)?;
        for row in 0..rows {
            self.q.set(row, row, Complex::new(1.0, 0.0));
            for col in 0..cols {
                self.r.set(row, col, a[row * cols + col]);
            }
        }
        for column in 0..cols {
            self.reflect(column);
        }
        self.check_rank()
    }

    #[must_use]
    pub const fn q(&self) -> &CMat {
        &self.q
    }

    #[must_use]
    pub const fn r(&self) -> &CMat {
        &self.r
    }

    pub fn null_basis(&self, out: &mut [Complex<f32>]) -> Result<usize, LinalgError> {
        let width = self.rows - self.cols;
        if out.len() < self.rows * width {
            return Err(LinalgError::Order(out.len()));
        }
        for row in 0..self.rows {
            for col in 0..width {
                out[row * width + col] = self.q.get(row, self.cols + col);
            }
        }
        Ok(width)
    }

    pub fn solve_upper(&self, rhs: &mut [Complex<f32>]) -> Result<(), LinalgError> {
        let n = self.cols;
        if rhs.len() < n {
            return Err(LinalgError::Order(rhs.len()));
        }
        self.check_rank()?;
        for i in (0..n).rev() {
            let solved: Complex<f32> = rhs[i + 1..n]
                .iter()
                .enumerate()
                .map(|(offset, &x)| self.r.get(i, i + 1 + offset) * x)
                .sum();
            rhs[i] = (rhs[i] - solved) / self.r.get(i, i);
        }
        Ok(())
    }

    fn reflect(&mut self, column: usize) {
        let rows = self.rows;
        let norm = (column..rows)
            .map(|row| self.r.get(row, column).norm_sqr())
            .sum::<f32>()
            .sqrt();
        if norm == 0.0 {
            return;
        }
        let head = self.r.get(column, column);
        let phase = if head.norm() > 0.0 {
            head / head.norm()
        } else {
            Complex::new(1.0, 0.0)
        };
        let alpha = -phase * norm;
        let mut v = [Complex::new(0.0f32, 0.0); MAX_ORDER];
        v[column] = head - alpha;
        for (row, slot) in v.iter_mut().enumerate().take(rows).skip(column + 1) {
            *slot = self.r.get(row, column);
        }
        let length = v[column..rows]
            .iter()
            .map(|value| value.norm_sqr())
            .sum::<f32>()
            .sqrt();
        for value in &mut v[column..rows] {
            *value /= length;
        }
        for col in (column + 1)..self.cols {
            let dot: Complex<f32> = (column..rows)
                .map(|row| v[row].conj() * self.r.get(row, col))
                .sum();
            for (row, &vr) in v.iter().enumerate().take(rows).skip(column) {
                self.r.add(row, col, -2.0 * vr * dot);
            }
        }
        self.r.set(column, column, alpha);
        for row in (column + 1)..rows {
            self.r.set(row, column, Complex::new(0.0, 0.0));
        }
        for row in 0..rows {
            let dot: Complex<f32> = (column..rows).map(|k| self.q.get(row, k) * v[k]).sum();
            for (k, &vk) in v.iter().enumerate().take(rows).skip(column) {
                self.q.add(row, k, -2.0 * dot * vk.conj());
            }
        }
    }

    fn check_rank(&self) -> Result<(), LinalgError> {
        let largest = (0..self.cols)
            .map(|i| self.r.get(i, i).norm())
            .fold(0.0f32, f32::max);
        match (0..self.cols).find(|&i| self.r.get(i, i).norm() <= RANK_TOL * largest) {
            Some(column) => Err(LinalgError::RankDeficient(column)),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::linalg::testing::random_vector;
    use crate::testutil::XorShift32;

    fn product(
        left: &[Complex<f32>],
        right: &[Complex<f32>],
        rows: usize,
        inner: usize,
        cols: usize,
        conj_left: bool,
    ) -> Vec<Complex<f32>> {
        let mut out = vec![Complex::new(0.0f32, 0.0); rows * cols];
        for row in 0..rows {
            for col in 0..cols {
                out[row * cols + col] = (0..inner)
                    .map(|k| {
                        let l = if conj_left {
                            left[k * rows + row].conj()
                        } else {
                            left[row * inner + k]
                        };
                        l * right[k * cols + col]
                    })
                    .sum();
            }
        }
        out
    }

    fn max_distance(a: &[Complex<f32>], b: &[Complex<f32>]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).norm())
            .fold(0.0, f32::max)
    }

    fn identity(n: usize) -> Vec<Complex<f32>> {
        let mut out = vec![Complex::new(0.0f32, 0.0); n * n];
        for i in 0..n {
            out[i * n + i] = Complex::new(1.0, 0.0);
        }
        out
    }

    fn block(matrix: &CMat, rows: usize, cols: usize) -> Vec<Complex<f32>> {
        (0..rows)
            .flat_map(|row| (0..cols).map(move |col| matrix.get(row, col)))
            .collect()
    }

    #[test]
    fn qr_reconstructs_and_q_is_unitary() {
        let mut rng = XorShift32(41);
        let a = random_vector(&mut rng, 10);
        let mut qr = Qr::new(5, 2).unwrap();
        qr.factor(&a).unwrap();
        let q = qr.q().as_slice();
        let r = block(qr.r(), 5, 2);
        assert!(max_distance(&product(q, &r, 5, 5, 2, false), &a) < 1e-5);
        assert!(max_distance(&product(q, q, 5, 5, 5, true), &identity(5)) < 1e-5);
        assert!(r[2].norm() == 0.0 && r[4].norm() == 0.0);
    }

    #[test]
    fn qr_null_basis_is_orthogonal_to_the_constraints() {
        let mut rng = XorShift32(43);
        let c = random_vector(&mut rng, 10);
        let mut qr = Qr::new(5, 2).unwrap();
        qr.factor(&c).unwrap();
        let mut basis = [Complex::new(0.0f32, 0.0); 15];
        assert_eq!(qr.null_basis(&mut basis), Ok(3));
        let zeros = vec![Complex::new(0.0f32, 0.0); 6];
        assert!(max_distance(&product(&c, &basis, 2, 5, 3, true), &zeros) < 1e-5);
        assert!(max_distance(&product(&basis, &basis, 3, 5, 3, true), &identity(3)) < 1e-5);
        assert_eq!(
            qr.null_basis(&mut [Complex::new(0.0, 0.0); 14]),
            Err(LinalgError::Order(14))
        );
    }

    #[test]
    fn solve_upper_inverts_a_square_system() {
        let mut rng = XorShift32(47);
        let a = random_vector(&mut rng, 9);
        let want = random_vector(&mut rng, 3);
        let b = product(&a, &want, 3, 3, 1, false);
        let mut qr = Qr::new(3, 3).unwrap();
        qr.factor(&a).unwrap();
        let mut x = product(qr.q().as_slice(), &b, 3, 3, 1, true);
        qr.solve_upper(&mut x).unwrap();
        assert!(max_distance(&x, &want) < 1e-4);
    }

    #[test]
    fn parallel_columns_are_rank_deficient() {
        let a = [
            Complex::new(1.0f32, 0.0),
            Complex::new(2.0, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(0.0, 2.0),
            Complex::new(3.0, -1.0),
            Complex::new(6.0, -2.0),
        ];
        let mut qr = Qr::new(3, 2).unwrap();
        assert_eq!(qr.factor(&a), Err(LinalgError::RankDeficient(1)));
        assert_eq!(
            qr.solve_upper(&mut [Complex::new(1.0, 0.0); 2]),
            Err(LinalgError::RankDeficient(1))
        );
    }

    #[test]
    fn shapes_outside_the_solver_are_refused() {
        assert_eq!(Qr::new(3, 4).err(), Some(LinalgError::Order(4)));
        assert_eq!(Qr::new(17, 2).err(), Some(LinalgError::Order(17)));
        assert_eq!(Qr::new(3, 0).err(), Some(LinalgError::Order(0)));
        let mut qr = Qr::new(3, 2).unwrap();
        assert_eq!(
            qr.factor(&[Complex::new(1.0, 0.0); 5]),
            Err(LinalgError::Order(5))
        );
        let mut bad = [Complex::new(1.0f32, 0.0); 6];
        bad[3] = Complex::new(f32::NAN, 0.0);
        assert_eq!(qr.factor(&bad), Err(LinalgError::NonFinite));
    }
}
