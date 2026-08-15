use num_complex::Complex;

use super::params::SubcarrierMap;

pub const MIN_NOISE_VAR: f64 = 1e-12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelEstimator {
    LongTraining,
    ShortComb,
}

#[derive(Clone, Debug)]
pub struct ChannelEstimate {
    inv: Vec<Complex<f32>>,
    gain: Vec<f32>,
    h: Vec<Complex<f32>>,
    known: Vec<bool>,
    noise_var: f64,
}

impl ChannelEstimate {
    #[must_use]
    pub fn new(fft: usize) -> Self {
        Self {
            inv: vec![Complex::new(0.0, 0.0); fft],
            gain: vec![0.0; fft],
            h: vec![Complex::new(0.0, 0.0); fft],
            known: vec![false; fft],
            noise_var: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.h.fill(Complex::new(0.0, 0.0));
        self.inv.fill(Complex::new(0.0, 0.0));
        self.gain.fill(0.0);
        self.known.fill(false);
        self.noise_var = 0.0;
    }

    pub fn set(&mut self, bin: usize, h: Complex<f32>) {
        self.h[bin] = h;
        self.known[bin] = true;
    }

    pub fn finish(&mut self, map: &SubcarrierMap, noise_var: f64, ramp_cycles_per_bin: f64) {
        if ramp_cycles_per_bin != 0.0 {
            self.rotate(map, -ramp_cycles_per_bin);
        }
        interpolate(map, &self.known, &mut self.h);
        if ramp_cycles_per_bin != 0.0 {
            self.rotate(map, ramp_cycles_per_bin);
        }
        for c in map.occupied() {
            let h = self.h[c.bin];
            let gain = h.norm_sqr();
            self.gain[c.bin] = gain;
            self.inv[c.bin] = if gain > 0.0 {
                h.conj() / gain
            } else {
                Complex::new(0.0, 0.0)
            };
        }
        self.noise_var = noise_var.max(MIN_NOISE_VAR);
    }

    fn rotate(&mut self, map: &SubcarrierMap, cycles_per_bin: f64) {
        for c in map.occupied() {
            let phase = std::f64::consts::TAU * cycles_per_bin * f64::from(c.offset);
            let (sin, cos) = phase.sin_cos();
            let h = self.h[c.bin];
            self.h[c.bin] = Complex::new(
                (f64::from(h.re) * cos - f64::from(h.im) * sin) as f32,
                (f64::from(h.re) * sin + f64::from(h.im) * cos) as f32,
            );
        }
    }

    #[must_use]
    pub fn h(&self, bin: usize) -> Complex<f32> {
        self.h[bin]
    }

    #[must_use]
    pub fn gain(&self, bin: usize) -> f32 {
        self.gain[bin]
    }

    #[must_use]
    pub fn equalize(&self, bin: usize, y: Complex<f32>) -> Complex<f32> {
        y * self.inv[bin]
    }

    #[must_use]
    pub fn noise_var(&self) -> f64 {
        self.noise_var
    }

    #[must_use]
    pub fn bin_noise_var(&self, bin: usize) -> f64 {
        let gain = f64::from(self.gain[bin]);
        if gain > 0.0 {
            self.noise_var / gain
        } else {
            f64::INFINITY
        }
    }
}

pub fn interpolate(map: &SubcarrierMap, known: &[bool], h: &mut [Complex<f32>]) {
    let occupied = map.occupied();
    let Some(first) = occupied.iter().position(|c| known[c.bin]) else {
        return;
    };
    let Some(last) = occupied.iter().rposition(|c| known[c.bin]) else {
        return;
    };
    for i in 0..first {
        h[occupied[i].bin] = h[occupied[first].bin];
    }
    for i in last + 1..occupied.len() {
        h[occupied[i].bin] = h[occupied[last].bin];
    }
    let mut lo = first;
    while lo < last {
        let hi = (lo + 1..=last)
            .find(|&j| known[occupied[j].bin])
            .unwrap_or(last);
        let (a, b) = (occupied[lo], occupied[hi]);
        let span = f64::from(b.offset - a.offset);
        for c in &occupied[lo + 1..hi] {
            let t = (f64::from(c.offset - a.offset) / span) as f32;
            h[c.bin] = h[a.bin] * (1.0 - t) + h[b.bin] * t;
        }
        lo = hi;
    }
}

#[must_use]
pub fn noise_var_from_repeats(first: &[Complex<f32>], second: &[Complex<f32>]) -> f64 {
    if first.is_empty() {
        return 0.0;
    }
    let sum: f64 = first
        .iter()
        .zip(second)
        .map(|(a, b)| f64::from((a - b).norm_sqr()))
        .sum();
    sum / (2.0 * first.len() as f64)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PilotFit {
    pub common_rad: f64,
    pub slope_rad_per_bin: f64,
}

impl PilotFit {
    #[must_use]
    pub fn phase_at(&self, offset: i32) -> f64 {
        self.common_rad + self.slope_rad_per_bin * f64::from(offset)
    }
}

pub const TRACK_ALPHA: f64 = 0.35;

pub const TRACK_BETA: f64 = TRACK_ALPHA * TRACK_ALPHA / (2.0 - TRACK_ALPHA);

#[derive(Clone, Debug, Default)]
pub struct PilotTracker {
    state: PilotFit,
    rate: PilotFit,
    symbols: usize,
}

impl PilotTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.state = PilotFit::default();
        self.rate = PilotFit::default();
        self.symbols = 0;
    }

    pub fn fit(&mut self, offsets: &[i32], errors: &[Complex<f32>], weights: &[f32]) -> PilotFit {
        let predicted = PilotFit {
            common_rad: self.state.common_rad + self.rate.common_rad,
            slope_rad_per_bin: self.state.slope_rad_per_bin + self.rate.slope_rad_per_bin,
        };
        let Some(measured) = weighted_line(offsets, errors, weights, predicted) else {
            self.state = predicted;
            self.symbols += 1;
            return predicted;
        };
        let fit = if self.symbols == 0 {
            self.rate = PilotFit::default();
            measured
        } else {
            let residual = PilotFit {
                common_rad: measured.common_rad - predicted.common_rad,
                slope_rad_per_bin: measured.slope_rad_per_bin - predicted.slope_rad_per_bin,
            };
            self.rate = PilotFit {
                common_rad: self.rate.common_rad + TRACK_BETA * residual.common_rad,
                slope_rad_per_bin: self.rate.slope_rad_per_bin
                    + TRACK_BETA * residual.slope_rad_per_bin,
            };
            PilotFit {
                common_rad: predicted.common_rad + TRACK_ALPHA * residual.common_rad,
                slope_rad_per_bin: predicted.slope_rad_per_bin
                    + TRACK_ALPHA * residual.slope_rad_per_bin,
            }
        };
        self.state = fit;
        self.symbols += 1;
        fit
    }

    #[must_use]
    pub fn last(&self) -> PilotFit {
        self.state
    }

    #[must_use]
    pub fn symbols(&self) -> usize {
        self.symbols
    }
}

fn weighted_line(
    offsets: &[i32],
    errors: &[Complex<f32>],
    weights: &[f32],
    predicted: PilotFit,
) -> Option<PilotFit> {
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for ((&offset, &error), &weight) in offsets.iter().zip(errors).zip(weights) {
        let w = f64::from(weight);
        if w <= 0.0 {
            continue;
        }
        let at = predicted.phase_at(offset);
        let residual =
            Complex::new(f64::from(error.re), f64::from(error.im)) * Complex::from_polar(1.0, -at);
        let phase = at + residual.arg();
        let x = f64::from(offset);
        sw += w;
        sx += w * x;
        sy += w * phase;
        sxx += w * x * x;
        sxy += w * x * phase;
    }
    if sw <= 0.0 {
        return None;
    }
    let det = sw * sxx - sx * sx;
    if det.abs() <= 1e-12 {
        return Some(PilotFit {
            common_rad: sy / sw,
            slope_rad_per_bin: 0.0,
        });
    }
    Some(PilotFit {
        common_rad: (sxx * sy - sx * sxy) / det,
        slope_rad_per_bin: (sw * sxy - sx * sy) / det,
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use super::*;
    use crate::ofdm::params::OfdmParams;

    fn map() -> SubcarrierMap {
        OfdmParams::wifi_like().map().clone()
    }

    #[test]
    fn interpolation_reproduces_a_linear_channel_exactly() {
        let map = map();
        let truth = |offset: i32| Complex::new(1.0 + 0.01 * offset as f32, 0.02 * offset as f32);
        let mut h = vec![Complex::new(0.0, 0.0); map.fft()];
        let mut known = vec![false; map.fft()];
        for c in map.occupied().iter().filter(|c| c.offset % 4 == 0) {
            h[c.bin] = truth(c.offset);
            known[c.bin] = true;
        }
        interpolate(&map, &known, &mut h);
        for c in map.occupied().iter().filter(|c| c.offset.abs() <= 24) {
            let want = truth(c.offset);
            assert!(
                (h[c.bin] - want).norm() < 1e-5,
                "bin {} ({}): {:?} vs {want:?}",
                c.bin,
                c.offset,
                h[c.bin]
            );
        }
    }

    #[test]
    fn interpolation_extends_flat_past_the_outermost_anchor() {
        let map = map();
        let mut h = vec![Complex::new(0.0, 0.0); map.fft()];
        let mut known = vec![false; map.fft()];
        for c in map.occupied().iter().filter(|c| c.offset.abs() <= 4) {
            h[c.bin] = Complex::new(c.offset as f32, 0.0);
            known[c.bin] = true;
        }
        interpolate(&map, &known, &mut h);
        let edge = |offset: i32| {
            h[map
                .occupied()
                .iter()
                .find(|c| c.offset == offset)
                .unwrap()
                .bin]
        };
        assert!((edge(26) - edge(4)).norm() < 1e-6);
        assert!((edge(-26) - edge(-4)).norm() < 1e-6);
        let mut empty = vec![Complex::new(0.0, 0.0); map.fft()];
        interpolate(&map, &vec![false; map.fft()], &mut empty);
        assert!(empty.iter().all(|v| *v == Complex::new(0.0, 0.0)));
    }

    #[test]
    fn averaged_repeats_leave_half_the_noise_variance() {
        use crate::ber::rng::Rng;
        let mut rng = Rng::new(0x1e5);
        for &sigma2 in &[0.01f64, 0.1, 1.0] {
            let sigma = (sigma2 / 2.0).sqrt();
            let draw = |rng: &mut Rng| {
                Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32)
            };
            let n = 20_000;
            let (mut first, mut second) = (Vec::with_capacity(n), Vec::with_capacity(n));
            let mut error = 0.0f64;
            for _ in 0..n {
                let (a, b) = (
                    Complex::new(1.0, 0.0) + draw(&mut rng),
                    Complex::new(1.0, 0.0) + draw(&mut rng),
                );
                let estimate = (a + b) * 0.5;
                error += f64::from((estimate - Complex::new(1.0, 0.0)).norm_sqr());
                first.push(a);
                second.push(b);
            }
            let measured_var = noise_var_from_repeats(&first, &second);
            assert!(
                (measured_var / sigma2 - 1.0).abs() < 0.05,
                "σ² {sigma2}: measured {measured_var}"
            );
            let mse = error / n as f64;
            assert!(
                (mse / (sigma2 / 2.0) - 1.0).abs() < 0.05,
                "σ² {sigma2}: estimator MSE {mse}, closed form {}",
                sigma2 / 2.0
            );
        }
    }

    #[test]
    fn the_pilot_fit_recovers_a_line_and_ignores_dead_pilots() {
        let offsets = [-21i32, -7, 7, 21];
        let truth = PilotFit {
            common_rad: 0.3,
            slope_rad_per_bin: 0.01,
        };
        let errors: Vec<Complex<f32>> = offsets
            .iter()
            .map(|&k| {
                let p = truth.phase_at(k);
                Complex::new(p.cos() as f32, p.sin() as f32)
            })
            .collect();
        let mut tracker = PilotTracker::new();
        let fit = tracker.fit(&offsets, &errors, &[1.0; 4]);
        assert!((fit.common_rad - truth.common_rad).abs() < 1e-6, "{fit:?}");
        assert!((fit.slope_rad_per_bin - truth.slope_rad_per_bin).abs() < 1e-9);

        let mut spoiled = errors.clone();
        spoiled[0] = Complex::new(-1.0, 0.0);
        tracker.reset();
        let fit = tracker.fit(&offsets, &spoiled, &[0.0, 1.0, 1.0, 1.0]);
        assert!((fit.common_rad - truth.common_rad).abs() < 1e-6, "{fit:?}");
    }

    #[test]
    fn tracking_follows_a_slope_past_the_unwrapping_ambiguity() {
        let pilots = [-21i32, -7, 7, 21];
        let step = TAU * 0.24 / 64.0;
        let wrap = |e: f64| (e + PI).rem_euclid(TAU) - PI;
        let mut tracked = PilotTracker::new();
        let mut untracked = PilotTracker::new();
        let (mut transient, mut steady, mut worst_untracked) = (0.0f64, 0.0f64, 0.0f64);
        for symbol in 0..64 {
            let truth = PilotFit {
                common_rad: 0.0,
                slope_rad_per_bin: step * f64::from(symbol),
            };
            let errors: Vec<Complex<f32>> = pilots
                .iter()
                .map(|&k| {
                    let p = truth.phase_at(k);
                    Complex::new(p.cos() as f32, p.sin() as f32)
                })
                .collect();
            let a = tracked.fit(&pilots, &errors, &[1.0; 4]);
            untracked.reset();
            let b = untracked.fit(&pilots, &errors, &[1.0; 4]);
            let residual = |fit: PilotFit| {
                (-26..=26)
                    .map(|k| wrap(fit.phase_at(k) - truth.phase_at(k)).abs())
                    .fold(0.0f64, f64::max)
            };
            transient = transient.max(residual(a));
            if symbol >= 32 {
                steady = steady.max(residual(a));
            }
            worst_untracked = worst_untracked.max(residual(b));
        }
        assert!(transient < PI / 2.0, "transient residual {transient} rad");
        assert!(steady < 0.02, "steady-state residual {steady} rad");
        assert_eq!(tracked.symbols(), 64);
        assert!(
            worst_untracked > 1.0,
            "untracked residual {worst_untracked} rad — the ambiguity was never reached, so \
             this test is not measuring what it claims"
        );
    }

    #[test]
    fn a_nulled_bin_erases_instead_of_amplifying() {
        let map = map();
        let mut estimate = ChannelEstimate::new(map.fft());
        for c in map.occupied() {
            estimate.set(c.bin, Complex::new(1.0, 0.0));
        }
        let dead = map.data()[3].bin;
        estimate.set(dead, Complex::new(0.0, 0.0));
        estimate.finish(&map, 0.01, 0.0);
        assert_eq!(
            estimate.equalize(dead, Complex::new(0.4, -0.2)),
            Complex::new(0.0, 0.0)
        );
        assert!(estimate.bin_noise_var(dead).is_infinite());
        let live = map.data()[4].bin;
        assert!((estimate.bin_noise_var(live) - 0.01).abs() < 1e-9);
    }
}
