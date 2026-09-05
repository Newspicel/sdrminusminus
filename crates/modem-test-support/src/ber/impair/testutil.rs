use std::f64::consts::TAU;

use num_complex::Complex;

use super::sinc::interp;
use crate::ber::rng::Rng;

pub(crate) fn ones(len: usize) -> Vec<Complex<f32>> {
    vec![Complex::new(1.0, 0.0); len]
}

pub(crate) fn tone(f: f64, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|n| {
            let cycles = f * n as f64;
            let phase = TAU * (cycles - cycles.floor());
            let (sin, cos) = phase.sin_cos();
            Complex::new(cos as f32, sin as f32)
        })
        .collect()
}

pub(crate) fn white(rng: &mut Rng, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|_| {
            let (i, q) = rng.normal_pair();
            Complex::new(
                (i * std::f64::consts::FRAC_1_SQRT_2) as f32,
                (q * std::f64::consts::FRAC_1_SQRT_2) as f32,
            )
        })
        .collect()
}

pub(crate) fn arg_increments(x: &[Complex<f32>]) -> Vec<f64> {
    x.windows(2)
        .map(|w| f64::from((w[1] * w[0].conj()).arg()))
        .collect()
}

pub(crate) fn est_delay(
    x: &[Complex<f32>],
    y: &[Complex<f32>],
    range: std::ops::Range<usize>,
    max_lag: usize,
) -> f64 {
    let metric_int = |lag: usize| -> f64 {
        let mut acc = Complex::new(0.0f64, 0.0);
        for n in range.clone() {
            let a = Complex::new(f64::from(y[n].re), f64::from(y[n].im));
            let b = &x[n - lag];
            acc += a * Complex::new(f64::from(b.re), f64::from(b.im)).conj();
        }
        acc.norm_sqr()
    };
    let mut best = 0usize;
    let mut best_metric = f64::NEG_INFINITY;
    for lag in 0..=max_lag {
        let m = metric_int(lag);
        if m > best_metric {
            best_metric = m;
            best = lag;
        }
    }

    let metric_frac = |tau: f64| -> f64 {
        let mut acc = Complex::new(0.0f64, 0.0);
        for n in range.clone() {
            let a = Complex::new(f64::from(y[n].re), f64::from(y[n].im));
            let b = interp(x, n as f64 - tau);
            acc += a * Complex::new(f64::from(b.re), f64::from(b.im)).conj();
        }
        acc.norm_sqr()
    };
    let mut center = best as f64;
    for (half_span, step) in [
        (1.0f64, 0.2f64),
        (0.2, 0.02),
        (0.02, 0.002),
        (0.002, 0.0005),
    ] {
        let mut best_tau = center;
        let mut best_m = f64::NEG_INFINITY;
        let steps = (2.0 * half_span / step).round() as usize;
        for k in 0..=steps {
            let tau = center - half_span + k as f64 * step;
            let m = metric_frac(tau);
            if m > best_m {
                best_m = m;
                best_tau = tau;
            }
        }
        center = best_tau;
    }
    center
}
