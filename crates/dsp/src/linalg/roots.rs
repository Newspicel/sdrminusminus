use std::f64::consts::{FRAC_PI_2, TAU};

use num_complex::Complex;

use super::{GeneralEigen, LinalgError, MAX_SCHUR_ORDER};

const ABERTH_BUDGET: u32 = 200;
const ROOT_TOL: f64 = 1e-13;
const NEGLIGIBLE: f64 = 1e-300;

pub struct Roots {
    max_degree: usize,
    z: Vec<Complex<f64>>,
    companion: Vec<Complex<f64>>,
    fallback: GeneralEigen,
    aberth_budget: u32,
    fell_back: bool,
}

impl Roots {
    pub fn new(max_degree: usize) -> Result<Self, LinalgError> {
        if max_degree == 0 || max_degree > MAX_SCHUR_ORDER {
            return Err(LinalgError::Order(max_degree));
        }
        Ok(Self {
            max_degree,
            z: vec![Complex::new(0.0, 0.0); max_degree],
            companion: vec![Complex::new(0.0, 0.0); max_degree * max_degree],
            fallback: GeneralEigen::new(max_degree)?,
            aberth_budget: ABERTH_BUDGET,
            fell_back: false,
        })
    }

    #[must_use]
    pub const fn fell_back(&self) -> bool {
        self.fell_back
    }

    pub fn solve(
        &mut self,
        coeffs: &[Complex<f64>],
        out: &mut [Complex<f64>],
    ) -> Result<usize, LinalgError> {
        self.fell_back = false;
        if !coeffs.iter().all(|value| value.is_finite()) {
            return Err(LinalgError::NonFinite);
        }
        let largest = coeffs.iter().map(|value| value.norm()).fold(0.0, f64::max);
        let top = coeffs
            .iter()
            .rposition(|value| value.norm() > NEGLIGIBLE * largest)
            .filter(|&top| top > 0)
            .ok_or(LinalgError::Degenerate)?;
        if top > self.max_degree {
            return Err(LinalgError::Order(top));
        }
        if out.len() < top {
            return Err(LinalgError::Order(out.len()));
        }
        let low = coeffs
            .iter()
            .position(|value| value.norm() > 0.0)
            .unwrap_or(top);
        out[..low].fill(Complex::new(0.0, 0.0));
        if low == top {
            return Ok(top);
        }
        let poly = &coeffs[low..=top];
        let found = &mut out[low..top];
        match self.aberth(poly, found) {
            Err(LinalgError::NotConverged(_)) => {
                self.fell_back = true;
                self.companion_roots(poly, found)?;
            }
            other => other?,
        }
        Ok(top)
    }

    fn aberth(
        &mut self,
        poly: &[Complex<f64>],
        out: &mut [Complex<f64>],
    ) -> Result<(), LinalgError> {
        let degree = poly.len() - 1;
        let z = &mut self.z[..degree];
        let radius = (poly[0].norm() / poly[degree].norm()).powf(1.0 / degree as f64);
        for (k, root) in z.iter_mut().enumerate() {
            let angle = TAU * k as f64 / degree as f64 + FRAC_PI_2 / degree as f64;
            *root = Complex::from_polar(radius, angle);
        }
        let all = (1u64 << degree) - 1;
        let mut converged = 0u64;
        for _ in 0..self.aberth_budget {
            for k in 0..degree {
                if converged & (1 << k) != 0 {
                    continue;
                }
                let Some(delta) = correction(poly, z, k) else {
                    continue;
                };
                z[k] -= delta;
                if delta.norm() <= ROOT_TOL * (1.0 + z[k].norm()) {
                    converged |= 1 << k;
                }
            }
            if converged == all {
                out.copy_from_slice(z);
                return Ok(());
            }
        }
        Err(LinalgError::NotConverged(self.aberth_budget))
    }

    fn companion_roots(
        &mut self,
        poly: &[Complex<f64>],
        out: &mut [Complex<f64>],
    ) -> Result<(), LinalgError> {
        let degree = poly.len() - 1;
        let lead = poly[degree];
        let matrix = &mut self.companion[..degree * degree];
        matrix.fill(Complex::new(0.0, 0.0));
        for col in 0..degree {
            matrix[col] = -poly[degree - 1 - col] / lead;
        }
        for row in 1..degree {
            matrix[row * degree + row - 1] = Complex::new(1.0, 0.0);
        }
        self.fallback.eigenvalues(matrix, out)
    }
}

fn correction(poly: &[Complex<f64>], z: &[Complex<f64>], k: usize) -> Option<Complex<f64>> {
    let zk = z[k];
    let repulsion: Complex<f64> = z
        .iter()
        .enumerate()
        .filter(|&(j, _)| j != k)
        .map(|(_, &zj)| (zk - zj).inv())
        .sum();
    if !repulsion.is_finite() {
        return None;
    }
    let (numerator, denominator) = if zk.norm() <= 1.0 {
        horner(poly, zk)
    } else {
        let y = zk.inv();
        let (q, dq) = horner_reversed(poly, y);
        (zk * q, (poly.len() - 1) as f64 * q - y * dq)
    };
    if numerator.norm() == 0.0 {
        return Some(Complex::new(0.0, 0.0));
    }
    let delta = numerator / (denominator - numerator * repulsion);
    delta.is_finite().then_some(delta)
}

fn horner(poly: &[Complex<f64>], z: Complex<f64>) -> (Complex<f64>, Complex<f64>) {
    let mut value = Complex::new(0.0, 0.0);
    let mut slope = Complex::new(0.0, 0.0);
    for &coeff in poly.iter().rev() {
        slope = slope * z + value;
        value = value * z + coeff;
    }
    (value, slope)
}

fn horner_reversed(poly: &[Complex<f64>], y: Complex<f64>) -> (Complex<f64>, Complex<f64>) {
    let mut value = Complex::new(0.0, 0.0);
    let mut slope = Complex::new(0.0, 0.0);
    for &coeff in poly {
        slope = slope * y + value;
        value = value * y + coeff;
    }
    (value, slope)
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::linalg::MAX_POLY_DEGREE;

    fn c(re: f64, im: f64) -> Complex<f64> {
        Complex::new(re, im)
    }

    fn expand(roots: &[Complex<f64>]) -> Vec<Complex<f64>> {
        let mut coeffs = vec![c(1.0, 0.0)];
        for &root in roots {
            let mut next = vec![c(0.0, 0.0); coeffs.len() + 1];
            for (k, &value) in coeffs.iter().enumerate() {
                next[k + 1] += value;
                next[k] -= value * root;
            }
            coeffs = next;
        }
        coeffs.iter().map(|value| value * c(0.5, -1.5)).collect()
    }

    fn worst_match(found: &[Complex<f64>], want: &[Complex<f64>]) -> f64 {
        let mut used = vec![false; want.len()];
        let mut worst = 0.0f64;
        for value in found {
            let (index, distance) = want
                .iter()
                .enumerate()
                .filter(|(index, _)| !used[*index])
                .map(|(index, target)| (index, (value - target).norm()))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            used[index] = true;
            worst = worst.max(distance);
        }
        worst
    }

    fn mixed_roots() -> Vec<Complex<f64>> {
        vec![
            c(0.3, 0.2),
            c(0.0, -0.5),
            c(0.7, 0.0),
            Complex::from_polar(1.0, 1.0),
            Complex::from_polar(1.0, -2.5),
            c(-1.0, 0.0),
            c(1.8, -0.4),
            c(-2.2, 1.1),
        ]
    }

    #[test]
    fn aberth_finds_roots_built_from_known_roots() {
        let want = mixed_roots();
        let mut roots = Roots::new(MAX_POLY_DEGREE).unwrap();
        let mut out = [c(0.0, 0.0); MAX_POLY_DEGREE];
        assert_eq!(roots.solve(&expand(&want), &mut out), Ok(8));
        assert!(!roots.fell_back());
        assert!(worst_match(&out[..8], &want) < 1e-9);
    }

    #[test]
    fn aberth_handles_a_double_root() {
        let want = [c(0.5, 0.0), c(0.5, 0.0), c(-1.0, 0.0), c(0.0, 2.0)];
        let mut roots = Roots::new(8).unwrap();
        let mut out = [c(0.0, 0.0); 4];
        assert_eq!(roots.solve(&expand(&want), &mut out), Ok(4));
        assert!(worst_match(&out, &want) < 1e-5);
    }

    #[test]
    fn roots_fall_back_to_the_companion_matrix() {
        let want = mixed_roots();
        let mut roots = Roots::new(MAX_POLY_DEGREE).unwrap();
        roots.aberth_budget = 1;
        let mut out = [c(0.0, 0.0); 8];
        assert_eq!(roots.solve(&expand(&want), &mut out), Ok(8));
        assert!(roots.fell_back());
        assert!(worst_match(&out, &want) < 1e-9);
    }

    #[test]
    fn zero_roots_split_off_exactly() {
        let mut roots = Roots::new(4).unwrap();
        let mut out = [c(9.0, 9.0); 3];
        let coeffs = [c(0.0, 0.0), c(0.0, 0.0), c(-2.0, 0.0), c(2.0, 0.0)];
        assert_eq!(roots.solve(&coeffs, &mut out), Ok(3));
        assert_eq!(out[0], c(0.0, 0.0));
        assert_eq!(out[1], c(0.0, 0.0));
        assert!((out[2] - c(1.0, 0.0)).norm() < 1e-12);
        let monomial = [c(0.0, 0.0), c(0.0, 0.0), c(5.0, 0.0)];
        assert_eq!(roots.solve(&monomial, &mut out), Ok(2));
        assert_eq!(&out[..2], &[c(0.0, 0.0); 2]);
    }

    #[test]
    fn negligible_leading_coefficients_are_dropped() {
        let mut roots = Roots::new(4).unwrap();
        let mut out = [c(0.0, 0.0); 2];
        let coeffs = [c(-3.0, 0.0), c(1.5, 0.0), c(1e-310, 0.0), c(0.0, 0.0)];
        assert_eq!(roots.solve(&coeffs, &mut out), Ok(1));
        assert!((out[0] - c(2.0, 0.0)).norm() < 1e-12);
    }

    #[test]
    fn nothing_to_solve_is_degenerate() {
        let mut roots = Roots::new(4).unwrap();
        let mut out = [c(0.0, 0.0); 4];
        assert_eq!(roots.solve(&[], &mut out), Err(LinalgError::Degenerate));
        assert_eq!(
            roots.solve(&[c(3.0, 0.0)], &mut out),
            Err(LinalgError::Degenerate)
        );
        assert_eq!(
            roots.solve(&[c(0.0, 0.0); 3], &mut out),
            Err(LinalgError::Degenerate)
        );
        assert_eq!(
            roots.solve(&[c(1.0, 0.0), c(f64::NAN, 0.0)], &mut out),
            Err(LinalgError::NonFinite)
        );
    }

    #[test]
    fn a_degree_above_the_capacity_is_refused() {
        let mut roots = Roots::new(2).unwrap();
        let mut out = [c(0.0, 0.0); 4];
        assert_eq!(
            roots.solve(&[c(1.0, 0.0); 4], &mut out),
            Err(LinalgError::Order(3))
        );
        let mut short = [c(0.0, 0.0); 1];
        assert_eq!(
            roots.solve(&[c(1.0, 0.0); 3], &mut short),
            Err(LinalgError::Order(1))
        );
        assert!(Roots::new(0).is_err());
        assert!(Roots::new(MAX_SCHUR_ORDER + 1).is_err());
    }
}
