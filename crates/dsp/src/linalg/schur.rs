use num_complex::Complex;

use super::{LinalgError, MAX_SCHUR_ORDER};

const EXCEPTIONAL_EVERY: u32 = 10;
const BUDGET_PER_ORDER: u32 = 30;

pub struct GeneralEigen {
    n: usize,
    h: Vec<Complex<f64>>,
    work: Vec<Complex<f64>>,
}

impl GeneralEigen {
    pub fn new(n: usize) -> Result<Self, LinalgError> {
        if n == 0 || n > MAX_SCHUR_ORDER {
            return Err(LinalgError::Order(n));
        }
        Ok(Self {
            n,
            h: vec![Complex::new(0.0, 0.0); n * n],
            work: vec![Complex::new(0.0, 0.0); 2 * n],
        })
    }

    pub fn eigenvalues(
        &mut self,
        matrix: &[Complex<f64>],
        out: &mut [Complex<f64>],
    ) -> Result<(), LinalgError> {
        let m = matrix.len().isqrt();
        if m == 0 || m * m != matrix.len() || m > self.n {
            return Err(LinalgError::Order(m));
        }
        if out.len() < m {
            return Err(LinalgError::Order(out.len()));
        }
        if !matrix.iter().all(|value| value.is_finite()) {
            return Err(LinalgError::NonFinite);
        }
        let mut schur = Schur {
            m,
            h: &mut self.h[..m * m],
            work: &mut self.work[..2 * m],
        };
        schur.h.copy_from_slice(matrix);
        schur.hessenberg();
        schur.iterate(&mut out[..m])
    }
}

struct Schur<'a> {
    m: usize,
    h: &'a mut [Complex<f64>],
    work: &'a mut [Complex<f64>],
}

impl Schur<'_> {
    fn at(&self, row: usize, col: usize) -> Complex<f64> {
        self.h[row * self.m + col]
    }

    fn put(&mut self, row: usize, col: usize, value: Complex<f64>) {
        self.h[row * self.m + col] = value;
    }

    fn hessenberg(&mut self) {
        let m = self.m;
        for k in 0..m.saturating_sub(2) {
            let norm = ((k + 1)..m)
                .map(|row| self.at(row, k).norm_sqr())
                .sum::<f64>()
                .sqrt();
            if norm == 0.0 {
                continue;
            }
            let head = self.at(k + 1, k);
            let phase = if head.norm() > 0.0 {
                head / head.norm()
            } else {
                Complex::new(1.0, 0.0)
            };
            let alpha = -phase * norm;
            let v = &mut self.work[..m];
            v.fill(Complex::new(0.0, 0.0));
            v[k + 1] = head - alpha;
            for (row, slot) in v.iter_mut().enumerate().skip(k + 2) {
                *slot = self.h[row * m + k];
            }
            let length = v.iter().map(|value| value.norm_sqr()).sum::<f64>().sqrt();
            for value in v.iter_mut() {
                *value /= length;
            }
            self.reflect_left(k);
            self.reflect_right();
            self.put(k + 1, k, alpha);
            for row in (k + 2)..m {
                self.put(row, k, Complex::new(0.0, 0.0));
            }
        }
    }

    fn reflect_left(&mut self, first_col: usize) {
        let m = self.m;
        for col in first_col..m {
            let dot: Complex<f64> = (0..m)
                .map(|row| self.work[row].conj() * self.h[row * m + col])
                .sum();
            for row in 0..m {
                let delta = 2.0 * self.work[row] * dot;
                self.h[row * m + col] -= delta;
            }
        }
    }

    fn reflect_right(&mut self) {
        let m = self.m;
        for row in 0..m {
            let dot: Complex<f64> = (0..m)
                .map(|col| self.h[row * m + col] * self.work[col])
                .sum();
            for col in 0..m {
                let delta = 2.0 * dot * self.work[col].conj();
                self.h[row * m + col] -= delta;
            }
        }
    }

    fn iterate(&mut self, out: &mut [Complex<f64>]) -> Result<(), LinalgError> {
        let budget = BUDGET_PER_ORDER * self.m as u32;
        let scale = self.h.iter().map(|value| value.norm()).fold(0.0, f64::max);
        let mut total = 0u32;
        let mut since_deflation = 0u32;
        let mut hi = self.m - 1;
        loop {
            if hi == 0 {
                out[0] = self.at(0, 0);
                return Ok(());
            }
            let lo = self.active_start(hi, scale);
            if lo == hi {
                out[hi] = self.at(hi, hi);
                hi -= 1;
                since_deflation = 0;
                continue;
            }
            if total >= budget {
                return Err(LinalgError::NotConverged(budget));
            }
            total += 1;
            since_deflation += 1;
            let shift = if since_deflation.is_multiple_of(EXCEPTIONAL_EVERY) {
                self.exceptional_shift(lo, hi)
            } else {
                self.wilkinson_shift(hi)
            };
            self.qr_step(lo, hi, shift);
        }
    }

    fn active_start(&mut self, hi: usize, scale: f64) -> usize {
        let mut l = hi;
        while l > 0 {
            let sub = self.at(l, l - 1).norm();
            let mut neighbours = self.at(l, l).norm() + self.at(l - 1, l - 1).norm();
            if neighbours == 0.0 {
                neighbours = scale;
            }
            if sub <= f64::EPSILON * neighbours {
                self.put(l, l - 1, Complex::new(0.0, 0.0));
                return l;
            }
            l -= 1;
        }
        0
    }

    fn wilkinson_shift(&self, hi: usize) -> Complex<f64> {
        let a = self.at(hi - 1, hi - 1);
        let b = self.at(hi - 1, hi);
        let c = self.at(hi, hi - 1);
        let d = self.at(hi, hi);
        let half = 0.5 * (a - d);
        let root = (half * half + b * c).sqrt();
        let near = d - b * c / (half + root);
        let far = d - b * c / (half - root);
        let chosen = if (half + root).norm() >= (half - root).norm() {
            near
        } else {
            far
        };
        if chosen.is_finite() { chosen } else { d }
    }

    fn exceptional_shift(&self, lo: usize, hi: usize) -> Complex<f64> {
        let mut spread = self.at(hi, hi - 1).norm();
        if hi >= lo + 2 {
            spread += self.at(hi - 1, hi - 2).norm();
        }
        self.at(hi, hi) + spread
    }

    fn qr_step(&mut self, lo: usize, hi: usize, shift: Complex<f64>) {
        let m = self.m;
        for k in lo..=hi {
            self.h[k * m + k] -= shift;
        }
        for k in lo..hi {
            let (c, s) = givens(self.at(k, k), self.at(k + 1, k));
            self.work[2 * k] = Complex::new(c, 0.0);
            self.work[2 * k + 1] = s;
            for col in k..=hi {
                let top = self.at(k, col);
                let bottom = self.at(k + 1, col);
                self.put(k, col, top * c + s * bottom);
                self.put(k + 1, col, bottom * c - s.conj() * top);
            }
        }
        for k in lo..hi {
            let c = self.work[2 * k].re;
            let s = self.work[2 * k + 1];
            for row in lo..=(k + 1).min(hi) {
                let left = self.at(row, k);
                let right = self.at(row, k + 1);
                self.put(row, k, left * c + right * s.conj());
                self.put(row, k + 1, right * c - left * s);
            }
        }
        for k in lo..=hi {
            self.h[k * m + k] += shift;
        }
    }
}

fn givens(x: Complex<f64>, y: Complex<f64>) -> (f64, Complex<f64>) {
    let ax = x.norm();
    let r = ax.hypot(y.norm());
    if r == 0.0 {
        return (1.0, Complex::new(0.0, 0.0));
    }
    if ax == 0.0 {
        return (0.0, y.conj() / y.norm());
    }
    (ax / r, (x / ax) * y.conj() / r)
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;

    fn c(re: f64, im: f64) -> Complex<f64> {
        Complex::new(re, im)
    }

    fn real<const N: usize>(rows: &[[f64; N]; N]) -> Vec<Complex<f64>> {
        rows.iter().flatten().map(|&value| c(value, 0.0)).collect()
    }

    fn matmul(a: &[Complex<f64>], b: &[Complex<f64>], n: usize) -> Vec<Complex<f64>> {
        let mut out = vec![c(0.0, 0.0); n * n];
        for row in 0..n {
            for col in 0..n {
                out[row * n + col] = (0..n).map(|k| a[row * n + k] * b[k * n + col]).sum();
            }
        }
        out
    }

    fn assert_matches(found: &[Complex<f64>], want: &[Complex<f64>], tolerance: f64) {
        let mut used = vec![false; want.len()];
        for value in found {
            let (index, distance) = want
                .iter()
                .enumerate()
                .filter(|(index, _)| !used[*index])
                .map(|(index, target)| (index, (value - target).norm()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            assert!(
                distance < tolerance,
                "{value} is {distance} from {:?}",
                want[index]
            );
            used[index] = true;
        }
    }

    #[test]
    fn schur_finds_eigenvalues_of_a_rotation_plus_shift() {
        let block = real(&[
            [3.0, -1.0, 1.0, 2.0],
            [1.0, 3.0, 0.0, 1.0],
            [0.0, 0.0, -1.0, -2.0],
            [0.0, 0.0, 2.0, -1.0],
        ]);
        let mix = real(&[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0, 1.0],
        ]);
        let unmix = real(&[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [-1.0, 0.0, 1.0, 0.0],
            [0.0, 1.0, 0.0, 1.0],
        ]);
        let matrix = matmul(&matmul(&mix, &block, 4), &unmix, 4);
        let mut solver = GeneralEigen::new(4).unwrap();
        let mut out = [c(0.0, 0.0); 4];
        solver.eigenvalues(&matrix, &mut out).unwrap();
        assert_matches(
            &out,
            &[c(3.0, 1.0), c(3.0, -1.0), c(-1.0, 2.0), c(-1.0, -2.0)],
            1e-10,
        );
    }

    #[test]
    fn complex_triangular_similarity_keeps_its_diagonal() {
        let diagonal = [
            c(0.5, -2.0),
            c(-1.5, 0.25),
            c(2.0, 2.0),
            c(0.0, -0.75),
            c(4.0, 0.0),
        ];
        let n = diagonal.len();
        let mut upper = vec![c(0.0, 0.0); n * n];
        let mut mix = vec![c(0.0, 0.0); n * n];
        let mut unmix = vec![c(0.0, 0.0); n * n];
        for row in 0..n {
            upper[row * n + row] = diagonal[row];
            for col in (row + 1)..n {
                upper[row * n + col] = c(0.3 * (row + col) as f64, -0.2 * col as f64);
            }
            mix[row * n + row] = c(1.0, 0.0);
            unmix[row * n + row] = c(1.0, 0.0);
        }
        mix[4 * n] = c(0.5, 1.0);
        unmix[4 * n] = c(-0.5, -1.0);
        mix[3 * n + 1] = c(0.0, -2.0);
        unmix[3 * n + 1] = c(0.0, 2.0);
        let matrix = matmul(&matmul(&mix, &upper, n), &unmix, n);
        let mut solver = GeneralEigen::new(8).unwrap();
        let mut out = [c(0.0, 0.0); 5];
        solver.eigenvalues(&matrix, &mut out).unwrap();
        assert_matches(&out, &diagonal, 1e-10);
    }

    #[test]
    fn one_by_one_and_bad_shapes() {
        let mut solver = GeneralEigen::new(3).unwrap();
        let mut out = [c(0.0, 0.0); 3];
        solver.eigenvalues(&[c(2.0, -1.0)], &mut out).unwrap();
        assert_eq!(out[0], c(2.0, -1.0));
        assert_eq!(
            solver.eigenvalues(&[c(1.0, 0.0); 5], &mut out),
            Err(LinalgError::Order(2))
        );
        assert_eq!(
            solver.eigenvalues(&[c(1.0, 0.0); 16], &mut out),
            Err(LinalgError::Order(4))
        );
        assert_eq!(
            solver.eigenvalues(&[c(f64::NAN, 0.0); 4], &mut out),
            Err(LinalgError::NonFinite)
        );
        assert!(GeneralEigen::new(MAX_SCHUR_ORDER + 1).is_err());
    }

    #[test]
    fn a_cyclic_permutation_needs_the_exceptional_shift() {
        let matrix = real(&[
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ]);
        let mut solver = GeneralEigen::new(4).unwrap();
        let mut out = [c(0.0, 0.0); 4];
        solver.eigenvalues(&matrix, &mut out).unwrap();
        assert_matches(
            &out,
            &[c(1.0, 0.0), c(0.0, 1.0), c(-1.0, 0.0), c(0.0, -1.0)],
            1e-10,
        );
    }

    #[test]
    fn a_nilpotent_block_deflates() {
        let matrix = real(&[[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]]);
        let mut solver = GeneralEigen::new(3).unwrap();
        let mut out = [c(9.0, 9.0); 3];
        solver.eigenvalues(&matrix, &mut out).unwrap();
        assert!(out.iter().all(|value| value.norm() < 1e-12));
    }
}
