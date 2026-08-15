use num_complex::Complex;

use super::Constellation;
use crate::soft::Llr;

const MAX_BITS: usize = 32;

pub fn max_log_llrs(y: Complex<f32>, c: &Constellation, noise_var: f64, out: &mut [Llr]) {
    let bits = c.bits_per_symbol();
    assert_eq!(bits, out.len(), "one LLR slot per label bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut min0 = [f64::INFINITY; MAX_BITS];
    let mut min1 = [f64::INFINITY; MAX_BITS];
    for (p, &label) in c.points().iter().zip(c.labels()) {
        let d2 = dist2(y, *p);
        for k in 0..bits {
            if (label >> k) & 1 == 1 {
                min1[k] = min1[k].min(d2);
            } else {
                min0[k] = min0[k].min(d2);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr(((min0[k] - min1[k]) / noise_var) as f32);
    }
}

pub fn exact_llrs(y: Complex<f32>, c: &Constellation, noise_var: f64, out: &mut [Llr]) {
    let bits = c.bits_per_symbol();
    assert_eq!(bits, out.len(), "one LLR slot per label bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut sum0 = [LogSum::EMPTY; MAX_BITS];
    let mut sum1 = [LogSum::EMPTY; MAX_BITS];
    for (p, &label) in c.points().iter().zip(c.labels()) {
        let exponent = -dist2(y, *p) / noise_var;
        for k in 0..bits {
            if (label >> k) & 1 == 1 {
                sum1[k].add(exponent);
            } else {
                sum0[k].add(exponent);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr((sum1[k].value() - sum0[k].value()) as f32);
    }
}

pub fn energy_llrs(energies: &[f32], noise_var: f64, out: &mut [Llr]) {
    let m = energies.len();
    assert!(
        m.is_power_of_two() && m >= 2 && m.trailing_zeros() as usize <= MAX_BITS,
        "{m} tones is not a 2^k filterbank"
    );
    let bits = m.trailing_zeros() as usize;
    assert_eq!(bits, out.len(), "one LLR slot per tone-index bit");
    assert!(
        noise_var.is_finite() && noise_var > 0.0,
        "noise_var is a measured variance; {noise_var} is not one"
    );
    let mut max0 = [f64::NEG_INFINITY; MAX_BITS];
    let mut max1 = [f64::NEG_INFINITY; MAX_BITS];
    for (tone, &e) in energies.iter().enumerate() {
        let e = f64::from(e);
        for k in 0..bits {
            if (tone >> k) & 1 == 1 {
                max1[k] = max1[k].max(e);
            } else {
                max0[k] = max0[k].max(e);
            }
        }
    }
    for (k, slot) in out.iter_mut().enumerate() {
        *slot = Llr(((max1[k] - max0[k]) / noise_var) as f32);
    }
}

#[must_use]
pub fn noise_var_from_known(received: &[Complex<f32>], expected: &[Complex<f32>]) -> f64 {
    assert_eq!(
        received.len(),
        expected.len(),
        "received and expected must pair one-to-one"
    );
    assert!(!received.is_empty(), "no known symbols, no estimate");
    received
        .iter()
        .zip(expected)
        .map(|(&r, &e)| dist2(r, e))
        .sum::<f64>()
        / received.len() as f64
}

fn dist2(a: Complex<f32>, b: Complex<f32>) -> f64 {
    let dr = f64::from(a.re) - f64::from(b.re);
    let di = f64::from(a.im) - f64::from(b.im);
    dr * dr + di * di
}

#[derive(Clone, Copy)]
struct LogSum {
    max: f64,
    sum: f64,
}

impl LogSum {
    const EMPTY: Self = Self {
        max: f64::NEG_INFINITY,
        sum: 0.0,
    };

    fn add(&mut self, x: f64) {
        if x > self.max {
            self.sum = self.sum * (self.max - x).exp() + 1.0;
            self.max = x;
        } else {
            self.sum += (x - self.max).exp();
        }
    }

    fn value(self) -> f64 {
        self.max + self.sum.ln()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::{
        impair::{Awgn, Impairment},
        perf::assert_no_alloc,
        rng::Rng,
    };

    fn bpsk() -> Constellation {
        Constellation::from_points(
            vec![Complex::new(-1.0, 0.0), Complex::new(1.0, 0.0)],
            vec![0, 1],
        )
        .unwrap()
    }

    fn gray_4pam() -> Constellation {
        Constellation::from_points(
            vec![
                Complex::new(-3.0, 0.0),
                Complex::new(-1.0, 0.0),
                Complex::new(1.0, 0.0),
                Complex::new(3.0, 0.0),
            ],
            vec![0b00, 0b01, 0b11, 0b10],
        )
        .unwrap()
    }

    #[test]
    fn bpsk_matches_the_hand_computed_closed_form() {
        let c = bpsk();
        let sigma: f64 = 0.5;
        let noise_var = 2.0 * sigma * sigma;
        let y = Complex::new(0.3f32, -0.2);
        let mut out = [Llr(0.0); 1];
        max_log_llrs(y, &c, noise_var, &mut out);
        assert!((f64::from(out[0].0) - 2.4).abs() < 1e-5, "llr {}", out[0].0);
        let closed = 2.0 * f64::from(y.re) / (sigma * sigma);
        assert!((f64::from(out[0].0) - closed).abs() < 1e-6);
    }

    #[test]
    fn two_point_table_makes_maxlog_exact() {
        let c = bpsk();
        for y in [
            Complex::new(0.3f32, -0.2),
            Complex::new(-1.7, 0.4),
            Complex::new(0.01, 0.0),
            Complex::new(2.5, -2.5),
        ] {
            let mut ml = [Llr(0.0); 1];
            let mut ex = [Llr(0.0); 1];
            max_log_llrs(y, &c, 0.5, &mut ml);
            exact_llrs(y, &c, 0.5, &mut ex);
            assert!(
                (ml[0].0 - ex[0].0).abs() < 1e-6,
                "max-log {} vs exact {} at {y}",
                ml[0].0,
                ex[0].0
            );
        }
    }

    #[test]
    fn gray_4pam_maxlog_matches_hand_computation() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        max_log_llrs(Complex::new(0.6, 0.0), &c, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.053_374_8).abs() < 1e-5,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) - 2.146_625_3).abs() < 1e-5,
            "bit 1: {}",
            out[1].0
        );
    }

    #[test]
    fn gray_4pam_exact_tier_matches_hand_computation() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        exact_llrs(Complex::new(0.6, 0.0), &c, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.162_316_7).abs() < 1e-5,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) - 2.441_057_2).abs() < 1e-5,
            "bit 1: {}",
            out[1].0
        );
    }

    #[test]
    fn exact_converges_to_maxlog_at_high_snr_and_departs_boundedly_at_low() {
        let c = gray_4pam();
        let y = Complex::new(0.6, 0.0);
        let diff = |noise_var: f64| {
            let mut ml = [Llr(0.0); 2];
            let mut ex = [Llr(0.0); 2];
            max_log_llrs(y, &c, noise_var, &mut ml);
            exact_llrs(y, &c, noise_var, &mut ex);
            [
                f64::from(ex[0].0) - f64::from(ml[0].0),
                f64::from(ex[1].0) - f64::from(ml[1].0),
            ]
        };
        for d in diff(0.01) {
            assert!(d.abs() < 1e-6, "high SNR: {d}");
        }
        let low = diff(4.0);
        let bound = std::f64::consts::LN_2 + 1e-9;
        assert!(low.iter().any(|d| d.abs() > 0.01), "low SNR: {low:?}");
        for d in low {
            assert!(d.abs() <= bound, "low SNR diff {d} past ln 2");
        }
    }

    #[test]
    fn fsk4_energy_llrs_match_hand_computation() {
        let energies = [0.1f32, 1.2, 0.3, 0.2];
        let mut out = [Llr(0.0); 2];
        energy_llrs(&energies, 0.5, &mut out);
        assert!(
            (f64::from(out[0].0) - 1.8).abs() < 1e-6,
            "bit 0: {}",
            out[0].0
        );
        assert!(
            (f64::from(out[1].0) + 1.8).abs() < 1e-6,
            "bit 1: {}",
            out[1].0
        );
        assert!(out[0].bit());
        assert!(!out[1].bit());
    }

    #[test]
    fn noise_var_estimator_reads_injected_awgn_within_two_percent() {
        let x = std::f32::consts::FRAC_1_SQRT_2;
        let qpsk = [
            Complex::new(x, x),
            Complex::new(-x, x),
            Complex::new(-x, -x),
            Complex::new(x, -x),
        ];
        let expected: Vec<Complex<f32>> = (0..100_000).map(|i| qpsk[i % 4]).collect();
        let mut received = expected.clone();
        let sigma = 0.3;
        Awgn::with_sigma(sigma).apply(&mut received, &mut Rng::new(0x0f5e));
        let estimate = noise_var_from_known(&received, &expected);
        let truth = 2.0 * sigma * sigma;
        assert!(
            (estimate / truth - 1.0).abs() < 0.02,
            "estimated {estimate}, injected {truth}"
        );
    }

    #[test]
    fn clean_symbols_estimate_zero_variance() {
        let expected = vec![Complex::new(1.0f32, 0.0); 16];
        assert_eq!(noise_var_from_known(&expected, &expected), 0.0);
    }

    #[test]
    fn demap_paths_allocate_nothing() {
        let c = gray_4pam();
        let y = Complex::new(0.6f32, -0.1);
        let energies = [0.1f32, 1.2, 0.3, 0.2];
        let symbols = [Complex::new(1.0f32, 0.0); 64];
        let mut out = [Llr(0.0); 2];
        assert_no_alloc("max_log_llrs", || max_log_llrs(y, &c, 0.5, &mut out));
        assert_no_alloc("exact_llrs", || exact_llrs(y, &c, 0.5, &mut out));
        assert_no_alloc("energy_llrs", || energy_llrs(&energies, 0.5, &mut out));
        assert_no_alloc("hard_slice", || {
            std::hint::black_box(c.hard_slice(y));
        });
        assert_no_alloc("noise_var_from_known", || {
            std::hint::black_box(noise_var_from_known(&symbols, &symbols));
        });
    }

    #[test]
    fn maxlog_signs_agree_with_hard_slice() {
        let c = gray_4pam();
        let mut out = [Llr(0.0); 2];
        for i in -20..=20 {
            let y = Complex::new(i as f32 * 0.1, 0.0);
            max_log_llrs(y, &c, 0.3, &mut out);
            let label = c.hard_slice(y);
            for (k, llr) in out.iter().enumerate() {
                if !llr.is_erasure() {
                    assert_eq!(
                        llr.bit(),
                        (label >> k) & 1 == 1,
                        "bit {k} at y = {y}: llr {}",
                        llr.0
                    );
                }
            }
        }
    }
}
