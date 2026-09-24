use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::CssDemod;

const FRACTION_FLOOR: f64 = 1e-6;

const STRETCH_TOLERANCE: f64 = 1e-3;

const FOLD_FLOOR: f64 = 0.05;

impl CssDemod {
    pub(super) fn load(&mut self, iq: &[Complex<f32>], start: f64, spin: f64, period: f64) {
        let n = self.window.len() as f64;
        let rate = period / n;
        let fold = n * (rate - 1.0);
        if fold.abs() > FOLD_FLOOR && self.load_resampled(iq, start, spin, rate) {
            self.set_stretch(1.0);
            return;
        }
        self.load_advanced(iq, start, spin);
        self.set_stretch(rate.recip());
    }

    fn load_advanced(&mut self, iq: &[Complex<f32>], start: f64, spin: f64) {
        let whole = start.round();
        let base = whole as i64;
        let n = self.window.len();
        let step = Complex::from_polar(1.0, -TAU * spin / n as f64);
        let mut turn = Complex::from_polar(1.0, -TAU * spin * (whole - start) / n as f64);
        for (offset, slot) in self.window.iter_mut().enumerate() {
            let sample: Complex<f32> = usize::try_from(base + offset as i64)
                .ok()
                .and_then(|at| iq.get(at))
                .copied()
                .unwrap_or_default();
            *slot = sample * narrow(turn);
            turn *= step;
        }
        let fraction = start - whole;
        if fraction.abs() > FRACTION_FLOOR {
            self.advance_window(fraction);
        }
    }

    fn advance_window(&mut self, fraction: f64) {
        let n = self.window.len();
        let half = n / 2;
        self.fft
            .process_with_scratch(&mut self.window, &mut self.scratch);
        let step = Complex::from_polar(1.0, TAU * fraction / n as f64);
        let mut up = Complex::new(1.0f64, 0.0);
        let mut down = step.conj();
        for m in 0..half {
            self.window[m] *= narrow(up);
            up *= step;
        }
        for m in 1..half {
            self.window[n - m] *= narrow(down);
            down *= step.conj();
        }
        self.window[half] *= (PI * fraction).cos() as f32;
        self.ifft
            .process_with_scratch(&mut self.window, &mut self.scratch);
        let scale = (n as f32).recip();
        for sample in &mut self.window {
            *sample *= scale;
        }
    }

    fn set_stretch(&mut self, stretch: f64) {
        let n = self.reference.len() as f64;
        if ((stretch - self.stretch) * n).abs() <= STRETCH_TOLERANCE {
            return;
        }
        self.stretch = stretch;
        for (k, slot) in self.reference.iter_mut().enumerate() {
            let t = k as f64 * stretch;
            let turns = t * t / (2.0 * n) - t / 2.0;
            let (sin, cos) = (TAU * (turns - turns.floor())).sin_cos();
            *slot = Complex::new(cos as f32, -sin as f32);
        }
    }

    pub(super) fn dechirp(&mut self) {
        let n = self.window.len();
        for k in 0..n {
            let z = self.window[k] * self.reference[k];
            self.dechirped[k] = z;
            self.bins[k] = z;
        }
        self.fft
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        let scale = (n as f32).recip();
        for (slot, bin) in self.energies.iter_mut().zip(&self.bins) {
            *slot = bin.norm_sqr() * scale;
        }
        self.map_symbol_energies();
    }

    fn map_symbol_energies(&mut self) {
        let n = self.energies.len();
        for (value, slot) in self.symbol_energies.iter_mut().enumerate() {
            let bin = (value as f64 * self.stretch).round() as usize % n;
            *slot = self.energies[bin];
        }
    }

    pub(super) fn fraction(&self, peak: usize) -> f64 {
        let n = self.bins.len();
        let at = |i: usize| {
            let b = self.bins[i % n];
            Complex::new(f64::from(b.re), f64::from(b.im))
        };
        let (left, mid, right) = (at(peak + n - 1), at(peak), at(peak + 1));
        let denominator = mid * 2.0 - left - right;
        if denominator.norm() <= 0.0 {
            return 0.0;
        }
        ((left - right) / denominator).re.clamp(-0.5, 0.5)
    }

    pub(super) fn split_tone(&self, expected: f64, split: usize) -> SplitTone {
        let mut tone = expected;
        for _ in 0..REFINE_PASSES {
            tone += self.moments(tone, split).correction(self.dechirped.len());
            tone = tone.clamp(expected - 0.5, expected + 0.5);
        }
        let moments = self.moments(tone, split);
        let [before, after] = moments.sums;
        SplitTone {
            tone,
            late: (after * before.conj()).arg() / TAU,
            weight: before.norm() * after.norm(),
        }
    }

    fn moments(&self, tone: f64, split: usize) -> Moments {
        let n = self.dechirped.len();
        let centres = [split as f64 / 2.0 - 0.5, (split + n) as f64 / 2.0 - 0.5];
        let fold = n as f64 * (1.0 - self.stretch);
        let steps = [tone, tone + fold].map(|t| Complex::from_polar(1.0, -TAU * t / n as f64));
        let mut turns = [Complex::new(1.0f64, 0.0); 2];
        let mut moments = Moments::default();
        for (k, z) in self.dechirped.iter().enumerate() {
            let side = usize::from(k >= split);
            let term = Complex::new(f64::from(z.re), f64::from(z.im)) * turns[side];
            let lever = k as f64 - centres[side];
            moments.sums[side] += term;
            moments.slopes[side] += term * lever;
            moments.spread[side] += lever * lever;
            moments.lengths[side] += 1.0;
            turns[0] *= steps[0];
            turns[1] *= steps[1];
        }
        moments
    }
}

const REFINE_PASSES: usize = 2;

#[derive(Clone, Copy, Debug)]
pub(super) struct SplitTone {
    pub tone: f64,
    pub late: f64,
    pub weight: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Moments {
    sums: [Complex<f64>; 2],
    slopes: [Complex<f64>; 2],
    spread: [f64; 2],
    lengths: [f64; 2],
}

impl Moments {
    fn correction(&self, n: usize) -> f64 {
        let mut num = 0.0;
        let mut den = 0.0;
        for side in 0..2 {
            if self.lengths[side] < 2.0 {
                continue;
            }
            num += (self.slopes[side] * self.sums[side].conj()).im;
            den += self.sums[side].norm_sqr() * self.spread[side] / self.lengths[side];
        }
        if den <= 0.0 {
            return 0.0;
        }
        (n as f64 / TAU * num / den).clamp(-0.5, 0.5)
    }
}

fn narrow(z: Complex<f64>) -> Complex<f32> {
    Complex::new(z.re as f32, z.im as f32)
}
