use std::f32::consts::{FRAC_PI_2, PI};

use num_complex::Complex;

const SQRT_HALF_BITS: u32 = 0x3F35_04F3;
const MANTISSA_BITS: u32 = 23;
const SUBNORMAL_SCALE: f32 = 16_777_216.0;
const SUBNORMAL_OCTAVES: f32 = 24.0;
const LOG2_SERIES: [f32; 6] = [
    1.442_713_3,
    -0.721_132_04,
    0.479_351_46,
    -0.367_487_73,
    0.322_139_23,
    -0.206_597_67,
];
const DB_PER_OCTAVE: f32 = 10.0 * std::f32::consts::LOG10_2;

#[must_use]
#[inline(always)]
pub fn fast_log2(x: f32) -> f32 {
    let subnormal = x < f32::MIN_POSITIVE;
    let scaled = if subnormal { x * SUBNORMAL_SCALE } else { x };
    let bits = scaled.to_bits();
    let exponent = (bits.wrapping_sub(SQRT_HALF_BITS) as i32) >> MANTISSA_BITS;
    let mantissa = f32::from_bits(bits.wrapping_sub((exponent as u32) << MANTISSA_BITS));
    let t = mantissa - 1.0;
    let [c0, c1, c2, c3, c4, c5] = LOG2_SERIES;
    let series = c0 + t * (c1 + t * (c2 + t * (c3 + t * (c4 + t * c5))));
    let octaves = exponent as f32 - if subnormal { SUBNORMAL_OCTAVES } else { 0.0 };
    let log = octaves + t * series;
    let log = if x == f32::INFINITY { x } else { log };
    if x > 0.0 {
        log
    } else if x == 0.0 {
        f32::NEG_INFINITY
    } else {
        f32::NAN
    }
}

#[must_use]
#[inline(always)]
pub fn fast_log10(x: f32) -> f32 {
    fast_log2(x) * std::f32::consts::LOG10_2
}

#[must_use]
#[inline(always)]
pub fn fast_power_db(power: f32) -> f32 {
    fast_log2(power) * DB_PER_OCTAVE
}

const ATAN_UNIT: [f32; 7] = [
    0.999_996_1,
    -0.333_173_7,
    0.198_078_14,
    -0.132_333_38,
    0.079_623_6,
    -0.033_604_16,
    0.006_811_773,
];

#[inline]
fn atan_unit(ratio: f32) -> f32 {
    let square = ratio * ratio;
    let mut poly = ATAN_UNIT[6];
    poly = poly * square + ATAN_UNIT[5];
    poly = poly * square + ATAN_UNIT[4];
    poly = poly * square + ATAN_UNIT[3];
    poly = poly * square + ATAN_UNIT[2];
    poly = poly * square + ATAN_UNIT[1];
    poly = poly * square + ATAN_UNIT[0];
    poly * ratio
}

#[inline]
#[must_use]
pub fn fast_atan2(y: f32, x: f32) -> f32 {
    let (ax, ay) = (x.abs(), y.abs());
    let steep = ay > ax;
    let (lo, hi) = if steep { (ax, ay) } else { (ay, ax) };
    let ratio = if ax == ay { 1.0 } else { lo / hi };
    let mut angle = atan_unit(ratio);
    angle = if steep { FRAC_PI_2 - angle } else { angle };
    angle = if x < 0.0 { PI - angle } else { angle };
    angle = if ax + ay == 0.0 { 0.0 } else { angle };
    angle.copysign(y)
}

#[inline]
#[must_use]
pub fn fast_arg(z: Complex<f32>) -> f32 {
    fast_atan2(z.im, z.re)
}

pub fn phase_diff_into(
    iq: &[Complex<f32>],
    prev: &mut Complex<f32>,
    scale: f32,
    out: &mut Vec<f32>,
) {
    out.clear();
    let (Some(&first), Some(&last)) = (iq.first(), iq.last()) else {
        return;
    };
    out.push(fast_arg(first * prev.conj()) * scale);
    out.extend(
        iq.iter()
            .skip(1)
            .zip(iq)
            .map(|(&now, &before)| fast_arg(now * before.conj()) * scale),
    );
    *prev = last;
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOLERANCE: f64 = 1e-6;

    fn angle_error(fast: f32, exact: f64) -> f64 {
        let diff = (f64::from(fast) - exact).rem_euclid(std::f64::consts::TAU);
        diff.min(std::f64::consts::TAU - diff)
    }

    #[test]
    fn matches_atan2_over_a_dense_grid_of_angles_and_magnitudes() {
        let magnitudes = [
            1e-44, 1e-40, 1e-38, 1e-30, 1e-10, 1e-3, 0.5, 1.0, 7.0, 1e5, 1e20, 1e35, 3e38,
        ];
        let mut worst = 0.0f64;
        for &magnitude in &magnitudes {
            for k in 0..20_000 {
                let theta = -std::f64::consts::PI + std::f64::consts::TAU * f64::from(k) / 20_000.0;
                let (y, x) = (
                    (magnitude * theta.sin()) as f32,
                    (magnitude * theta.cos()) as f32,
                );
                if x == 0.0 && y == 0.0 {
                    continue;
                }
                let exact = f64::from(y).atan2(f64::from(x));
                worst = worst.max(angle_error(fast_atan2(y, x), exact));
            }
        }
        assert!(worst < TOLERANCE, "worst error {worst}");
    }

    #[test]
    fn matches_atan2_over_a_ratio_sweep_in_every_quadrant() {
        let mut worst = 0.0f64;
        for k in 0..=100_000 {
            let ratio = k as f32 / 100_000.0;
            for (y, x) in [
                (ratio, 1.0),
                (1.0, ratio),
                (-ratio, 1.0),
                (ratio, -1.0),
                (-1.0, -ratio),
                (1.0, -ratio),
            ] {
                let exact = f64::from(y).atan2(f64::from(x));
                worst = worst.max(angle_error(fast_atan2(y, x), exact));
            }
        }
        assert!(worst < TOLERANCE, "worst error {worst}");
    }

    #[test]
    fn zero_over_zero_is_zero() {
        for (y, x) in [(0.0f32, 0.0f32), (-0.0, 0.0), (0.0, -0.0), (-0.0, -0.0)] {
            let angle = fast_atan2(y, x);
            assert_eq!(angle, 0.0, "atan2({y}, {x})");
            assert_eq!(angle.is_sign_negative(), y.is_sign_negative());
        }
        assert_eq!(fast_arg(Complex::new(0.0, 0.0)), 0.0);
    }

    #[test]
    fn signed_zeros_and_axes_follow_atan2() {
        let cases = [
            (0.0f32, 1.0f32),
            (-0.0, 1.0),
            (0.0, -1.0),
            (-0.0, -1.0),
            (1.0, 0.0),
            (-1.0, 0.0),
            (1.0, -0.0),
            (-1.0, -0.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
            (-1.0, 1.0),
        ];
        for (y, x) in cases {
            let fast = fast_atan2(y, x);
            let exact = y.atan2(x);
            assert!((fast - exact).abs() < 1e-6, "atan2({y}, {x}) = {fast}");
            assert_eq!(fast.is_sign_negative(), exact.is_sign_negative());
        }
    }

    #[test]
    fn infinities_follow_atan2() {
        let inf = f32::INFINITY;
        for (y, x) in [
            (inf, inf),
            (inf, -inf),
            (-inf, inf),
            (-inf, -inf),
            (inf, 1.0),
            (-inf, -1.0),
            (1.0, inf),
            (1.0, -inf),
            (-1.0, -inf),
            (0.0, -inf),
        ] {
            let fast = fast_atan2(y, x);
            let exact = y.atan2(x);
            assert!((fast - exact).abs() < 1e-6, "atan2({y}, {x}) = {fast}");
        }
    }

    #[test]
    fn nan_propagates() {
        for (y, x) in [
            (f32::NAN, 1.0),
            (1.0, f32::NAN),
            (f32::NAN, f32::NAN),
            (f32::NAN, 0.0),
            (0.0, f32::NAN),
            (f32::NAN, f32::INFINITY),
        ] {
            assert!(fast_atan2(y, x).is_nan(), "atan2({y}, {x})");
        }
    }

    #[test]
    fn block_matches_scalar_per_sample() {
        let iq: Vec<Complex<f32>> = (0..1_031)
            .map(|k| {
                let k = k as f32;
                Complex::from_polar(0.1 + (k * 0.37).sin().abs(), k * 0.91 + (k * 0.05).cos())
            })
            .collect();
        let start = Complex::new(0.3, -0.8);
        let (scale, mut prev, mut out) = (1.7, start, Vec::new());
        phase_diff_into(&iq, &mut prev, scale, &mut out);
        assert_eq!(out.len(), iq.len());
        assert_eq!(prev, iq[iq.len() - 1]);
        let mut before = start;
        for (&x, &got) in iq.iter().zip(&out) {
            assert_eq!(
                got.to_bits(),
                (fast_arg(x * before.conj()) * scale).to_bits()
            );
            before = x;
        }
    }

    #[test]
    fn block_carries_state_across_calls_and_ignores_empty_input() {
        let iq: Vec<Complex<f32>> = (0..64)
            .map(|k| Complex::from_polar(1.0, 0.2 * k as f32))
            .collect();
        let (mut whole, mut split) = (Vec::new(), Vec::new());
        let mut prev = Complex::new(1.0, 0.0);
        phase_diff_into(&iq, &mut prev, 1.0, &mut whole);
        let mut prev = Complex::new(1.0, 0.0);
        let mut part = Vec::new();
        for chunk in iq.chunks(7) {
            phase_diff_into(&[], &mut prev, 1.0, &mut part);
            assert!(part.is_empty());
            phase_diff_into(chunk, &mut prev, 1.0, &mut part);
            split.extend_from_slice(&part);
        }
        assert_eq!(whole, split);
        assert!(whole[1..].iter().all(|&v| (v - 0.2).abs() < 1e-5));
    }

    fn every_positive_float(stride: u32) -> impl Iterator<Item = f32> {
        (1..f32::INFINITY.to_bits())
            .step_by(stride as usize)
            .chain([
                f32::MIN_POSITIVE.to_bits() - 1,
                f32::MIN_POSITIVE.to_bits(),
                f32::MAX.to_bits(),
            ])
            .map(f32::from_bits)
    }

    #[test]
    fn log10_stays_within_three_micro_decades_of_std_from_subnormals_to_max() {
        let worst = every_positive_float(997)
            .map(|x| (fast_log10(x) - x.log10()).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 3e-5, "worst {worst}");
    }

    #[test]
    fn log2_is_exact_at_powers_of_two_and_close_to_std_near_one() {
        for exponent in -149i32..128 {
            let x = if exponent < -126 {
                f32::from_bits(1 << (exponent + 149))
            } else {
                f32::from_bits(((exponent + 127) as u32) << 23)
            };
            assert!(
                (fast_log2(x) - exponent as f32).abs() < 1e-5,
                "2^{exponent}"
            );
        }
        let worst = (0..100_000)
            .map(|step| 0.5 + step as f32 / 50_000.0)
            .map(|x| (fast_log2(x) - x.log2()).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 5e-6, "worst {worst}");
    }

    #[test]
    fn power_db_is_invisible_next_to_std_on_a_display_scale() {
        let worst = every_positive_float(9_973)
            .map(|p| (fast_power_db(p) - 10.0 * p.log10()).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "worst {worst} dB");
    }

    #[test]
    fn edge_values_match_std() {
        assert_eq!(fast_log10(0.0), f32::NEG_INFINITY);
        assert_eq!(fast_log10(-0.0), f32::NEG_INFINITY);
        assert_eq!(fast_log10(f32::INFINITY), f32::INFINITY);
        assert!(fast_log10(-1.0).is_nan());
        assert!(fast_log10(f32::NEG_INFINITY).is_nan());
        assert!(fast_log10(f32::NAN).is_nan());
        assert_eq!(fast_log10(1.0), 0.0);
    }
}
