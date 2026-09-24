use std::f64::consts::{FRAC_1_SQRT_2, PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::LoopFilter;

const FREQ_LIMIT_CYCLES_PER_SYMBOL: f64 = 0.25;

const AMPLITUDE_SYMBOLS: f32 = 32.0;

#[derive(Clone, Debug)]
pub(super) struct LaurentCostas {
    acquire: LoopFilter,
    track: LoopFilter,
    phase: f64,
    isi: f32,
    amplitude: f32,
    held: [Complex<f32>; 2],
}

impl LaurentCostas {
    pub(super) fn new(acquire_bw: f64, track_bw: f64, isi: f32) -> Self {
        Self {
            acquire: LoopFilter::new(acquire_bw, FRAC_1_SQRT_2, FREQ_LIMIT_CYCLES_PER_SYMBOL),
            track: LoopFilter::new(track_bw, FRAC_1_SQRT_2, FREQ_LIMIT_CYCLES_PER_SYMBOL),
            phase: 0.0,
            isi,
            amplitude: 1.0,
            held: [Complex::new(0.0, 0.0); 2],
        }
    }

    pub(super) fn advance(&mut self, y: Complex<f32>, locked: bool) -> Complex<f32> {
        let z = y * Complex::from_polar(1.0, -self.phase as f32);
        let [before, centre] = self.held;
        self.held = [centre, z];
        let error = self.error(before, centre, z);
        let step = if locked {
            let freq = self.acquire.freq_norm();
            self.track.reset(freq);
            let step = self.track.advance(error);
            self.acquire.reset(self.track.freq_norm());
            step
        } else {
            let step = self.acquire.advance(error);
            self.track.reset(self.acquire.freq_norm());
            step
        };
        self.phase = wrap(self.phase + step);
        z
    }

    fn error(&mut self, before: Complex<f32>, centre: Complex<f32>, after: Complex<f32>) -> f64 {
        let spread = decide(after) - decide(before);
        self.amplitude += (centre.re.abs() - self.amplitude) / AMPLITUDE_SYMBOLS;
        let residue = centre.im - self.isi * self.amplitude * spread;
        if self.amplitude <= f32::MIN_POSITIVE {
            return 0.0;
        }
        f64::from((residue * decide(centre) / self.amplitude).clamp(-1.0, 1.0))
    }

    pub(super) fn clean(
        &self,
        z: Complex<f32>,
        before: Complex<f32>,
        after: Complex<f32>,
    ) -> Complex<f32> {
        Complex::new(
            z.re,
            z.im - self.isi * self.amplitude * (decide(after) - decide(before)),
        )
    }

    pub(super) fn freq_cycles_per_symbol(&self) -> f64 {
        self.acquire.freq_norm()
    }

    pub(super) fn shed_frequency(&mut self, fraction: f64) -> f64 {
        let freq = self.acquire.freq_norm();
        let shed = freq * fraction.clamp(0.0, 1.0);
        self.acquire.reset(freq - shed);
        self.track.reset(freq - shed);
        shed
    }

    pub(super) fn reset(&mut self) {
        self.acquire.reset(0.0);
        self.track.reset(0.0);
        self.phase = 0.0;
        self.amplitude = 1.0;
        self.held = [Complex::new(0.0, 0.0); 2];
    }
}

fn decide(z: Complex<f32>) -> f32 {
    if z.re >= 0.0 { 1.0 } else { -1.0 }
}

fn wrap(theta: f64) -> f64 {
    (theta + PI).rem_euclid(TAU) - PI
}
