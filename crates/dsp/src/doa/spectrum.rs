use std::ops::Range;

use num_complex::Complex;

use crate::linalg::{CMat, Cholesky, Eigen};
use crate::manifold::SteeringGrid;

const MUSIC_FLOOR: f32 = 1e-6;

#[derive(Clone, Copy)]
pub enum Pseudospectrum<'a> {
    Bartlett(&'a CMat),
    Capon(&'a Cholesky),
    Music { eigen: &'a Eigen, signals: usize },
}

impl Pseudospectrum<'_> {
    #[must_use]
    pub fn power(&self, a: &[Complex<f32>], scratch: &mut [Complex<f32>]) -> f32 {
        match *self {
            Self::Bartlett(r) => bartlett(r, a),
            Self::Capon(loaded) => capon(loaded, a, scratch),
            Self::Music { eigen, signals } => music(eigen, signals, a),
        }
    }

    pub fn scan(&self, grid: &SteeringGrid, scratch: &mut [Complex<f32>], out: &mut [f32]) {
        for (point, value) in out.iter_mut().enumerate().take(grid.points()) {
            *value = self.power(grid.vector(point), scratch);
        }
    }
}

#[must_use]
pub fn norm_sqr(a: &[Complex<f32>]) -> f32 {
    a.iter().map(|value| value.norm_sqr()).sum()
}

#[must_use]
pub fn inner(e: &[Complex<f32>], a: &[Complex<f32>]) -> Complex<f32> {
    e.iter().zip(a).map(|(e, a)| e.conj() * a).sum()
}

#[must_use]
pub fn bartlett(r: &CMat, a: &[Complex<f32>]) -> f32 {
    let norm = norm_sqr(&a[..r.order()]);
    if norm > 0.0 {
        r.quad(a) / (norm * norm)
    } else {
        0.0
    }
}

#[must_use]
pub fn capon(loaded: &Cholesky, a: &[Complex<f32>], scratch: &mut [Complex<f32>]) -> f32 {
    let inverse = loaded.quad_inverse(a, scratch);
    if inverse > 0.0 { inverse.recip() } else { 0.0 }
}

#[must_use]
pub fn music(eigen: &Eigen, signals: usize, a: &[Complex<f32>]) -> f32 {
    let m = eigen.order();
    if m < 2 || a.len() < m {
        return 0.0;
    }
    let a = &a[..m];
    let norm = norm_sqr(a);
    if norm <= 0.0 {
        return 0.0;
    }
    let signals = signals.clamp(1, m - 1);
    let projected =
        |range: Range<usize>| -> f32 { range.map(|k| inner(eigen.vector(k), a).norm_sqr()).sum() };
    let residual = if 2 * signals <= m {
        norm - projected(m - signals..m)
    } else {
        projected(0..m - signals)
    };
    norm / residual.max(MUSIC_FLOOR * norm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::HermitianEigen;
    use crate::manifold::{Direction, Geometry, GridSpec, Manifold, Winding, steer};

    const FREQ: f64 = 433.92e6;

    fn with_source(r: &mut CMat, a: &[Complex<f32>], power: f32) {
        let n = r.order();
        for i in 0..n {
            for j in 0..n {
                r.add(i, j, a[i] * a[j].conj() * power);
            }
        }
    }

    fn noise(n: usize, power: f32) -> CMat {
        let mut r = CMat::identity(n).unwrap();
        r.scale(power);
        r
    }

    fn kraken() -> Geometry {
        Geometry::uca(0.2939, 5, 0.0, Winding::Clockwise).unwrap()
    }

    fn steering(geometry: &Geometry, azimuth_deg: f64) -> [Complex<f32>; 5] {
        let mut a = [Complex::new(0.0f32, 0.0); 5];
        steer(
            geometry.positions(),
            FREQ,
            Direction::horizon(azimuth_deg),
            &mut a,
        );
        a
    }

    #[test]
    fn bartlett_and_capon_give_signal_plus_noise_share_at_the_source() {
        let geometry = kraken();
        let a = steering(&geometry, 37.0);
        let mut r = noise(5, 1.0);
        with_source(&mut r, &a, 4.0);
        let want = 4.0 + 1.0 / 5.0;
        assert!((bartlett(&r, &a) - want).abs() < 1e-4);
        let mut chol = Cholesky::new(5).unwrap();
        chol.factor_cmat(&r).unwrap();
        let mut scratch = [Complex::new(0.0f32, 0.0); 5];
        assert!((capon(&chol, &a, &mut scratch) - want).abs() < 1e-3);
        assert_eq!(bartlett(&r, &[Complex::new(0.0, 0.0); 5]), 0.0);
    }

    #[test]
    fn music_peaks_at_the_sources_with_either_projection() {
        let geometry = kraken();
        let a = steering(&geometry, 137.0);
        let b = steering(&geometry, 250.0);
        let c = steering(&geometry, 170.0);
        let mut r = noise(5, 0.1);
        with_source(&mut r, &a, 10.0);
        with_source(&mut r, &b, 3.0);
        let mut solver = HermitianEigen::new(5).unwrap();
        let mut eigen = Eigen::new();
        solver.solve(&r, &mut eigen).unwrap();
        for signals in [2, 3] {
            let at_source = music(&eigen, signals, &a);
            let beside = music(&eigen, signals, &c);
            assert!(
                at_source > 100.0 * beside,
                "{signals}: {at_source} {beside}"
            );
        }
        let two = music(&eigen, 2, &b);
        let three = music(&eigen, 3, &b);
        assert!(two > 100.0 && three > 100.0, "{two} {three}");
        assert_eq!(music(&eigen, 2, &[Complex::new(0.0, 0.0); 5]), 0.0);
    }

    #[test]
    fn scan_fills_every_grid_point() {
        let manifold = Manifold::ideal(kraken());
        let grid = SteeringGrid::new(&manifold, GridSpec::ring(2.0), FREQ).unwrap();
        let a = steering(manifold.geometry(), 90.0);
        let mut r = noise(5, 0.01);
        with_source(&mut r, &a, 1.0);
        let mut out = vec![0.0f32; grid.points()];
        let mut scratch = [Complex::new(0.0f32, 0.0); 5];
        Pseudospectrum::Bartlett(&r).scan(&grid, &mut scratch, &mut out);
        let best = out
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.total_cmp(y.1))
            .map(|(point, _)| point)
            .unwrap();
        assert_eq!(best, 45);
    }
}
