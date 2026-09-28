use num_complex::Complex;

use super::{BeamError, MAX_ORDER, WeightSet, ZERO, common_len, snapshot};
use crate::linalg::LinalgError;

const MODULUS_EMA: f32 = 1e-3;
const DIVERGE_FACTOR: f32 = 1e3;
const MAX_STEP: f32 = 2.0;

pub struct Cma {
    n: usize,
    w: [Complex<f32>; MAX_ORDER],
    seed: [Complex<f32>; MAX_ORDER],
    seed_norm: f32,
    step: f32,
    power: f32,
    fourth: f32,
    resets: u32,
}

impl Cma {
    pub fn new(n: usize, step: f32) -> Result<Self, BeamError> {
        if !(2..=MAX_ORDER).contains(&n) {
            return Err(BeamError::Lanes(n));
        }
        check_step(step)?;
        let mut seed = [ZERO; MAX_ORDER];
        seed[0] = Complex::new(1.0, 0.0);
        Ok(Self {
            n,
            w: seed,
            seed,
            seed_norm: 1.0,
            step,
            power: 0.0,
            fourth: 0.0,
            resets: 0,
        })
    }

    pub fn set_step(&mut self, step: f32) -> Result<(), BeamError> {
        check_step(step)?;
        self.step = step;
        Ok(())
    }

    pub fn seed(&mut self, weights: &WeightSet) -> Result<(), BeamError> {
        if weights.len() != self.n {
            return Err(BeamError::Lanes(weights.len()));
        }
        let norm = weights.norm_sqr().sqrt();
        if !norm.is_finite() {
            return Err(LinalgError::NonFinite.into());
        }
        if norm <= 0.0 {
            return Err(LinalgError::RankDeficient(0).into());
        }
        weights.textbook(&mut self.seed[..self.n]);
        self.seed_norm = norm;
        self.restart();
        Ok(())
    }

    pub fn process(
        &mut self,
        lanes: &[&[Complex<f32>]],
        out: &mut Vec<Complex<f32>>,
    ) -> Result<(), BeamError> {
        let len = common_len(lanes, self.n)?;
        let mut x = [ZERO; MAX_ORDER];
        let mut diverged = false;
        for t in 0..len {
            let energy = snapshot(lanes, t, &mut x);
            match self.sample(&x, energy) {
                Some(y) => out.push(y),
                None => {
                    out.push(ZERO);
                    self.restart();
                    if !diverged {
                        diverged = true;
                        self.resets += 1;
                    }
                }
            }
        }
        if diverged {
            Err(BeamError::Diverged)
        } else {
            Ok(())
        }
    }

    pub fn weights(&self, out: &mut WeightSet) {
        *out = WeightSet::from_textbook(&self.w[..self.n]);
    }

    #[must_use]
    pub const fn resets(&self) -> u32 {
        self.resets
    }

    fn restart(&mut self) {
        self.w = self.seed;
        self.power = 0.0;
        self.fourth = 0.0;
    }

    fn sample(&mut self, x: &[Complex<f32>; MAX_ORDER], energy: f32) -> Option<Complex<f32>> {
        let n = self.n;
        let y: Complex<f32> = self.w[..n]
            .iter()
            .zip(&x[..n])
            .map(|(w, x)| w.conj() * x)
            .sum();
        let level = y.norm_sqr();
        if !(y.is_finite() && energy.is_finite()) {
            return None;
        }
        if self.power > 0.0 {
            self.power += MODULUS_EMA * (level - self.power);
            self.fourth += MODULUS_EMA * (level * level - self.fourth);
        } else {
            self.power = level;
            self.fourth = level * level;
        }
        let denominator = (f32::MIN_POSITIVE + energy) * self.power;
        if denominator > 0.0 {
            let modulus = self.fourth / self.power;
            let error = y * (level - modulus);
            let gain = error.conj() * (self.step / denominator);
            for (w, x) in self.w[..n].iter_mut().zip(&x[..n]) {
                *w -= x * gain;
            }
        }
        let norm = self.w[..n]
            .iter()
            .map(Complex::norm_sqr)
            .sum::<f32>()
            .sqrt();
        if !(norm.is_finite() && norm > 0.0 && norm <= DIVERGE_FACTOR * self.seed_norm) {
            return None;
        }
        let scale = self.seed_norm / norm;
        for w in &mut self.w[..n] {
            *w *= scale;
        }
        Some(y)
    }
}

fn check_step(step: f32) -> Result<(), BeamError> {
    if step.is_finite() && step > 0.0 && step < MAX_STEP {
        Ok(())
    } else {
        Err(BeamError::Setting("step"))
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::BlockSolver;
    use crate::beamform::testing::{FREQ, combine, covariance, kraken, steering, views};
    use crate::manifold::Direction;
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};

    fn modulus_spread(samples: &[Complex<f32>]) -> f32 {
        let levels: Vec<f64> = samples.iter().map(|y| f64::from(y.norm_sqr())).collect();
        let mean = levels.iter().sum::<f64>() / levels.len() as f64;
        let variance = levels.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / levels.len() as f64;
        (variance / (mean * mean)) as f32
    }

    #[test]
    fn cma_recovers_a_constant_modulus_signal() {
        let mut scene = ArrayScene::new(kraken().geometry().clone(), FREQ, 1e6)
            .with_seed(17)
            .with_noise_db(-10.0)
            .with_source(SceneSource::new(
                Direction::horizon(60.0),
                10.0,
                SceneSignal::Fm {
                    offset_hz: 1e4,
                    deviation_hz: 5e3,
                    rate_hz: 1e3,
                },
            ))
            .with_source(SceneSource::new(
                Direction::horizon(150.0),
                10.0,
                SceneSignal::Noise {
                    offset_hz: -2e4,
                    bandwidth_hz: 5e4,
                },
            ));
        let lanes = scene.render(300_000).unwrap();
        let head: Vec<Vec<Complex<f32>>> =
            lanes.iter().map(|lane| lane[..4_000].to_vec()).collect();
        let mut solver = BlockSolver::new(5).unwrap();
        let mut seed = WeightSet::zeros(5);
        solver.mrc(&covariance(&head), None, &mut seed).unwrap();
        let mut cma = Cma::new(5, 0.05).unwrap();
        cma.seed(&seed).unwrap();
        let mut out = Vec::with_capacity(300_000);
        cma.process(&views(&lanes), &mut out).unwrap();
        let tail: Vec<Vec<Complex<f32>>> =
            lanes.iter().map(|lane| lane[50_000..].to_vec()).collect();
        let seeded = modulus_spread(&combine(&tail, seed.as_slice()));
        let adapted = modulus_spread(&out[50_000..]);
        assert!(
            adapted * 10.0 < seeded,
            "modulus spread {seeded} then {adapted}"
        );
        assert_eq!(cma.resets(), 0);
        let mut weights = WeightSet::zeros(5);
        cma.weights(&mut weights);
        assert!((weights.norm_sqr() - seed.norm_sqr()).abs() < 1e-3 * seed.norm_sqr());
        let wanted = weights.response(&steering(&kraken(), 60.0)).norm_sqr();
        let rejected = weights.response(&steering(&kraken(), 150.0)).norm_sqr();
        assert!(wanted > 100.0 * rejected, "{wanted} vs {rejected}");
    }

    #[test]
    fn cma_resets_to_its_seed_on_bad_input() {
        let mut cma = Cma::new(2, 0.05).unwrap();
        let seed = WeightSet::from_textbook(&[Complex::new(0.6, 0.0), Complex::new(0.0, 0.8)]);
        cma.seed(&seed).unwrap();
        let mut lanes = vec![vec![Complex::new(1.0f32, 0.0); 32]; 2];
        lanes[0][7] = Complex::new(f32::NAN, 0.0);
        let mut out = Vec::with_capacity(64);
        assert_eq!(
            cma.process(&views(&lanes), &mut out),
            Err(BeamError::Diverged)
        );
        assert_eq!(cma.resets(), 1);
        assert_eq!(out.len(), 32);
        assert!(out.iter().all(|value| value.is_finite()));
        assert_eq!(
            cma.seed(&WeightSet::zeros(2)),
            Err(BeamError::Linalg(LinalgError::RankDeficient(0)))
        );
        assert_eq!(cma.seed(&WeightSet::zeros(3)), Err(BeamError::Lanes(3)));
        assert_eq!(cma.set_step(-1.0), Err(BeamError::Setting("step")));
        assert!(matches!(Cma::new(1, 0.05), Err(BeamError::Lanes(1))));
    }
}
