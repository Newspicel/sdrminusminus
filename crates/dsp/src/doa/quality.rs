use std::f64::consts::SQRT_2;

use num_complex::Complex;

use super::spectrum::inner;
use super::{CONFIDENCE_WINDOW_DEG, LIKELIHOOD_FLOOR, LIKELIHOOD_POINTS, SIGMA_FLOOR_DEG};
use crate::linalg::{CMat, Cholesky, LinalgError, MAX_ORDER};
use crate::special::erf;

pub const MAX_SIGMA_DEG: f64 = 180.0;

const MIN_SPREAD: f64 = 1e-12;
const MIN_SNR: f64 = 1e-12;
const GRAM_LOADING: f32 = 1e-6;

#[must_use]
pub fn rate_spread(rates: &[f64]) -> f64 {
    if rates.is_empty() {
        return 0.0;
    }
    let mean = rates.iter().sum::<f64>() / rates.len() as f64;
    rates.iter().map(|g| (g - mean) * (g - mean)).sum()
}

#[must_use]
pub fn crb_sigma_rad(rates: &[f64], snr: f64, elements: usize, snapshots: f64) -> f64 {
    let spread = rate_spread(rates).max(MIN_SPREAD);
    let snr = if snr.is_finite() {
        snr.max(MIN_SNR)
    } else {
        MIN_SNR
    };
    let n = elements.max(1) as f64;
    let variance = (1.0 / snr + 1.0 / (n * snr * snr)) / (2.0 * snapshots.max(1.0) * spread);
    variance.sqrt()
}

#[must_use]
pub fn model_sigma_rad(rates: &[f64], cal_sigma_deg: f64) -> f64 {
    cal_sigma_deg.abs().to_radians() / rate_spread(rates).max(MIN_SPREAD).sqrt()
}

#[must_use]
pub fn total_sigma_deg(crb_rad: f64, model_rad: f64) -> f64 {
    let crb = crb_rad.to_degrees();
    let model = model_rad.to_degrees();
    let total = (crb * crb + model * model + SIGMA_FLOOR_DEG * SIGMA_FLOOR_DEG).sqrt();
    if total.is_finite() {
        total.min(MAX_SIGMA_DEG)
    } else {
        MAX_SIGMA_DEG
    }
}

#[must_use]
pub fn confidence(sigma_deg: f64, ambiguity: f32) -> f32 {
    if !(sigma_deg.is_finite() && sigma_deg > 0.0) {
        return 0.0;
    }
    let inside = erf(CONFIDENCE_WINDOW_DEG / (sigma_deg * SQRT_2));
    let unique = f64::from(1.0 - ambiguity.clamp(0.0, 1.0));
    (inside * unique).clamp(0.0, 1.0) as f32
}

pub fn joint_fit(
    steering: &[Complex<f32>],
    count: usize,
    elements: usize,
    r: &CMat,
    chol: &mut Cholesky,
    scratch: &mut [Complex<f32>],
) -> Result<f32, LinalgError> {
    let trace = r.trace_re();
    if count == 0 || trace.is_nan() || trace <= 0.0 {
        return Ok(0.0);
    }
    let size = count * elements;
    if count > MAX_ORDER || r.order() != elements || steering.len() < size || scratch.len() < size {
        return Err(LinalgError::Order(count));
    }
    let columns = &steering[..size];
    let products = &mut scratch[..size];
    for (a, out) in columns
        .chunks_exact(elements)
        .zip(products.chunks_exact_mut(elements))
    {
        multiply(r, a, out);
    }
    let mut gram = CMat::zeros(count)?;
    for (i, left) in columns.chunks_exact(elements).enumerate() {
        for (j, right) in columns.chunks_exact(elements).enumerate() {
            gram.set(i, j, inner(left, right));
        }
    }
    chol.factor_loaded(&gram, GRAM_LOADING)?;
    let mut explained = 0.0f32;
    let mut column = [Complex::new(0.0f32, 0.0); MAX_ORDER];
    for (b, product) in products.chunks_exact(elements).enumerate() {
        for (value, left) in column.iter_mut().zip(columns.chunks_exact(elements)) {
            *value = inner(left, product);
        }
        chol.solve(&mut column[..count]);
        explained += column[b].re;
    }
    Ok((explained / trace).clamp(0.0, 1.0))
}

fn multiply(r: &CMat, a: &[Complex<f32>], out: &mut [Complex<f32>]) {
    let n = r.order();
    for (row, value) in out.iter_mut().enumerate().take(n) {
        *value = (0..n).map(|col| r.get(row, col) * a[col]).sum();
    }
}

pub fn temper_likelihood(raw: &mut [f32; LIKELIHOOD_POINTS], curvature: f64, sigma_rad: f64) {
    let sharp = curvature * sigma_rad * sigma_rad;
    let tau = if sharp.is_finite() && sharp > 0.0 {
        sharp.recip().min(1.0)
    } else {
        1.0
    };
    let floor = LIKELIHOOD_FLOOR.ln();
    for value in raw.iter_mut() {
        let tempered = (f64::from(*value) * tau) as f32;
        *value = if tempered.is_nan() {
            floor
        } else {
            tempered.max(floor)
        };
    }
}

pub fn quantize_likelihood(ln: &[f32], out: &mut [u8]) {
    let floor = LIKELIHOOD_FLOOR.ln();
    for (byte, &value) in out.iter_mut().zip(ln) {
        let level = (255.0 * (value - floor) / -floor).round();
        *byte = if level.is_nan() {
            0
        } else {
            level.clamp(0.0, 255.0) as u8
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{Direction, Geometry, Winding, phase_rates, steer};

    const FREQ: f64 = 433.92e6;

    fn kraken() -> Geometry {
        Geometry::uca(0.2939, 5, 0.0, Winding::Clockwise).unwrap()
    }

    fn rates_at(geometry: &Geometry, azimuth_deg: f64) -> Vec<f64> {
        let mut rates = vec![0.0; geometry.len()];
        phase_rates(
            geometry.positions(),
            FREQ,
            Direction::horizon(azimuth_deg),
            &mut rates,
        );
        rates
    }

    #[test]
    fn crb_shrinks_with_snr_and_snapshots() {
        let rates = rates_at(&kraken(), 37.0);
        let low = crb_sigma_rad(&rates, 1.0, 5, 1000.0);
        let high = crb_sigma_rad(&rates, 100.0, 5, 1000.0);
        let longer = crb_sigma_rad(&rates, 1.0, 5, 4000.0);
        assert!(high < low / 5.0);
        assert!((longer - low / 2.0).abs() < 1e-12);
        let spread = rate_spread(&rates);
        let want = ((1.0 + 0.2) / (2000.0 * spread)).sqrt();
        assert!((low - want).abs() < 1e-12);
        assert!(crb_sigma_rad(&rates, f64::NAN, 5, 1000.0).is_finite());
    }

    #[test]
    fn calibration_error_scales_with_the_rate_spread() {
        let rates = rates_at(&kraken(), 37.0);
        let model = model_sigma_rad(&rates, 5.0);
        assert!((model - 5f64.to_radians() / rate_spread(&rates).sqrt()).abs() < 1e-12);
        let total = total_sigma_deg(0.0, model);
        assert!(total >= model.to_degrees());
        assert!((total_sigma_deg(0.0, 0.0) - SIGMA_FLOOR_DEG).abs() < 1e-12);
        assert_eq!(total_sigma_deg(f64::INFINITY, 0.0), MAX_SIGMA_DEG);
        assert_eq!(total_sigma_deg(f64::NAN, 0.0), MAX_SIGMA_DEG);
    }

    #[test]
    fn confidence_follows_sigma_and_ambiguity() {
        assert!(confidence(2.0, 0.0) > 0.98);
        assert!(confidence(60.0, 0.0) < 0.1);
        assert!((confidence(2.0, 0.5) - confidence(2.0, 0.0) / 2.0).abs() < 1e-6);
        assert_eq!(confidence(f64::NAN, 0.0), 0.0);
    }

    #[test]
    fn joint_fit_matches_the_single_wave_formula() {
        let geometry = kraken();
        let mut a = [Complex::new(0.0f32, 0.0); 5];
        steer(geometry.positions(), FREQ, Direction::horizon(37.0), &mut a);
        let snr = 10.0f32;
        let mut r = CMat::identity(5).unwrap();
        for i in 0..5 {
            for j in 0..5 {
                r.add(i, j, a[i] * a[j].conj() * snr);
            }
        }
        let mut chol = Cholesky::new(16).unwrap();
        let mut scratch = [Complex::new(0.0f32, 0.0); 80];
        let fit = joint_fit(&a, 1, 5, &r, &mut chol, &mut scratch).unwrap();
        let want = (5.0 * snr + 1.0) / (5.0 * (snr + 1.0));
        assert!((fit - want).abs() < 1e-4, "{fit} {want}");
        let mut b = [Complex::new(0.0f32, 0.0); 10];
        b[..5].copy_from_slice(&a);
        steer(
            geometry.positions(),
            FREQ,
            Direction::horizon(200.0),
            &mut b[5..],
        );
        let two = joint_fit(&b, 2, 5, &r, &mut chol, &mut scratch).unwrap();
        assert!(two > fit && two < 1.0, "{two}");
        let noise = CMat::identity(5).unwrap();
        let alone = joint_fit(&a, 1, 5, &noise, &mut chol, &mut scratch).unwrap();
        assert!((alone - 0.2).abs() < 1e-5);
        assert_eq!(joint_fit(&a, 0, 5, &r, &mut chol, &mut scratch), Ok(0.0));
        assert_eq!(
            joint_fit(&a, 2, 5, &r, &mut chol, &mut scratch),
            Err(LinalgError::Order(2))
        );
    }

    #[test]
    fn tempering_reaches_the_target_curvature_and_keeps_the_floor() {
        let sigma = 3f64.to_radians();
        let curvature = 1e4;
        let mut raw = [0.0f32; LIKELIHOOD_POINTS];
        for (i, value) in raw.iter_mut().enumerate() {
            let offset = (i as f64 - 180.0).to_radians();
            *value = (-0.5 * curvature * offset * offset) as f32;
        }
        temper_likelihood(&mut raw, curvature, sigma);
        let second = f64::from(raw[181] - 2.0 * raw[180] + raw[179]) / 1f64.to_radians().powi(2);
        assert!((-1.0 / second - sigma * sigma).abs() < 0.01 * sigma * sigma);
        assert!(raw.iter().all(|&v| v >= LIKELIHOOD_FLOOR.ln()));
        let mut flat = [-0.01f32; LIKELIHOOD_POINTS];
        temper_likelihood(&mut flat, 0.1, sigma);
        assert!(flat.iter().all(|&v| (v + 0.01).abs() < 1e-7));
    }

    #[test]
    fn quantized_likelihood_spans_the_byte() {
        let floor = LIKELIHOOD_FLOOR.ln();
        let mut out = [7u8; 5];
        quantize_likelihood(&[0.0, floor, floor / 2.0, 3.0, f32::NAN], &mut out);
        assert_eq!(out, [255, 0, 128, 255, 0]);
    }
}
