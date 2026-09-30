const P: f64 = 0.327_591_1;
const A: [f64; 5] = [
    0.254_829_592,
    -0.284_496_736,
    1.421_413_741,
    -1.453_152_027,
    1.061_405_429,
];

#[must_use]
pub fn erf(x: f64) -> f64 {
    let magnitude = x.abs();
    let t = 1.0 / (1.0 + P * magnitude);
    let series = A.iter().rev().fold(0.0, |sum, &a| (sum + a) * t);
    let value = 1.0 - series * (-magnitude * magnitude).exp();
    value.copysign(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_matches_reference_values() {
        for (x, want) in [
            (0.5, 0.520_499_877_8),
            (1.0, 0.842_700_792_9),
            (2.0, 0.995_322_265_0),
        ] {
            assert!((erf(x) - want).abs() < 2e-7, "erf({x}) = {}", erf(x));
            assert!((erf(-x) + want).abs() < 2e-7);
        }
    }

    #[test]
    fn erf_is_bounded_and_odd_at_the_ends() {
        assert!(erf(0.0).abs() < 2e-7);
        assert_eq!(erf(f64::INFINITY), 1.0);
        assert_eq!(erf(f64::NEG_INFINITY), -1.0);
        assert!(erf(f64::NAN).is_nan());
        assert!((erf(6.0) - 1.0).abs() < 2e-7);
    }
}
