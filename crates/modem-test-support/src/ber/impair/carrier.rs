use std::f64::consts::TAU;

use num_complex::Complex;

use super::Impairment;
use crate::ber::rng::Rng;

fn rotate(s: &mut Complex<f32>, cycles: f64) {
    let phase = TAU * cycles;
    let (sin, cos) = phase.sin_cos();
    let re = f64::from(s.re);
    let im = f64::from(s.im);
    s.re = (re * cos - im * sin) as f32;
    s.im = (re * sin + im * cos) as f32;
}

#[derive(Clone, Copy, Debug)]
pub struct Cfo {
    cycles_per_sample: f64,
}

impl Cfo {
    #[must_use]
    pub fn from_hz(offset_hz: f64, sample_rate_hz: f64) -> Self {
        Self {
            cycles_per_sample: offset_hz / sample_rate_hz,
        }
    }

    #[must_use]
    pub fn from_cycles_per_sample(cycles_per_sample: f64) -> Self {
        Self { cycles_per_sample }
    }
}

impl Impairment for Cfo {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let mut acc = 0.0f64;
        for s in x.iter_mut() {
            rotate(s, acc);
            acc += self.cycles_per_sample;
            acc -= acc.floor();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Drift {
    cycles_per_sample2: f64,
}

impl Drift {
    #[must_use]
    pub fn from_hz_per_s(hz_per_s: f64, sample_rate_hz: f64) -> Self {
        Self {
            cycles_per_sample2: hz_per_s / (sample_rate_hz * sample_rate_hz),
        }
    }
}

impl Impairment for Drift {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let d = self.cycles_per_sample2;
        let mut freq = 0.0f64;
        let mut acc = 0.0f64;
        for s in x.iter_mut() {
            rotate(s, acc);
            acc += freq + 0.5 * d;
            acc -= acc.floor();
            freq += d;
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PhaseNoise {
    rms_deg: f64,
}

impl PhaseNoise {
    #[must_use]
    pub fn new(rms_deg: f64) -> Self {
        Self { rms_deg }
    }
}

impl Impairment for PhaseNoise {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        if x.len() < 2 {
            return;
        }
        let rms_cycles = self.rms_deg / 360.0;
        let q = 2.0 * rms_cycles * rms_cycles / (x.len() - 1) as f64;
        let step = q.sqrt();
        let mut acc = 0.0f64;
        for s in x.iter_mut() {
            rotate(s, acc);
            acc += step * rng.normal();
            acc -= acc.floor();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{Cfo, Drift, PhaseNoise};
    use crate::ber::{
        impair::{
            Impairment,
            testutil::{arg_increments, ones},
        },
        rng::Rng,
    };

    #[test]
    fn cfo_phase_slope_reads_back() {
        let applied = Cfo::from_hz(1234.5, 48_000.0);
        let mut x = ones(50_000);
        applied.apply(&mut x, &mut Rng::new(0));
        let incs = arg_increments(&x);
        let mean = incs.iter().sum::<f64>() / incs.len() as f64;
        let measured_hz = mean / TAU * 48_000.0;
        assert!(
            (measured_hz - 1234.5).abs() < 0.01,
            "measured {measured_hz} Hz"
        );
    }

    #[test]
    fn drift_second_difference_reads_back() {
        let rate = 48_000.0;
        let applied_hz_per_s = 500.0;
        let mut x = ones(48_000);
        Drift::from_hz_per_s(applied_hz_per_s, rate).apply(&mut x, &mut Rng::new(0));
        let incs = arg_increments(&x);
        let second: f64 =
            incs.windows(2).map(|w| w[1] - w[0]).sum::<f64>() / (incs.len() - 1) as f64;
        let measured = second / TAU * rate * rate;
        assert!(
            (measured / applied_hz_per_s - 1.0).abs() < 0.01,
            "measured {measured} Hz/s"
        );
    }

    #[test]
    fn phase_noise_integrated_rms_reads_back() {
        let rms_deg = 10.0;
        let applied = PhaseNoise::new(rms_deg);
        let mut rng = Rng::new(0x1f2f);
        let n = 1024;
        let runs = 600;
        let mut sum_sq = 0.0f64;
        for _ in 0..runs {
            let mut x = ones(n);
            applied.apply(&mut x, &mut rng);
            let mut phi = 0.0f64;
            sum_sq += arg_increments(&x)
                .iter()
                .map(|d| {
                    phi += d;
                    phi * phi
                })
                .sum::<f64>();
        }
        let measured_deg = (sum_sq / (runs * (n - 1)) as f64).sqrt().to_degrees();
        assert!(
            (measured_deg / rms_deg - 1.0).abs() < 0.05,
            "applied {rms_deg}° RMS, measured {measured_deg}°"
        );
    }
}
