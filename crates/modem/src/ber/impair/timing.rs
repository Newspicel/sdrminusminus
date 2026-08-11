//! Sampling-clock impairments: static fractional delay, sample-clock ppm error, and timing
//! jitter. All three re-evaluate the waveform at shifted instants through the shared
//! windowed-sinc interpolator ([`super::sinc`]), so "a fraction of a sample" means the same
//! thing on every timing axis. Each clones the input — the instrument's quality bar is the
//! interpolator's, not an in-place trick's, and none of this is a hot path.

use num_complex::Complex;

use super::{Impairment, sinc::interp};
use crate::ber::rng::Rng;

/// Static delay of `delay_samples` (fractionally, ≥ 0): `y[n] = x(n − d)`. Sub-sample timing
/// offset is the axis a symbol-sync loop's acquisition range is measured on.
#[derive(Clone, Copy, Debug)]
pub struct TimingOffset {
    delay_samples: f64,
}

impl TimingOffset {
    #[must_use]
    pub fn new(delay_samples: f64) -> Self {
        debug_assert!(delay_samples >= 0.0, "a negative delay is an advance");
        Self { delay_samples }
    }
}

impl Impairment for TimingOffset {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let src = x.clone();
        for (n, s) in x.iter_mut().enumerate() {
            *s = interp(&src, n as f64 - self.delay_samples);
        }
    }
}

/// Sample-clock error in parts per million: the waveform is resampled by `1 + ppm·1e−6`.
/// Positive ppm models a receiver clock running fast — it takes more samples of the same
/// signal, so the output is longer and every feature in it drifts later at `ppm·1e−6`
/// samples per sample. This is the one impairment here that changes the length.
#[derive(Clone, Copy, Debug)]
pub struct ClockError {
    ppm: f64,
}

impl ClockError {
    #[must_use]
    pub fn new(ppm: f64) -> Self {
        Self { ppm }
    }
}

impl Impairment for ClockError {
    fn apply(&self, x: &mut Vec<Complex<f32>>, _rng: &mut Rng) {
        let ratio = 1.0 + self.ppm * 1e-6;
        let src = std::mem::take(x);
        let out_len = (src.len() as f64 * ratio).floor() as usize;
        x.reserve(out_len);
        for m in 0..out_len {
            x.push(interp(&src, m as f64 / ratio));
        }
    }
}

/// The jitter's spectral character — stated, because a timing loop that shrugs off white
/// jitter can still be walked away from by a random-walk clock, and a limits row must say
/// which one it measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JitterKind {
    /// Independent per-sample timing error — an ADC's aperture jitter.
    White,
    /// Wiener-process timing error — an unstabilised clock wandering. Like
    /// [`PhaseNoise`](super::PhaseNoise), the stated RMS is the ensemble integrated RMS over
    /// the waveform, and it is calibrated on the ensemble.
    RandomWalk,
}

/// Per-sample timing jitter: `y[n] = x(n + j[n])`. The level is stated in fractions of a
/// symbol — the unit a timing loop's tolerance is quoted in — and converted through the
/// stated samples-per-symbol at construction.
#[derive(Clone, Copy, Debug)]
pub struct TimingJitter {
    kind: JitterKind,
    rms_samples: f64,
}

impl TimingJitter {
    #[must_use]
    pub fn new(kind: JitterKind, rms_symbol_fraction: f64, samples_per_symbol: f64) -> Self {
        Self {
            kind,
            rms_samples: rms_symbol_fraction * samples_per_symbol,
        }
    }
}

impl Impairment for TimingJitter {
    fn apply(&self, x: &mut Vec<Complex<f32>>, rng: &mut Rng) {
        if x.len() < 2 {
            return;
        }
        let src = x.clone();
        match self.kind {
            JitterKind::White => {
                for (n, s) in x.iter_mut().enumerate() {
                    *s = interp(&src, n as f64 + self.rms_samples * rng.normal());
                }
            }
            JitterKind::RandomWalk => {
                // Same ensemble bookkeeping as the Wiener phase noise: per-step variance q
                // chosen so E[(1/N)·Σ j[n]²] equals the stated RMS².
                let q = 2.0 * self.rms_samples * self.rms_samples / (src.len() - 1) as f64;
                let step = q.sqrt();
                let mut j = 0.0f64;
                for (n, s) in x.iter_mut().enumerate() {
                    *s = interp(&src, n as f64 + j);
                    j += step * rng.normal();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::{ClockError, JitterKind, TimingJitter, TimingOffset};
    use crate::ber::{
        impair::{
            Impairment,
            interference::rrc_qpsk,
            sinc::edge_guard,
            testutil::{est_delay, tone},
        },
        rng::Rng,
    };

    /// Applied == measured: cross-correlating the delayed waveform against the original
    /// recovers the constructed delay within 0.01 samples, integer and fractional parts both.
    #[test]
    fn fractional_delay_recovered_by_correlation() {
        let x = rrc_qpsk(&mut Rng::new(0xde1a), 8192, 4, 0.35);
        for applied in [0.415, 3.37] {
            let mut y = x.clone();
            TimingOffset::new(applied).apply(&mut y, &mut Rng::new(0));
            let measured = est_delay(&x, &y, 64..8192 - 64, 8);
            assert!(
                (measured - applied).abs() < 0.01,
                "applied {applied}, measured {measured}"
            );
        }
    }

    /// Applied == measured twice over: the output length carries the resampling ratio, and
    /// the delay measured by correlation at two ends of the waveform drifts at the applied
    /// ppm.
    #[test]
    fn clock_ppm_reads_back_from_length_and_drift() {
        let applied_ppm = 200.0;
        let len = 131_072usize;
        let x = rrc_qpsk(&mut Rng::new(0xc10c), len, 4, 0.35);
        let mut y = x.clone();
        ClockError::new(applied_ppm).apply(&mut y, &mut Rng::new(0));

        let expect_len = (len as f64 * (1.0 + applied_ppm * 1e-6)).floor();
        assert!(
            (y.len() as f64 - expect_len).abs() <= 1.0,
            "len {}",
            y.len()
        );

        let early = est_delay(&x, &y, 2_000..6_000, 4);
        let late = est_delay(&x, &y, 120_000..124_000, 64);
        let slope = (late - early) / (122_000.0 - 4_000.0);
        let measured_ppm = slope * 1e6;
        assert!(
            (measured_ppm - applied_ppm).abs() < 5.0,
            "applied {applied_ppm} ppm, measured {measured_ppm}"
        );
    }

    /// Applied == measured for white jitter: on a tone, each sample's timing error appears
    /// as phase error 2π·f·j[n], so the phase-deviation RMS divided by 2π·f reads the jitter
    /// back directly.
    #[test]
    fn white_jitter_rms_reads_back_on_a_tone() {
        let f = 0.2;
        let sps = 4.0;
        let applied_fraction = 0.015; // 0.06 samples RMS
        let n = 65_536;
        let x = tone(f, n);
        let mut y = x.clone();
        TimingJitter::new(JitterKind::White, applied_fraction, sps)
            .apply(&mut y, &mut Rng::new(0x717e));
        let guard = edge_guard();
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for i in guard..n - guard {
            let err = (y[i] * x[i].conj()).arg();
            let j = f64::from(err) / (TAU * f);
            sum_sq += j * j;
            count += 1;
        }
        let measured = (sum_sq / count as f64).sqrt() / sps;
        assert!(
            (measured / applied_fraction - 1.0).abs() < 0.05,
            "applied {applied_fraction} sym RMS, measured {measured}"
        );
    }

    /// Random-walk jitter calibrates on the ensemble its RMS is defined over, same as the
    /// Wiener phase noise; 400 realisations bring the estimator well inside the 5% gate.
    #[test]
    fn random_walk_jitter_rms_reads_back_on_the_ensemble() {
        let f = 0.2;
        let sps = 4.0;
        let applied_fraction = 0.0125; // 0.05 samples RMS
        let n = 2_048;
        let x = tone(f, n);
        let jitter = TimingJitter::new(JitterKind::RandomWalk, applied_fraction, sps);
        let mut rng = Rng::new(0x7a1c);
        let guard = edge_guard();
        let mut sum_sq = 0.0f64;
        let mut count = 0usize;
        for _ in 0..400 {
            let mut y = x.clone();
            jitter.apply(&mut y, &mut rng);
            for i in guard..n - guard {
                let err = (y[i] * x[i].conj()).arg();
                let j = f64::from(err) / (TAU * f);
                sum_sq += j * j;
                count += 1;
            }
        }
        let measured = (sum_sq / count as f64).sqrt() / sps;
        assert!(
            (measured / applied_fraction - 1.0).abs() < 0.05,
            "applied {applied_fraction} sym RMS, measured {measured}"
        );
    }
}
