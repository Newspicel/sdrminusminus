use std::f64::consts::PI;

use sdrmm_dsp::design_gaussian;

use super::{Norm, normalise, renorm_designed};

fn midpoint_taps(sps: f64, l: usize, shape: impl Fn(f64) -> f64) -> Vec<f64> {
    assert!(sps > 1.0, "need more than one sample per symbol");
    assert!(l >= 1, "pulse must cover at least one symbol");
    let n = (l as f64 * sps).round() as usize;
    assert!(
        n >= 2,
        "pulse needs at least two samples across its support"
    );
    (0..n).map(|k| shape((k as f64 + 0.5) / n as f64)).collect()
}

#[must_use]
pub fn rect(sps: f64, norm: Norm) -> Vec<f32> {
    lrec(sps, 1, norm)
}

#[must_use]
pub fn lrec(sps: f64, l: usize, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, l, |_| 1.0), norm)
}

#[must_use]
pub fn lrc(sps: f64, l: usize, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, l, |u| 1.0 - (2.0 * PI * u).cos()), norm)
}

#[must_use]
pub fn half_sine(sps: f64, norm: Norm) -> Vec<f32> {
    normalise(midpoint_taps(sps, 1, |u| (PI * u).sin()), norm)
}

#[must_use]
pub fn gaussian(sps: f64, bt: f64, span: usize, norm: Norm) -> Vec<f32> {
    renorm_designed(design_gaussian(sps, bt, span), norm)
}

#[must_use]
pub fn gaussian_freq(sps: f64, bt: f64, span: usize, norm: Norm) -> Vec<f32> {
    assert!(
        span >= 2,
        "total span must exceed the rect's own symbol: {span}"
    );
    let smoothing = design_gaussian(sps, bt, span - 1);
    let n_rect = sps.round() as usize;
    assert!(n_rect >= 2, "need at least two samples per symbol");
    let mut g = vec![0.0f64; smoothing.len() + n_rect - 1];
    for (i, &s) in smoothing.iter().enumerate() {
        for slot in &mut g[i..i + n_rect] {
            *slot += f64::from(s);
        }
    }
    normalise(g, norm)
}

#[must_use]
pub fn phase_pulse(freq: &[f32]) -> Vec<f32> {
    let mut acc = 0.0f64;
    freq.iter()
        .map(|&g| {
            acc += f64::from(g);
            (0.5 * acc) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrec_of_one_symbol_is_rect() {
        for sps in [4.0, 8.0, 12.5] {
            for norm in [Norm::Energy, Norm::Area] {
                assert_eq!(lrec(sps, 1, norm), rect(sps, norm), "sps={sps} {norm:?}");
            }
        }
    }

    #[test]
    fn half_sine_energy_and_area_match_the_closed_forms() {
        for n in [8usize, 5] {
            let energy = half_sine(n as f64, Norm::Energy);
            let area = half_sine(n as f64, Norm::Area);
            assert_eq!(energy.len(), n);
            let energy_scale = (n as f64 / 2.0).sqrt().recip();
            let area_scale = (PI / (2.0 * n as f64)).sin();
            for k in 0..n {
                let raw = (PI * (k as f64 + 0.5) / n as f64).sin();
                let e = f64::from(energy[k]);
                let a = f64::from(area[k]);
                assert!((e - raw * energy_scale).abs() < 1e-6, "n={n} tap {k}: {e}");
                assert!((a - raw * area_scale).abs() < 1e-6, "n={n} tap {k}: {a}");
            }
        }
    }

    #[test]
    fn gaussian_under_area_norm_is_bit_identical_to_design_gaussian() {
        for (sps, bt, span) in [(8.0, 0.5, 3), (10.0, 0.4, 4), (5.0, 0.3, 3)] {
            assert_eq!(
                gaussian(sps, bt, span, Norm::Area),
                design_gaussian(sps, bt, span),
                "sps={sps} bt={bt} span={span}"
            );
        }
    }

    #[test]
    fn gaussian_freq_at_bt_half_is_narrower_in_time_than_bt_point_three() {
        let sps = 10usize;
        let sharp = gaussian_freq(sps as f64, 0.5, 4, Norm::Area);
        let smooth = gaussian_freq(sps as f64, 0.3, 4, Norm::Area);
        assert_eq!(sharp.len(), smooth.len());
        assert_eq!(sharp.len(), 4 * sps);
        let peak = |taps: &[f32]| taps.iter().fold(0.0f32, |m, &x| m.max(x));
        assert!(peak(&sharp) > peak(&smooth), "peak ordering");
        let central = |taps: &[f32]| {
            let lo = taps.len() / 2 - sps / 2;
            taps[lo..lo + sps]
                .iter()
                .map(|&x| f64::from(x))
                .sum::<f64>()
        };
        assert!(central(&sharp) > central(&smooth), "concentration ordering");
    }

    #[test]
    fn area_normalised_frequency_pulses_reach_q_of_one_half() {
        let pulses: Vec<(&str, Vec<f32>)> = vec![
            ("rect", rect(8.0, Norm::Area)),
            ("half_sine", half_sine(8.0, Norm::Area)),
            ("lrec(2)", lrec(8.0, 2, Norm::Area)),
            ("lrec(3)", lrec(8.0, 3, Norm::Area)),
            ("lrc(2)", lrc(8.0, 2, Norm::Area)),
            ("lrc(3)", lrc(8.0, 3, Norm::Area)),
            (
                "gaussian_freq BT=0.3",
                gaussian_freq(8.0, 0.3, 4, Norm::Area),
            ),
            (
                "gaussian_freq BT=0.5",
                gaussian_freq(8.0, 0.5, 3, Norm::Area),
            ),
        ];
        for (name, g) in pulses {
            let q = phase_pulse(&g);
            let last = f64::from(*q.last().unwrap());
            assert!((last - 0.5).abs() < 1e-5, "{name}: q(∞) = {last}");
            for (k, w) in q.windows(2).enumerate() {
                assert!(w[1] >= w[0], "{name}: q not monotone at {k}");
            }
            assert!(q[0] >= 0.0, "{name}: q starts negative");
        }
    }
}
