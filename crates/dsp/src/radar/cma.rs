use num_complex::Complex;

use super::RadarDspError;
use super::nlms::WindowRatio;

type C32 = Complex<f32>;

pub const MAX_CMA_TAPS: usize = 64;
pub const LOCK_GAIN_DB: f32 = 1.0;

const CLEAN_DISPERSION: f64 = 0.1;
const AGC_RATE: f32 = 1e-4;
const AGC_ATTACK: f32 = 16.0;
const SILENCE_POWER: f32 = 1e-20;
const REGRESSOR_FLOOR: f32 = 1e-6;

pub struct ReferenceCma {
    taps: usize,
    step: f32,
    weights: Vec<C32>,
    history: Vec<C32>,
    head: usize,
    power: f32,
    primed: bool,
    silent: bool,
    fallback_frames: u64,
    dispersion: WindowRatio,
}

impl ReferenceCma {
    pub fn new(taps: usize, step: f32, sample_rate: f64) -> Result<Self, RadarDspError> {
        if !(1..=MAX_CMA_TAPS).contains(&taps) || !(step > 0.0 && step.is_finite()) {
            return Err(RadarDspError::Setting);
        }
        let mut cma = Self {
            taps,
            step,
            weights: vec![C32::default(); taps],
            history: vec![C32::default(); 2 * taps],
            head: 0,
            power: 0.0,
            primed: false,
            silent: false,
            fallback_frames: 0,
            dispersion: WindowRatio::new(sample_rate)?,
        };
        cma.reset_weights();
        Ok(cma)
    }

    pub fn process(&mut self, input: &[C32], out: &mut [C32]) -> usize {
        let len = input.len().min(out.len());
        for (&r, y) in input.iter().zip(out.iter_mut()).take(len) {
            *y = self.step_sample(r);
        }
        len
    }

    #[must_use]
    pub fn gain_db(&self) -> f32 {
        self.dispersion.value_db()
    }

    #[must_use]
    pub fn locked(&self) -> bool {
        !self.silent && (self.gain_db() > LOCK_GAIN_DB || self.clean())
    }

    fn clean(&self) -> bool {
        self.dispersion
            .denominator_mean()
            .is_some_and(|dispersion| dispersion <= CLEAN_DISPERSION)
    }

    #[must_use]
    pub const fn fallback_frames(&self) -> u64 {
        self.fallback_frames
    }

    #[must_use]
    pub fn weights(&self) -> &[C32] {
        &self.weights
    }

    pub fn reset(&mut self) {
        self.reset_weights();
        self.power = 0.0;
        self.primed = false;
        self.silent = false;
        self.dispersion.clear();
    }

    fn reset_weights(&mut self) {
        self.weights.fill(C32::default());
        self.weights[0] = C32::new(1.0, 0.0);
        self.history.fill(C32::default());
        self.head = 0;
    }

    fn normalise(&mut self, sample: C32) -> Option<C32> {
        let energy = sample.norm_sqr();
        if !energy.is_finite() {
            self.primed = false;
            return None;
        }
        if self.primed && energy > AGC_ATTACK * self.power {
            self.power = energy;
        } else if self.primed {
            self.power += AGC_RATE * (energy - self.power);
            if self.power <= SILENCE_POWER {
                self.primed = false;
                return None;
            }
        } else {
            if energy <= SILENCE_POWER {
                return None;
            }
            self.power = energy;
            self.primed = true;
        }
        Some(sample / self.power.sqrt())
    }

    fn fall_back(&mut self) {
        self.reset_weights();
        self.dispersion.clear();
        self.fallback_frames += 1;
    }

    fn step_sample(&mut self, sample: C32) -> C32 {
        let Some(x) = self.normalise(sample) else {
            if !self.silent {
                self.silent = true;
                self.fall_back();
            }
            return C32::default();
        };
        self.silent = false;
        let taps = self.taps;
        self.head = (self.head + taps - 1) % taps;
        self.history[self.head] = x;
        self.history[self.head + taps] = x;
        let regressor = &self.history[self.head..self.head + taps];
        let (y, norm) = self
            .weights
            .iter()
            .zip(regressor)
            .fold((C32::default(), 0.0f32), |(sum, norm), (w, value)| {
                (sum + w.conj() * value, norm + value.norm_sqr())
            });
        let modulus = y.norm_sqr();
        let gain = y.conj() * (self.step * (modulus - 1.0) / (REGRESSOR_FLOOR + norm));
        let mut energy = 0.0f32;
        for (w, value) in self.weights.iter_mut().zip(regressor) {
            *w -= gain * value;
            energy += w.norm_sqr();
        }
        if !(energy.is_finite() && y.is_finite()) {
            self.fall_back();
            return x;
        }
        let spread_in = x.norm_sqr() - 1.0;
        let spread_out = modulus - 1.0;
        self.dispersion
            .add(spread_in * spread_in, spread_out * spread_out, 1);
        y
    }
}

#[cfg(test)]
mod tests {
    use super::super::nlms::tests::{Noise, fm_reference};
    use super::*;

    const FS: f64 = 266_666.67;

    fn multipath(clean: &[C32]) -> Vec<C32> {
        let echo = C32::from_polar(0.5, 1.1);
        (0..clean.len())
            .map(|n| {
                clean[n]
                    + if n >= 5 {
                        clean[n - 5] * echo
                    } else {
                        C32::default()
                    }
            })
            .collect()
    }

    fn dispersion(samples: &[C32]) -> f64 {
        let power =
            samples.iter().map(|x| f64::from(x.norm_sqr())).sum::<f64>() / samples.len() as f64;
        samples
            .iter()
            .map(|x| {
                let spread = f64::from(x.norm_sqr()) / power - 1.0;
                spread * spread
            })
            .sum::<f64>()
            / samples.len() as f64
    }

    #[test]
    fn cma_removes_reference_multipath() {
        let clean = fm_reference(4 * FS as usize, 11);
        let received = multipath(&clean);
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let mut out = vec![C32::default(); received.len()];
        cma.process(&received, &mut out);
        let tail = received.len() - FS as usize / 2;
        let before = dispersion(&received[tail..]);
        let after = dispersion(&out[tail..]);
        let gain = 10.0 * (before / after).log10();
        assert!(gain >= 10.0, "{gain}");
        assert!(cma.gain_db() >= 10.0, "{}", cma.gain_db());
        assert!(cma.locked());
        assert_eq!(cma.fallback_frames(), 0);
    }

    #[test]
    fn a_clean_reference_counts_as_locked() {
        let clean = fm_reference(FS as usize, 14);
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let mut out = vec![C32::default(); clean.len()];
        cma.process(&clean, &mut out);
        assert!(cma.gain_db() < LOCK_GAIN_DB, "{}", cma.gain_db());
        assert!(cma.locked());
        cma.process(&[C32::new(f32::NAN, 0.0)], &mut out[..1]);
        assert!(!cma.locked());
    }

    #[test]
    fn noise_is_never_clean() {
        let mut noise = Noise(21);
        let received: Vec<C32> = (0..FS as usize).map(|_| noise.complex()).collect();
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let mut out = vec![C32::default(); received.len()];
        cma.process(&received, &mut out);
        let dispersion = cma.dispersion.denominator_mean().unwrap();
        assert!(dispersion > 4.0 * CLEAN_DISPERSION, "{dispersion}");
        assert!(!cma.clean());
    }

    #[test]
    fn cma_keeps_zero_delay() {
        let clean = fm_reference(4 * FS as usize, 12);
        let received = multipath(&clean);
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let mut out = vec![C32::default(); received.len()];
        cma.process(&received, &mut out);
        let tail = received.len() - 20_000;
        let correlation = |lag: usize| {
            (tail..received.len() - 16)
                .map(|n| out[n + lag] * clean[n].conj())
                .sum::<C32>()
                .norm()
        };
        let best = (0..16)
            .max_by(|a, b| correlation(*a).total_cmp(&correlation(*b)))
            .unwrap();
        assert_eq!(best, 0);
    }

    #[test]
    fn cma_resets_on_silence() {
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let zeros = vec![C32::default(); 1000];
        let mut out = vec![C32::new(9.0, 9.0); 1000];
        cma.process(&zeros, &mut out);
        assert!(out.iter().all(|y| *y == C32::default()));
        assert_eq!(cma.fallback_frames(), 1);
        let mut noise = Noise(4);
        let mut burst: Vec<C32> = (0..5000).map(|_| noise.complex() * 1e3).collect();
        burst[100] = C32::new(f32::INFINITY, 0.0);
        let mut out = vec![C32::default(); burst.len()];
        cma.process(&burst, &mut out);
        assert!(out.iter().all(|y| y.is_finite()));
        assert_eq!(cma.fallback_frames(), 2);
        cma.process(&zeros, &mut out[..1000]);
        assert!(out.iter().all(|y| y.is_finite()));
    }

    #[test]
    fn a_fallback_drops_the_lock() {
        let received = multipath(&fm_reference(FS as usize, 13));
        let mut cma = ReferenceCma::new(16, 1e-3, FS).unwrap();
        let mut out = vec![C32::default(); received.len()];
        cma.process(&received, &mut out);
        assert!(cma.locked());
        cma.process(&[C32::new(f32::NAN, 0.0)], &mut out[..1]);
        assert!(!cma.locked());
        assert_eq!(cma.gain_db(), 0.0);
        assert_eq!(cma.fallback_frames(), 1);
        assert_eq!(cma.weights()[0], C32::new(1.0, 0.0));
    }

    #[test]
    fn invalid_settings_are_refused() {
        assert!(ReferenceCma::new(0, 1e-3, FS).is_err());
        assert!(ReferenceCma::new(65, 1e-3, FS).is_err());
        assert!(ReferenceCma::new(16, -1.0, FS).is_err());
        assert!(ReferenceCma::new(16, 1e-3, f64::NAN).is_err());
    }
}
