use std::f64::consts::FRAC_PI_2;

use num_complex::Complex;

use super::CovarianceError;
use crate::linalg::{CMat, LinalgError, MAX_POLY_DEGREE, Roots};
use crate::manifold::{Geometry, MAX_ELEMENTS, Shape, wavenumber};
use crate::special::{bessel_j, wrap_deg};

pub const BESSEL_FLOOR: f64 = 0.1;

const BIAS_BEARINGS: usize = 36;
const MIN_ELEMENTS: usize = 3;

type C64 = Complex<f64>;

pub struct PhaseMode {
    elements: usize,
    modes: usize,
    radius_m: f64,
    angles_rad: [f64; MAX_ELEMENTS],
    unit: [C64; MAX_ELEMENTS * MAX_ELEMENTS],
    vandermonde: [C64; MAX_ELEMENTS * MAX_ELEMENTS],
    vandermonde_modes: Option<usize>,
    magnitudes: [f32; MAX_ELEMENTS],
    noise: [f32; MAX_ELEMENTS],
    mode_bias_deg: f32,
    roots: Roots,
    coeffs: [C64; MAX_POLY_DEGREE + 1],
    found: [C64; MAX_POLY_DEGREE],
}

impl PhaseMode {
    pub fn new(geometry: &Geometry, freq_hz: f64) -> Result<Self, CovarianceError> {
        let Shape::Uca {
            radius_m,
            first_deg,
            winding,
        } = geometry.shape()
        else {
            return Err(CovarianceError::NotStructured);
        };
        let elements = geometry.len();
        if elements < MIN_ELEMENTS {
            return Err(CovarianceError::NotStructured);
        }
        let turn = if winding == crate::manifold::Winding::Clockwise {
            1.0
        } else {
            -1.0
        };
        let mut angles_rad = [0.0; MAX_ELEMENTS];
        for (i, angle) in angles_rad.iter_mut().enumerate().take(elements) {
            *angle = (first_deg + turn * 360.0 * i as f64 / elements as f64).to_radians();
        }
        let mut phase_mode = Self {
            elements,
            modes: (elements - 1) / 2,
            radius_m,
            angles_rad,
            unit: [C64::new(0.0, 0.0); MAX_ELEMENTS * MAX_ELEMENTS],
            vandermonde: [C64::new(0.0, 0.0); MAX_ELEMENTS * MAX_ELEMENTS],
            vandermonde_modes: None,
            magnitudes: [0.0; MAX_ELEMENTS],
            noise: [0.0; MAX_ELEMENTS],
            mode_bias_deg: 0.0,
            roots: Roots::new(MAX_POLY_DEGREE)?,
            coeffs: [C64::new(0.0, 0.0); MAX_POLY_DEGREE + 1],
            found: [C64::new(0.0, 0.0); MAX_POLY_DEGREE],
        };
        phase_mode.retune(freq_hz)?;
        Ok(phase_mode)
    }

    pub fn retune(&mut self, freq_hz: f64) -> Result<(), CovarianceError> {
        let beta = wavenumber(freq_hz) * self.radius_m;
        let mut bessel = [0.0f64; MAX_ELEMENTS];
        for (m, value) in bessel.iter_mut().enumerate().take(self.modes + 1) {
            *value = bessel_j(m as u32, beta)?;
        }
        self.fill_unit(&bessel);
        self.fill_vandermonde(&bessel);
        self.mode_bias_deg = self.measure_bias(beta)?;
        Ok(())
    }

    #[must_use]
    pub const fn virtual_len(&self) -> usize {
        2 * self.modes + 1
    }

    #[must_use]
    pub const fn modes(&self) -> usize {
        self.modes
    }

    #[must_use]
    pub const fn vandermonde_modes(&self) -> Option<usize> {
        self.vandermonde_modes
    }

    #[must_use]
    pub fn vandermonde_len(&self) -> usize {
        self.vandermonde_modes.map_or(0, |modes| 2 * modes + 1)
    }

    pub fn transform(&self, r: &CMat, out: &mut CMat) -> Result<(), CovarianceError> {
        congruence(&self.unit, self.virtual_len(), self.elements, r, out)
    }

    pub fn transform_vandermonde(&self, r: &CMat, out: &mut CMat) -> Result<(), CovarianceError> {
        if self.vandermonde_modes.is_none() {
            return Err(CovarianceError::BesselNull);
        }
        congruence(
            &self.vandermonde,
            self.vandermonde_len(),
            self.elements,
            r,
            out,
        )
    }

    pub fn project(
        &self,
        steering: &[Complex<f32>],
        out: &mut [Complex<f32>],
    ) -> Result<usize, CovarianceError> {
        apply(&self.unit, self.virtual_len(), self.elements, steering, out)
    }

    pub fn project_vandermonde(
        &self,
        steering: &[Complex<f32>],
        out: &mut [Complex<f32>],
    ) -> Result<usize, CovarianceError> {
        if self.vandermonde_modes.is_none() {
            return Err(CovarianceError::BesselNull);
        }
        apply(
            &self.vandermonde,
            self.vandermonde_len(),
            self.elements,
            steering,
            out,
        )
    }

    #[must_use]
    pub fn noise_diagonal(&self) -> &[f32] {
        &self.noise[..self.vandermonde_len()]
    }

    #[must_use]
    pub fn mode_magnitudes(&self) -> &[f32] {
        &self.magnitudes[..self.virtual_len()]
    }

    #[must_use]
    pub const fn mode_bias_deg(&self) -> f32 {
        self.mode_bias_deg
    }

    fn fill_unit(&mut self, bessel: &[f64; MAX_ELEMENTS]) {
        let n = self.elements;
        let scale = 1.0 / (n as f64).sqrt();
        for m in -(self.modes as i32)..=self.modes as i32 {
            let j_m = signed_bessel(bessel, m);
            let sign = if j_m < 0.0 { -1.0 } else { 1.0 };
            let row = (m + self.modes as i32) as usize;
            let phase = C64::from_polar(sign * scale, -f64::from(m) * FRAC_PI_2);
            self.magnitudes[row] = j_m.abs() as f32;
            for i in 0..n {
                let turn = C64::from_polar(1.0, f64::from(m) * self.angles_rad[i]);
                self.unit[row * n + i] = phase * turn;
            }
        }
    }

    fn fill_vandermonde(&mut self, bessel: &[f64; MAX_ELEMENTS]) {
        let kept = (0..=self.modes)
            .take_while(|&m| bessel[m].abs() >= BESSEL_FLOOR)
            .last()
            .filter(|&m| m >= 1);
        self.vandermonde_modes = kept;
        let Some(kept) = kept else {
            return;
        };
        let n = self.elements;
        for m in -(kept as i32)..=kept as i32 {
            let j_m = signed_bessel(bessel, m);
            let row = (m + kept as i32) as usize;
            let gain = C64::from_polar(1.0 / (n as f64 * j_m), -f64::from(m) * FRAC_PI_2);
            self.noise[row] = (1.0 / (n as f64 * j_m * j_m)) as f32;
            for i in 0..n {
                let turn = C64::from_polar(1.0, f64::from(m) * self.angles_rad[i]);
                self.vandermonde[row * n + i] = gain * turn;
            }
        }
    }

    fn measure_bias(&mut self, beta: f64) -> Result<f32, CovarianceError> {
        let mut worst = 0.0f64;
        for step in 0..BIAS_BEARINGS {
            let bearing = 360.0 * step as f64 / BIAS_BEARINGS as f64;
            if let Some(found) = self.root_music_bearing(beta, bearing)? {
                worst = worst.max(wrap_deg(found - bearing).abs());
            }
        }
        Ok(worst as f32)
    }

    fn root_music_bearing(
        &mut self,
        beta: f64,
        bearing: f64,
    ) -> Result<Option<f64>, CovarianceError> {
        let n = self.elements;
        let m = self.virtual_len();
        if m < 2 {
            return Ok(None);
        }
        let theta = bearing.to_radians();
        let mut steering = [C64::new(0.0, 0.0); MAX_ELEMENTS];
        for (value, angle) in steering.iter_mut().zip(&self.angles_rad[..n]) {
            *value = C64::from_polar(1.0, beta * (theta - angle).cos());
        }
        let mut virtual_ = [C64::new(0.0, 0.0); MAX_ELEMENTS];
        for (p, value) in virtual_.iter_mut().enumerate().take(m) {
            *value = (0..n).map(|i| self.unit[p * n + i] * steering[i]).sum();
        }
        let power: f64 = virtual_[..m].iter().map(C64::norm_sqr).sum();
        if power <= 0.0 {
            return Ok(None);
        }
        self.coeffs.fill(C64::new(0.0, 0.0));
        for p in 0..m {
            for q in 0..m {
                let identity = if p == q { 1.0 } else { 0.0 };
                let projector = identity - virtual_[p] * virtual_[q].conj() / power;
                let weight = f64::from(self.magnitudes[p]) * f64::from(self.magnitudes[q]);
                self.coeffs[q + m - 1 - p] += projector * weight;
            }
        }
        let degree = match self.roots.solve(&self.coeffs[..2 * m - 1], &mut self.found) {
            Ok(degree) => degree,
            Err(LinalgError::Degenerate) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(self.found[..degree]
            .iter()
            .min_by(|a, b| (a.norm() - 1.0).abs().total_cmp(&(b.norm() - 1.0).abs()))
            .map(|root| root.arg().to_degrees()))
    }
}

fn signed_bessel(bessel: &[f64; MAX_ELEMENTS], m: i32) -> f64 {
    let value = bessel[m.unsigned_abs() as usize];
    if m < 0 && m % 2 != 0 { -value } else { value }
}

fn congruence(
    transform: &[C64],
    rows: usize,
    cols: usize,
    r: &CMat,
    out: &mut CMat,
) -> Result<(), CovarianceError> {
    if r.order() != cols {
        return Err(LinalgError::Order(r.order()).into());
    }
    let mut left = [C64::new(0.0, 0.0); MAX_ELEMENTS * MAX_ELEMENTS];
    for p in 0..rows {
        for k in 0..cols {
            left[p * cols + k] = (0..cols)
                .map(|i| {
                    let value = r.get(i, k);
                    transform[p * cols + i] * C64::new(f64::from(value.re), f64::from(value.im))
                })
                .sum();
        }
    }
    out.resize(rows)?;
    for p in 0..rows {
        for q in 0..rows {
            let value: C64 = (0..cols)
                .map(|k| left[p * cols + k] * transform[q * cols + k].conj())
                .sum();
            out.set(p, q, Complex::new(value.re as f32, value.im as f32));
        }
    }
    Ok(())
}

fn apply(
    transform: &[C64],
    rows: usize,
    cols: usize,
    steering: &[Complex<f32>],
    out: &mut [Complex<f32>],
) -> Result<usize, CovarianceError> {
    if steering.len() != cols || out.len() < rows {
        return Err(LinalgError::Order(steering.len()).into());
    }
    for (p, value) in out.iter_mut().enumerate().take(rows) {
        let sum: C64 = steering
            .iter()
            .enumerate()
            .map(|(i, a)| transform[p * cols + i] * C64::new(f64::from(a.re), f64::from(a.im)))
            .sum();
        *value = Complex::new(sum.re as f32, sum.im as f32);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{Direction, LIGHT_SPEED_M_S, Winding, steer};

    fn uca_for_beta(count: usize, beta: f64, freq: f64) -> Geometry {
        let radius = beta / wavenumber(freq);
        Geometry::uca(radius, count, 0.0, Winding::Clockwise).unwrap()
    }

    fn uca_for_chord(count: usize, chord_wavelengths: f64, freq: f64) -> Geometry {
        let chord = chord_wavelengths * LIGHT_SPEED_M_S / freq;
        let radius = chord / (2.0 * (std::f64::consts::PI / count as f64).sin());
        Geometry::uca(radius, count, 0.0, Winding::Clockwise).unwrap()
    }

    fn off_diagonal_max(matrix: &CMat) -> f32 {
        let n = matrix.order();
        (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| matrix.get(i, j).norm())
            .fold(0.0, f32::max)
    }

    #[test]
    fn phase_mode_maps_uca_steering_to_a_vandermonde_vector() {
        let freq = 433.92e6;
        let geometry = uca_for_beta(8, 2.0, freq);
        let phase_mode = PhaseMode::new(&geometry, freq).unwrap();
        assert_eq!(phase_mode.vandermonde_modes(), Some(3));
        let mut steering = [Complex::new(0.0f32, 0.0); 8];
        let mut virtual_ = [Complex::new(0.0f32, 0.0); 7];
        for step in 0..72 {
            let theta = 5.0 * f64::from(step);
            steer(
                geometry.positions(),
                freq,
                Direction::horizon(theta),
                &mut steering,
            );
            assert_eq!(
                phase_mode
                    .project_vandermonde(&steering, &mut virtual_)
                    .unwrap(),
                7
            );
            for m in -3i32..=3 {
                let want = C64::from_polar(1.0, f64::from(m) * theta.to_radians());
                let got = virtual_[(m + 3) as usize];
                let error = (C64::new(f64::from(got.re), f64::from(got.im)) - want).norm();
                let alias = bessel_j(8 - m.unsigned_abs(), 2.0).unwrap().abs()
                    / bessel_j(m.unsigned_abs(), 2.0).unwrap().abs();
                assert!(
                    error <= alias + 1e-3,
                    "mode {m} at {theta}: {error} > {alias}"
                );
            }
        }
        let alias_three = bessel_j(5, 2.0).unwrap() / bessel_j(3, 2.0).unwrap();
        assert!((alias_three - 0.0546).abs() < 1e-3);
    }

    #[test]
    fn phase_mode_limits_modes_when_bessel_is_small() {
        let freq = 433.92e6;
        let limited = PhaseMode::new(&uca_for_beta(8, 0.3, freq), freq).unwrap();
        assert_eq!(limited.modes(), 3);
        assert_eq!(limited.vandermonde_modes(), Some(1));
        assert_eq!(limited.noise_diagonal().len(), 3);
        let null = PhaseMode::new(&uca_for_beta(8, 0.15, freq), freq).unwrap();
        let r = CMat::identity(8).unwrap();
        let mut out = CMat::zeros(1).unwrap();
        assert_eq!(
            null.transform_vandermonde(&r, &mut out),
            Err(CovarianceError::BesselNull)
        );
        null.transform(&r, &mut out).unwrap();
        assert_eq!(out.order(), 7);
        assert!(null.noise_diagonal().is_empty());
    }

    #[test]
    fn whitened_phase_mode_is_unitary_at_a_bessel_null() {
        let freq = 433.92e6;
        let geometry = uca_for_chord(5, 0.45, freq);
        let phase_mode = PhaseMode::new(&geometry, freq).unwrap();
        assert!(phase_mode.mode_magnitudes()[2] < 0.01);
        assert_eq!(phase_mode.vandermonde_modes(), None);
        let mut out = CMat::zeros(1).unwrap();
        phase_mode
            .transform(&CMat::identity(5).unwrap(), &mut out)
            .unwrap();
        assert_eq!(out.order(), 5);
        for i in 0..5 {
            for j in 0..5 {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((out.get(i, j) - Complex::new(want, 0.0)).norm() < 1e-6);
            }
        }
    }

    #[test]
    fn phase_mode_noise_is_diagonal() {
        let freq = 433.92e6;
        let geometry = uca_for_beta(8, 2.0, freq);
        let phase_mode = PhaseMode::new(&geometry, freq).unwrap();
        let mut out = CMat::zeros(1).unwrap();
        phase_mode
            .transform_vandermonde(&CMat::identity(8).unwrap(), &mut out)
            .unwrap();
        assert!(off_diagonal_max(&out) < 1e-6);
        for (k, &noise) in phase_mode.noise_diagonal().iter().enumerate() {
            assert!((out.get(k, k).re - noise).abs() < 1e-5 * noise);
        }
    }

    #[test]
    fn mode_bias_is_small_for_a_dense_ring_and_large_for_a_sparse_one() {
        let freq = 433.92e6;
        let dense = PhaseMode::new(&uca_for_beta(8, 2.0, freq), freq).unwrap();
        assert!(
            (dense.mode_bias_deg() - 0.114).abs() < 0.01,
            "{}",
            dense.mode_bias_deg()
        );
        let mut kraken = PhaseMode::new(&uca_for_chord(5, 0.5, freq), freq).unwrap();
        assert!(
            (kraken.mode_bias_deg() - 0.949).abs() < 0.01,
            "{}",
            kraken.mode_bias_deg()
        );
        kraken.retune(freq / 4.0).unwrap();
        assert!(kraken.mode_bias_deg() < 0.01, "{}", kraken.mode_bias_deg());
    }

    #[test]
    fn only_a_circle_has_phase_modes() {
        let line = Geometry::ula(0.5, 5, 90.0).unwrap();
        assert!(matches!(
            PhaseMode::new(&line, 3e8),
            Err(CovarianceError::NotStructured)
        ));
        let pair = Geometry::uca(0.5, 2, 0.0, Winding::Clockwise).unwrap();
        assert!(matches!(
            PhaseMode::new(&pair, 3e8),
            Err(CovarianceError::NotStructured)
        ));
        let huge = Geometry::uca(50.0, 5, 0.0, Winding::Clockwise).unwrap();
        assert!(matches!(
            PhaseMode::new(&huge, 3e9),
            Err(CovarianceError::Special(_))
        ));
    }
}
