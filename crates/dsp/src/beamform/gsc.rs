use num_complex::Complex;

use super::{BeamError, Constraints, MAX_CONSTRAINTS, WeightSet, ZERO, common_len, snapshot};
use crate::linalg::{LinalgError, MAX_ORDER, Qr};

const POWER_EMA: f32 = 1e-3;
const REGULARISE: f32 = 1e-6;
const DIVERGE_NORM: f32 = 1e3;
const DEFAULT_STEP: f32 = 0.05;
const MAX_STEP: f32 = 2.0;

pub struct Gsc {
    n: usize,
    free: usize,
    wq: [Complex<f32>; MAX_ORDER],
    blocking: [Complex<f32>; MAX_ORDER * MAX_ORDER],
    h: [Complex<f32>; MAX_ORDER],
    step: f32,
    power: f32,
    resets: u32,
}

impl Gsc {
    pub fn new(n: usize) -> Result<Self, BeamError> {
        if !(2..=MAX_ORDER).contains(&n) {
            return Err(BeamError::Lanes(n));
        }
        let free = n - 1;
        let mut gsc = Self {
            n,
            free,
            wq: [ZERO; MAX_ORDER],
            blocking: [ZERO; MAX_ORDER * MAX_ORDER],
            h: [ZERO; MAX_ORDER],
            step: DEFAULT_STEP,
            power: 0.0,
            resets: 0,
        };
        gsc.wq[0] = Complex::new(1.0, 0.0);
        for col in 0..free {
            gsc.blocking[(col + 1) * free + col] = Complex::new(1.0, 0.0);
        }
        Ok(gsc)
    }

    pub fn set_constraints(
        &mut self,
        constraints: &Constraints,
        qr: &mut Qr,
    ) -> Result<(), BeamError> {
        let n = self.n;
        if constraints.order() != n {
            return Err(BeamError::Lanes(constraints.order()));
        }
        let count = constraints.count();
        if count == 0 {
            return Err(LinalgError::Order(0).into());
        }
        let mut c = [ZERO; MAX_ORDER * MAX_CONSTRAINTS];
        for col in 0..count {
            for (row, value) in constraints.vector(col).iter().enumerate() {
                c[row * count + col] = *value;
            }
        }
        *qr = Qr::new(n, count)?;
        qr.factor(&c[..n * count])?;
        let quiescent = quiescent(qr, constraints, n)?;
        let mut next = [ZERO; MAX_ORDER * MAX_ORDER];
        let width = qr.null_basis(&mut next[..n * (n - count)])?;
        let adaptive = self.adaptive();
        self.wq = quiescent;
        self.blocking = next;
        self.free = width;
        self.h = [ZERO; MAX_ORDER];
        for (j, value) in self.h.iter_mut().enumerate().take(width) {
            *value = (0..n)
                .map(|row| self.blocking[row * width + j].conj() * adaptive[row])
                .sum();
        }
        if !self.h.iter().all(|value| value.is_finite()) {
            self.h = [ZERO; MAX_ORDER];
        }
        Ok(())
    }

    pub fn set_step(&mut self, step: f32) -> Result<(), BeamError> {
        if !(step.is_finite() && step > 0.0 && step < MAX_STEP) {
            return Err(BeamError::Setting("step"));
        }
        self.step = step;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.h = [ZERO; MAX_ORDER];
        self.power = 0.0;
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
                    self.reset();
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
        let n = self.n;
        let adaptive = self.adaptive();
        let mut textbook = [ZERO; MAX_ORDER];
        for ((value, quiescent), adapted) in textbook.iter_mut().zip(&self.wq).zip(&adaptive) {
            *value = quiescent - adapted;
        }
        *out = WeightSet::from_textbook(&textbook[..n]);
    }

    #[must_use]
    pub const fn resets(&self) -> u32 {
        self.resets
    }

    fn adaptive(&self) -> [Complex<f32>; MAX_ORDER] {
        let free = self.free;
        let mut out = [ZERO; MAX_ORDER];
        for (row, value) in out.iter_mut().enumerate().take(self.n) {
            *value = self.blocking[row * free..(row + 1) * free]
                .iter()
                .zip(&self.h)
                .map(|(b, h)| b * h)
                .sum();
        }
        out
    }

    fn sample(&mut self, x: &[Complex<f32>; MAX_ORDER], energy: f32) -> Option<Complex<f32>> {
        let n = self.n;
        let free = self.free;
        let mut u = [ZERO; MAX_ORDER];
        let mut d = ZERO;
        for (row, &value) in x.iter().enumerate().take(n) {
            d += self.wq[row].conj() * value;
            let basis = &self.blocking[row * free..(row + 1) * free];
            for (slot, b) in u.iter_mut().zip(basis) {
                *slot += b.conj() * value;
            }
        }
        let estimate: Complex<f32> = self
            .h
            .iter()
            .zip(&u[..free])
            .map(|(h, u)| h.conj() * u)
            .sum();
        let y = d - estimate;
        self.power += POWER_EMA * (energy / n as f32 - self.power);
        let u_energy: f32 = u[..free].iter().map(Complex::norm_sqr).sum();
        let denominator = REGULARISE * self.power + u_energy;
        if !(y.is_finite() && denominator.is_finite()) {
            return None;
        }
        if denominator > 0.0 {
            let gain = y.conj() * (self.step / denominator);
            let mut norm = 0.0f32;
            for (h, u) in self.h.iter_mut().zip(&u[..free]) {
                *h += u * gain;
                norm += h.norm_sqr();
            }
            if !(norm.is_finite() && norm <= DIVERGE_NORM * DIVERGE_NORM) {
                return None;
            }
        }
        Some(y)
    }
}

fn quiescent(
    qr: &Qr,
    constraints: &Constraints,
    n: usize,
) -> Result<[Complex<f32>; MAX_ORDER], BeamError> {
    let count = constraints.count();
    let r = qr.r();
    let mut z = [ZERO; MAX_CONSTRAINTS];
    for i in 0..count {
        let known: Complex<f32> = (0..i).map(|k| r.get(k, i).conj() * z[k]).sum();
        let pivot = r.get(i, i).conj();
        if pivot.norm() == 0.0 {
            return Err(LinalgError::RankDeficient(i).into());
        }
        z[i] = (constraints.response(i).conj() - known) / pivot;
    }
    let q = qr.q();
    let mut wq = [ZERO; MAX_ORDER];
    for (row, value) in wq.iter_mut().enumerate().take(n) {
        *value = (0..count).map(|i| q.get(row, i) * z[i]).sum();
    }
    if !wq.iter().all(|value| value.is_finite()) {
        return Err(LinalgError::NonFinite.into());
    }
    Ok(wq)
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::BlockSolver;
    use crate::beamform::testing::{FREQ, covariance, db, kraken, power, steering, views};
    use crate::manifold::Direction;
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};

    fn steer_only(azimuth: f64) -> Constraints {
        let mut constraints = Constraints::new(5);
        constraints
            .push(&steering(&kraken(), azimuth), Complex::new(1.0, 0.0))
            .unwrap();
        constraints
    }

    #[test]
    fn gsc_quiescent_weights_meet_the_constraints() {
        let manifold = kraken();
        let mut constraints = steer_only(40.0);
        constraints
            .push(&steering(&manifold, 170.0), Complex::new(0.0, 0.0))
            .unwrap();
        let mut gsc = Gsc::new(5).unwrap();
        let mut qr = Qr::new(5, 1).unwrap();
        gsc.set_constraints(&constraints, &mut qr).unwrap();
        let mut weights = WeightSet::zeros(5);
        gsc.weights(&mut weights);
        assert!(
            (weights.response(&steering(&manifold, 40.0)) - Complex::new(1.0, 0.0)).norm() < 1e-5
        );
        assert!(weights.response(&steering(&manifold, 170.0)).norm() < 1e-5);
        assert_eq!(gsc.set_step(0.0), Err(BeamError::Setting("step")));
        assert_eq!(
            gsc.set_constraints(&Constraints::new(5), &mut qr),
            Err(BeamError::Linalg(LinalgError::Order(0)))
        );
    }

    fn wanted_and_interferer(len: usize) -> Vec<Vec<Complex<f32>>> {
        let mut scene = ArrayScene::new(kraken().geometry().clone(), FREQ, 1e6)
            .with_seed(21)
            .with_noise_db(0.0)
            .with_source(SceneSource::new(
                Direction::horizon(137.0),
                0.0,
                SceneSignal::Tone { offset_hz: 2e4 },
            ))
            .with_source(SceneSource::new(
                Direction::horizon(210.0),
                20.0,
                SceneSignal::Noise {
                    offset_hz: -1e4,
                    bandwidth_hz: 5e4,
                },
            ));
        scene.render(len).unwrap()
    }

    #[test]
    fn gsc_keeps_its_null_across_a_steer_change() {
        let manifold = kraken();
        let lanes = wanted_and_interferer(20_000);
        let mut qr = Qr::new(5, 1).unwrap();
        let mut kept = Gsc::new(5).unwrap();
        kept.set_constraints(&steer_only(137.0), &mut qr).unwrap();
        let mut out = Vec::with_capacity(20_000);
        kept.process(&views(&lanes), &mut out).unwrap();
        kept.set_constraints(&steer_only(140.0), &mut qr).unwrap();
        let mut fresh = Gsc::new(5).unwrap();
        fresh.set_constraints(&steer_only(140.0), &mut qr).unwrap();
        let interferer = steering(&manifold, 210.0);
        let steer = steering(&manifold, 140.0);
        let mut weights = WeightSet::zeros(5);
        kept.weights(&mut weights);
        assert!((weights.response(&steer) - Complex::new(1.0, 0.0)).norm() < 1e-4);
        let kept_leak = weights.response(&interferer).norm_sqr();
        fresh.weights(&mut weights);
        let fresh_leak = weights.response(&interferer).norm_sqr();
        assert!(
            db(fresh_leak / kept_leak) > 10.0,
            "kept {kept_leak} vs fresh {fresh_leak}"
        );
    }

    #[test]
    fn gsc_converges_to_the_mvdr_output_power() {
        let manifold = kraken();
        let lanes = wanted_and_interferer(25_000);
        let mut gsc = Gsc::new(5).unwrap();
        let mut qr = Qr::new(5, 1).unwrap();
        gsc.set_constraints(&steer_only(137.0), &mut qr).unwrap();
        let mut out = Vec::with_capacity(25_000);
        gsc.process(&views(&lanes), &mut out).unwrap();
        let tail: Vec<Vec<Complex<f32>>> =
            lanes.iter().map(|lane| lane[20_000..].to_vec()).collect();
        let r = covariance(&tail);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut mvdr = WeightSet::zeros(5);
        solver
            .mvdr(&r, &steering(&manifold, 137.0), 1e-6, &mut mvdr)
            .unwrap();
        let mut textbook = [Complex::new(0.0f32, 0.0); 5];
        mvdr.textbook(&mut textbook);
        let optimum = r.quad(&textbook);
        let reached = power(&out[20_000..]);
        let gap = db(reached / optimum);
        assert!(gap.abs() < 1.0, "gsc {reached} vs mvdr {optimum}: {gap} dB");
        assert!(db(power(&lanes[0][20_000..]) / reached) > 10.0);
        assert_eq!(gsc.resets(), 0);
    }

    #[test]
    fn gsc_divergence_resets_once_and_keeps_running() {
        let mut gsc = Gsc::new(3).unwrap();
        let mut lanes = vec![vec![Complex::new(1.0f32, 0.5); 64]; 3];
        lanes[1][10] = Complex::new(f32::NAN, 0.0);
        lanes[2][11] = Complex::new(f32::INFINITY, 0.0);
        let mut out = Vec::with_capacity(128);
        assert_eq!(
            gsc.process(&views(&lanes), &mut out),
            Err(BeamError::Diverged)
        );
        assert_eq!(gsc.resets(), 1);
        assert_eq!(out.len(), 64);
        assert!(out.iter().all(|value| value.is_finite()));
        let clean = vec![vec![Complex::new(1.0f32, 0.5); 64]; 3];
        assert_eq!(gsc.process(&views(&clean), &mut out), Ok(()));
        assert_eq!(gsc.resets(), 1);
        assert_eq!(
            gsc.process(&views(&clean[..2]), &mut out),
            Err(BeamError::Lanes(2))
        );
    }
}
