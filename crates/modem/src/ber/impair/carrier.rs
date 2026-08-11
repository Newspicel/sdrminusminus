//! Carrier-path impairments: static CFO, linear frequency drift, and Wiener phase noise —
//! everything a receive LO does to a signal short of losing it. All three are pure phase
//! multiplications, so they share one discipline: the phase register is `f64` and wrapped to
//! [0, 1) cycles every sample, because an unwrapped accumulator loses precision exactly when
//! the waveform gets long enough for a sweep to care.

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

/// Static carrier frequency offset. Stored normalised (cycles per sample) so the model is
/// rate-free; the Hz constructor exists because limits tables state the axis in Hz at a
/// stated sample rate.
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

/// Linear frequency drift: instantaneous frequency `drift · t` Hz, i.e. quadratic phase.
/// Stored as cycles/sample² so, like [`Cfo`], the model itself is rate-free.
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
            // Trapezoidal step keeps φ[n] = d·n²/2 exactly: the half-step is the average
            // frequency across the sample interval, not an approximation.
            acc += freq + 0.5 * d;
            acc -= acc.floor();
            freq += d;
        }
    }
}

/// Wiener (random-walk) phase noise: `φ[n] = φ[n-1] + w[n]`, `w ~ N(0, q)`. The mask shape is
/// 1/f² — the far tail of a free-running oscillator's Lorentzian, which is the part that
/// stresses a carrier loop; flicker and floor regions are deliberately not modelled.
///
/// The stated level is the *integrated* RMS in degrees over the whole waveform, ensemble
/// sense: `E[(1/N)·Σ φ[n]²] = rms²`. A single walk realisation scatters widely around that —
/// which is exactly the realisation-to-realisation behaviour a limits sweep should average
/// over, so the level is defined on the ensemble and calibrated the same way.
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
        // E[φ[n]²] = n·q, so the mean over n of the expected square is q·(N−1)/2; solving for
        // the stated integrated RMS gives the per-step variance.
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

    /// Applied == measured: the phase slope read back off the rotated waveform is the
    /// constructed offset. Tolerance is set by f32 sample storage (~1e-7 rad/sample,
    /// averaged down over 50k samples).
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

    /// Applied == measured via the phase's second difference: for quadratic phase it is the
    /// constant 2π·d cycles/sample², rate-scaled back to Hz/s.
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

    /// Applied == measured on the ensemble the level is defined over: the RMS of the walk's
    /// phase trajectory, unwrapped from the waveform and pooled across 600 realisations.
    /// A single walk's integrated RMS has ~unit relative variance, so the pooling is what
    /// brings the estimator inside the 5% gate (~2% expected error at M=600).
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
            // First sample carries φ=0 by construction; the increments walk from there.
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
