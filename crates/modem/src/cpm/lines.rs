use std::f64::consts::{PI, TAU};

use num_complex::Complex;

const LINE_SYMBOLS: f64 = 2.0;

const BEAT_SYMBOLS: f64 = 32.0;

#[derive(Clone, Debug)]
pub(super) struct SquaredLines {
    rotor: Vec<Complex<f64>>,
    at: usize,
    alpha: f64,
    beat_alpha: f64,
    upper: Complex<f64>,
    lower: Complex<f64>,
    beat: Complex<f64>,
    sps: f64,
}

impl SquaredLines {
    pub(super) fn new(sps: usize) -> Self {
        let period = 2 * sps.max(1);
        Self {
            rotor: (0..period)
                .map(|n| Complex::from_polar(1.0, -PI * n as f64 / sps.max(1) as f64))
                .collect(),
            at: 0,
            alpha: (LINE_SYMBOLS * sps as f64).recip(),
            beat_alpha: (BEAT_SYMBOLS * sps as f64).recip(),
            upper: Complex::new(0.0, 0.0),
            lower: Complex::new(0.0, 0.0),
            beat: Complex::new(0.0, 0.0),
            sps: sps as f64,
        }
    }

    pub(super) fn push(&mut self, y: Complex<f32>) {
        let wide = Complex::new(f64::from(y.re), f64::from(y.im));
        let square = wide * wide;
        let rotor = self.rotor[self.at];
        self.at = (self.at + 1) % self.rotor.len();
        self.upper += self.alpha * (square * rotor - self.upper);
        self.lower += self.alpha * (square * rotor.conj() - self.lower);
        self.beat += self.beat_alpha * (self.lower * self.upper.conj() - self.beat);
    }

    pub(super) fn epoch_samples(&self) -> Option<f64> {
        (self.beat.norm() > 0.0).then(|| (self.beat.arg() / TAU * self.sps).rem_euclid(self.sps))
    }

    pub(super) fn reset(&mut self) {
        self.at = 0;
        self.upper = Complex::new(0.0, 0.0);
        self.lower = Complex::new(0.0, 0.0);
        self.beat = Complex::new(0.0, 0.0);
    }
}
