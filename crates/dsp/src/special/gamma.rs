use std::f64::consts::{PI, TAU};

const LANCZOS_G: f64 = 7.0;
const LANCZOS: [f64; 9] = [
    0.999_999_999_999_809_9,
    676.520_368_121_885_1,
    -1_259.139_216_722_402_8,
    771.323_428_777_653_1,
    -176.615_029_162_140_6,
    12.507_343_278_686_905,
    -0.138_571_095_265_720_12,
    9.984_369_578_019_572e-6,
    1.505_632_735_149_311_6e-7,
];
const MAX_TERMS: usize = 100_000;
const TINY: f64 = f64::MIN_POSITIVE / f64::EPSILON;

#[must_use]
pub fn ln_gamma(x: f64) -> f64 {
    if x.is_nan() || x == f64::NEG_INFINITY {
        return f64::NAN;
    }
    if x == f64::INFINITY {
        return f64::INFINITY;
    }
    if x <= 0.0 && x == x.floor() {
        return f64::INFINITY;
    }
    if x < 0.5 {
        return (PI / (PI * x).sin().abs()).ln() - ln_gamma(1.0 - x);
    }
    let shifted = x - 1.0;
    let sum = LANCZOS
        .iter()
        .enumerate()
        .skip(1)
        .fold(LANCZOS[0], |sum, (index, c)| {
            sum + c / (shifted + index as f64)
        });
    let t = shifted + LANCZOS_G + 0.5;
    0.5 * TAU.ln() + (shifted + 0.5) * t.ln() - t + sum.ln()
}

#[must_use]
pub fn gamma_p(a: f64, x: f64) -> f64 {
    match Regime::of(a, x) {
        Regime::Invalid => f64::NAN,
        Regime::Origin => 0.0,
        Regime::Infinite => 1.0,
        Regime::Series => series(a, x),
        Regime::Fraction => 1.0 - fraction(a, x),
    }
}

#[must_use]
pub fn gamma_q(a: f64, x: f64) -> f64 {
    match Regime::of(a, x) {
        Regime::Invalid => f64::NAN,
        Regime::Origin => 1.0,
        Regime::Infinite => 0.0,
        Regime::Series => 1.0 - series(a, x),
        Regime::Fraction => fraction(a, x),
    }
}

enum Regime {
    Invalid,
    Origin,
    Infinite,
    Series,
    Fraction,
}

impl Regime {
    fn of(a: f64, x: f64) -> Self {
        if !(a > 0.0 && a.is_finite() && x >= 0.0) {
            Self::Invalid
        } else if x == 0.0 {
            Self::Origin
        } else if x == f64::INFINITY {
            Self::Infinite
        } else if x < a + 1.0 {
            Self::Series
        } else {
            Self::Fraction
        }
    }
}

fn prefactor(a: f64, x: f64) -> f64 {
    (a * x.ln() - x - ln_gamma(a)).exp()
}

fn series(a: f64, x: f64) -> f64 {
    let mut denominator = a;
    let mut term = 1.0 / a;
    let mut sum = term;
    for _ in 0..MAX_TERMS {
        denominator += 1.0;
        term *= x / denominator;
        sum += term;
        if term.abs() < sum.abs() * f64::EPSILON {
            return sum * prefactor(a, x);
        }
    }
    f64::NAN
}

fn fraction(a: f64, x: f64) -> f64 {
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / TINY;
    let mut d = 1.0 / b;
    let mut h = d;
    for step in 1..=MAX_TERMS {
        let i = step as f64;
        let an = -i * (i - a);
        b += 2.0;
        d = away_from_zero(an * d + b).recip();
        c = away_from_zero(b + an / c);
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < f64::EPSILON {
            return prefactor(a, x) * h;
        }
    }
    f64::NAN
}

fn away_from_zero(value: f64) -> f64 {
    if value.abs() < TINY { TINY } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factorial(n: u32) -> f64 {
        (1..=n).map(f64::from).product()
    }

    #[test]
    fn ln_gamma_matches_known_values() {
        for (x, expected) in [
            (0.5, 0.572_364_942_924_700_1),
            (1.0, 0.0),
            (2.0, 0.0),
            (3.5, 1.200_973_602_347_074_3),
            (10.0, 12.801_827_480_081_469),
            (100.0, 359.134_205_369_575_4),
            (-0.5, 1.265_512_123_484_645_4),
        ] {
            let got = ln_gamma(x);
            assert!(
                (got - expected).abs() <= 1e-10 * expected.abs().max(1.0),
                "ln_gamma({x}) = {got}, expected {expected}"
            );
        }
        assert_eq!(ln_gamma(0.0), f64::INFINITY);
        assert_eq!(ln_gamma(-3.0), f64::INFINITY);
        assert!(ln_gamma(f64::NAN).is_nan());
    }

    #[test]
    fn ln_gamma_follows_the_factorials() {
        for n in 1..=30u32 {
            let expected = factorial(n - 1).ln();
            let got = ln_gamma(f64::from(n));
            assert!(
                (got - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                "n = {n}: {got} vs {expected}"
            );
        }
    }

    #[test]
    fn gamma_p_matches_integer_shape_closed_form() {
        for n in 1..=12u32 {
            for x in [1e-3_f64, 0.1, 0.5, 1.0, 2.0, 5.0, 9.5, 13.0, 20.0, 40.0] {
                let head: f64 = (0..n).map(|k| x.powi(k as i32) / factorial(k)).sum();
                let q = (-x).exp() * head;
                let p = 1.0 - q;
                let (got_p, got_q) = (gamma_p(f64::from(n), x), gamma_q(f64::from(n), x));
                assert!((got_p - p).abs() < 1e-12, "P({n}, {x}) = {got_p} vs {p}");
                assert!(
                    (got_q - q).abs() < 1e-12 * q.max(1e-3),
                    "Q({n}, {x}) = {got_q} vs {q}"
                );
            }
        }
    }

    #[test]
    fn gamma_p_and_q_sum_to_one() {
        for a in [0.3, 0.5, 1.7, 4.2, 33.3, 250.5, 4000.0] {
            for scale in [0.01, 0.5, 0.9, 1.0, 1.1, 2.0, 5.0] {
                let x = a * scale;
                let sum = gamma_p(a, x) + gamma_q(a, x);
                assert!((sum - 1.0).abs() < 1e-12, "a = {a}, x = {x}: {sum}");
            }
        }
    }

    #[test]
    fn gamma_p_follows_its_recurrence_for_large_shapes() {
        for a in [120.0_f64, 900.5, 5000.0] {
            for scale in [0.95, 1.0, 1.03] {
                let x = a * scale;
                let step = (a * x.ln() - x - ln_gamma(a + 1.0)).exp();
                let diff = gamma_p(a, x) - gamma_p(a + 1.0, x);
                assert!(
                    (diff - step).abs() < 1e-11,
                    "a = {a}, x = {x}: {diff} vs {step}"
                );
            }
        }
        let median_gap = gamma_p(1000.0, 1000.0) - 0.5;
        assert!(median_gap > 0.0 && median_gap < 0.01);
    }

    #[test]
    fn invalid_arguments_are_nan() {
        assert!(gamma_p(0.0, 1.0).is_nan());
        assert!(gamma_p(-1.0, 1.0).is_nan());
        assert!(gamma_q(2.0, -0.1).is_nan());
        assert!(gamma_p(f64::NAN, 1.0).is_nan());
        assert!(gamma_q(1.0, f64::NAN).is_nan());
        assert_eq!(gamma_p(2.0, 0.0), 0.0);
        assert_eq!(gamma_q(2.0, 0.0), 1.0);
        assert_eq!(gamma_p(2.0, f64::INFINITY), 1.0);
        assert_eq!(gamma_q(2.0, f64::INFINITY), 0.0);
    }
}
