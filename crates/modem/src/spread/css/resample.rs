use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::CssDemod;

pub(super) const HALF_TAPS: usize = 16;

const PHASES: usize = 256;

const KAISER_BETA: f64 = 8.0;

#[derive(Clone, Debug)]
pub(super) struct Interpolator {
    table: Vec<f32>,
}

impl Interpolator {
    pub(super) fn new() -> Self {
        let taps = 2 * HALF_TAPS;
        let mut table = Vec::with_capacity((PHASES + 1) * taps);
        for phase in 0..=PHASES {
            let fraction = phase as f64 / PHASES as f64;
            let row: Vec<f64> = (0..taps)
                .map(|i| kernel(fraction + (HALF_TAPS - 1) as f64 - i as f64))
                .collect();
            let gain: f64 = row.iter().sum();
            table.extend(row.iter().map(|w| (w / gain) as f32));
        }
        Self { table }
    }

    fn row(&self, fraction: f64) -> &[f32] {
        let taps = 2 * HALF_TAPS;
        let phase = (fraction * PHASES as f64).round() as usize;
        &self.table[phase * taps..(phase + 1) * taps]
    }
}

fn kernel(x: f64) -> f64 {
    let ratio = x / HALF_TAPS as f64;
    if ratio.abs() >= 1.0 {
        return 0.0;
    }
    let sinc = if x.abs() < 1e-12 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    };
    sinc * bessel_i0(KAISER_BETA * (1.0 - ratio * ratio).sqrt()) / bessel_i0(KAISER_BETA)
}

fn bessel_i0(x: f64) -> f64 {
    let quarter = x * x / 4.0;
    let mut term = 1.0;
    let mut sum = 1.0;
    for k in 1..32 {
        term *= quarter / (k * k) as f64;
        sum += term;
    }
    sum
}

pub(super) fn source_len(chips: usize) -> usize {
    chips + chips / 256 + 2 * HALF_TAPS + 8
}

impl CssDemod {
    pub(super) fn load_resampled(
        &mut self,
        iq: &[Complex<f32>],
        start: f64,
        spin: f64,
        rate: f64,
    ) -> bool {
        let n = self.window.len();
        let first = start.floor() as i64 - HALF_TAPS as i64 + 1;
        let last = (start + (n - 1) as f64 * rate).floor() as i64 + HALF_TAPS as i64;
        let span = usize::try_from(last - first + 1).unwrap_or(usize::MAX);
        if span > self.source.len() {
            return false;
        }
        self.derotate_source(iq, first, span, start, spin);
        for u in 0..n {
            let t = start + u as f64 * rate - first as f64;
            let base = t.floor();
            let row = self.interpolator.row(t - base);
            let from = base as usize + 1 - HALF_TAPS;
            let mut acc = Complex::new(0.0f32, 0.0);
            for (w, s) in row.iter().zip(&self.source[from..from + 2 * HALF_TAPS]) {
                acc += s * *w;
            }
            self.window[u] = acc;
        }
        true
    }

    fn derotate_source(
        &mut self,
        iq: &[Complex<f32>],
        first: i64,
        span: usize,
        start: f64,
        spin: f64,
    ) {
        let n = self.window.len() as f64;
        let step = Complex::from_polar(1.0, -TAU * spin / n);
        let mut turn = Complex::from_polar(1.0, -TAU * spin * (first as f64 - start) / n);
        for (i, slot) in self.source[..span].iter_mut().enumerate() {
            let sample: Complex<f32> = usize::try_from(first + i as i64)
                .ok()
                .and_then(|at| iq.get(at))
                .copied()
                .unwrap_or_default();
            *slot = sample * Complex::new(turn.re as f32, turn.im as f32);
            turn *= step;
        }
    }
}
