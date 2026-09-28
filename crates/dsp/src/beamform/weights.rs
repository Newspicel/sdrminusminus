use num_complex::Complex;

use super::{BeamError, Constraints, MAX_CONSTRAINTS, WeightSet, ZERO};
use crate::linalg::{CMat, Cholesky, Eigen, HermitianEigen, LinalgError, MAX_ORDER};

const SLC_LOADING: f32 = 1e-5;
const NOISE_FLOOR: f32 = 1e-3;
const OUT: usize = MAX_CONSTRAINTS;

pub struct BlockSolver {
    n: usize,
    chol: Cholesky,
    eigen: HermitianEigen,
    values: Eigen,
    small: Cholesky,
    work: CMat,
    scratch: [[Complex<f32>; MAX_ORDER]; MAX_CONSTRAINTS + 1],
}

impl BlockSolver {
    pub fn new(n: usize) -> Result<Self, BeamError> {
        if !(2..=MAX_ORDER).contains(&n) {
            return Err(BeamError::Lanes(n));
        }
        Ok(Self {
            n,
            chol: Cholesky::new(n)?,
            eigen: HermitianEigen::new(n)?,
            values: Eigen::new(),
            small: Cholesky::new(MAX_CONSTRAINTS)?,
            work: CMat::zeros(n)?,
            scratch: [[ZERO; MAX_ORDER]; MAX_CONSTRAINTS + 1],
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.n
    }

    pub fn das(&mut self, steer: &[Complex<f32>], out: &mut WeightSet) -> Result<(), BeamError> {
        self.check_vector(steer)?;
        let energy: f32 = steer.iter().map(Complex::norm_sqr).sum();
        if !energy.is_finite() {
            return Err(LinalgError::NonFinite.into());
        }
        if energy <= 0.0 {
            return Err(LinalgError::RankDeficient(0).into());
        }
        let textbook = &mut self.scratch[OUT][..self.n];
        for (value, a) in textbook.iter_mut().zip(steer) {
            *value = a / energy;
        }
        *out = WeightSet::from_textbook(textbook);
        Ok(())
    }

    pub fn mrc(
        &mut self,
        r: &CMat,
        noise: Option<&[f32]>,
        out: &mut WeightSet,
    ) -> Result<(), BeamError> {
        self.check_matrix(r)?;
        let n = self.n;
        let d = noise_diagonal(n, noise)?;
        self.work.resize(n)?;
        for row in 0..n {
            for col in 0..n {
                let scale = (d[row] * d[col]).sqrt().recip();
                self.work.set(row, col, r.get(row, col) * scale);
            }
        }
        self.eigen.solve(&self.work, &mut self.values)?;
        if self.values.values()[n - 1] <= 0.0 {
            return Err(LinalgError::RankDeficient(0).into());
        }
        let principal = self.values.vector(n - 1);
        let mut steer = [ZERO; MAX_ORDER];
        for ((value, v), weight) in steer.iter_mut().zip(principal).zip(&d) {
            *value = v * weight.sqrt();
        }
        let energy: f32 = steer[..n].iter().map(Complex::norm_sqr).sum();
        let whitened: f32 = steer[..n]
            .iter()
            .zip(&d)
            .map(|(a, weight)| a.norm_sqr() / weight)
            .sum();
        if !(energy.is_finite() && whitened.is_finite() && energy > 0.0 && whitened > 0.0) {
            return Err(LinalgError::NonFinite.into());
        }
        let norm = (n as f32 / energy).sqrt();
        let scale = 1.0 / (norm * whitened);
        let textbook = &mut self.scratch[OUT][..n];
        for ((value, a), weight) in textbook.iter_mut().zip(&steer).zip(&d) {
            *value = a * (scale / weight);
        }
        *out = WeightSet::from_textbook(textbook);
        Ok(())
    }

    pub fn mvdr(
        &mut self,
        r: &CMat,
        steer: &[Complex<f32>],
        loading: f32,
        out: &mut WeightSet,
    ) -> Result<f32, BeamError> {
        self.check_matrix(r)?;
        self.check_vector(steer)?;
        check_loading(loading)?;
        let n = self.n;
        let delta = self.chol.factor_loaded(r, loading)?;
        let y = &mut self.scratch[OUT][..n];
        self.chol.solve_into(steer, y);
        let gain: Complex<f32> = steer.iter().zip(y.iter()).map(|(a, v)| a.conj() * v).sum();
        if !(gain.re.is_finite() && gain.re > 0.0) {
            return Err(LinalgError::NotPositiveDefinite(0).into());
        }
        let scale = gain.re.recip();
        for value in y.iter_mut() {
            *value *= scale;
        }
        *out = WeightSet::from_textbook(y);
        Ok(delta)
    }

    pub fn lcmv(
        &mut self,
        r: Option<&CMat>,
        constraints: &Constraints,
        loading: f32,
        out: &mut WeightSet,
    ) -> Result<f32, BeamError> {
        let n = self.n;
        if constraints.order() != n {
            return Err(BeamError::Lanes(constraints.order()));
        }
        let count = constraints.count();
        if count == 0 {
            return Err(LinalgError::Order(0).into());
        }
        let delta = match r {
            Some(r) => {
                self.check_matrix(r)?;
                check_loading(loading)?;
                Some(self.chol.factor_loaded(r, loading)?)
            }
            None => None,
        };
        for k in 0..count {
            let column = &mut self.scratch[k][..n];
            match delta {
                Some(_) => self.chol.solve_into(constraints.vector(k), column),
                None => column.copy_from_slice(constraints.vector(k)),
            }
        }
        self.work.resize(count)?;
        for row in 0..count {
            for col in 0..count {
                let value = dot(constraints.vector(row), &self.scratch[col][..n]);
                self.work.set(row, col, value);
            }
        }
        self.small.factor_cmat(&self.work)?;
        let mut g = [ZERO; MAX_CONSTRAINTS];
        for (k, value) in g.iter_mut().enumerate().take(count) {
            *value = constraints.response(k).conj();
        }
        self.small.solve(&mut g[..count]);
        let (columns, rest) = self.scratch.split_at_mut(OUT);
        let textbook = &mut rest[0][..n];
        textbook.fill(ZERO);
        for (column, &weight) in columns.iter().zip(&g).take(count) {
            for (value, y) in textbook.iter_mut().zip(&column[..n]) {
                *value += y * weight;
            }
        }
        if !textbook.iter().all(|value| value.is_finite()) {
            return Err(LinalgError::NonFinite.into());
        }
        *out = WeightSet::from_textbook(textbook);
        Ok(delta.unwrap_or(0.0))
    }

    pub fn slc(
        &mut self,
        r: &CMat,
        primary: usize,
        references: &[usize],
        out: &mut WeightSet,
    ) -> Result<f32, BeamError> {
        self.check_matrix(r)?;
        let n = self.n;
        check_references(n, primary, references)?;
        let m = references.len();
        self.work.resize(m)?;
        for (row, &from_row) in references.iter().enumerate() {
            for (col, &from_col) in references.iter().enumerate() {
                self.work.set(row, col, r.get(from_row, from_col));
            }
        }
        let delta = self.chol.factor_loaded(&self.work, SLC_LOADING)?;
        let z = &mut self.scratch[OUT][..m];
        for (value, &lane) in z.iter_mut().zip(references) {
            *value = r.get(lane, primary);
        }
        self.chol.solve(z);
        if !z.iter().all(|value| value.is_finite()) {
            return Err(LinalgError::NonFinite.into());
        }
        *out = WeightSet::unit(n, primary);
        let weights = out.as_mut_slice();
        for (&lane, value) in references.iter().zip(z.iter()) {
            weights[lane] = -value.conj();
        }
        Ok(delta)
    }

    fn check_matrix(&self, r: &CMat) -> Result<(), BeamError> {
        if r.order() != self.n {
            return Err(BeamError::Lanes(r.order()));
        }
        Ok(())
    }

    fn check_vector(&self, vector: &[Complex<f32>]) -> Result<(), BeamError> {
        if vector.len() != self.n {
            return Err(BeamError::Lanes(vector.len()));
        }
        Ok(())
    }
}

fn noise_diagonal(n: usize, noise: Option<&[f32]>) -> Result<[f32; MAX_ORDER], BeamError> {
    let mut d = [1.0f32; MAX_ORDER];
    let Some(noise) = noise else {
        return Ok(d);
    };
    if noise.len() != n {
        return Err(BeamError::Lanes(noise.len()));
    }
    if !noise.iter().all(|sigma| sigma.is_finite() && *sigma >= 0.0) {
        return Err(LinalgError::NonFinite.into());
    }
    let mean = noise.iter().sum::<f32>() / n as f32;
    if mean <= 0.0 {
        return Err(LinalgError::RankDeficient(0).into());
    }
    for (slot, &sigma) in d.iter_mut().zip(noise) {
        *slot = sigma.max(NOISE_FLOOR * mean);
    }
    Ok(d)
}

fn check_loading(loading: f32) -> Result<(), BeamError> {
    if loading.is_finite() && loading >= 0.0 {
        Ok(())
    } else {
        Err(BeamError::Setting("loading"))
    }
}

pub(super) fn check_references(
    n: usize,
    primary: usize,
    references: &[usize],
) -> Result<(), BeamError> {
    let fits = !references.is_empty()
        && references.len() < n
        && primary < n
        && references.iter().enumerate().all(|(index, &lane)| {
            lane < n && lane != primary && !references[..index].contains(&lane)
        });
    if fits {
        Ok(())
    } else {
        Err(BeamError::Lanes(references.len() + 1))
    }
}

fn dot(a: &[Complex<f32>], b: &[Complex<f32>]) -> Complex<f32> {
    a.iter().zip(b).map(|(x, y)| x.conj() * y).sum()
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::testing::{FREQ, combine, covariance, db, kraken, power, steering};
    use crate::manifold::Direction;
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};
    use crate::testutil::XorShift32;

    fn noise(len: usize, power: f32, seed: u32) -> Vec<Complex<f32>> {
        let mut rng = XorShift32(seed);
        let scale = (1.5 * power).sqrt();
        (0..len)
            .map(|_| Complex::new(rng.next_f32() * scale, rng.next_f32() * scale))
            .collect()
    }

    fn tone(len: usize, cycles: f32, amplitude: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|t| {
                let angle = std::f32::consts::TAU * cycles * t as f32 / len as f32;
                Complex::from_polar(amplitude, angle)
            })
            .collect()
    }

    fn mix(parts: &[(&[Complex<f32>], Complex<f32>)]) -> Vec<Complex<f32>> {
        (0..parts[0].0.len())
            .map(|t| parts.iter().map(|(signal, gain)| signal[t] * gain).sum())
            .collect()
    }

    fn residual(
        lanes: &[Vec<Complex<f32>>],
        weights: &[Complex<f32>],
        wanted: &[Complex<f32>],
    ) -> f32 {
        let out = combine(lanes, weights);
        let error: Vec<Complex<f32>> = out.iter().zip(wanted).map(|(y, s)| y - s).collect();
        power(&error) / power(wanted)
    }

    fn old_cancel(r: &CMat, references: &[usize]) -> Vec<Complex<f32>> {
        let m = references.len();
        let mut aux = CMat::zeros(m).unwrap();
        for (row, &a) in references.iter().enumerate() {
            for (col, &b) in references.iter().enumerate() {
                aux.set(row, col, r.get(a, b));
            }
        }
        let mut chol = Cholesky::new(m).unwrap();
        chol.factor_cmat(&aux).unwrap();
        let mut s: Vec<Complex<f32>> = references.iter().map(|&lane| r.get(0, lane)).collect();
        chol.solve(&mut s);
        let mut weights = vec![Complex::new(1.0, 0.0)];
        weights.extend(s.iter().map(|value| -value));
        weights
    }

    #[test]
    fn slc_nulls_two_interferers_with_three_lanes() {
        let len = 20_000;
        let wanted = tone(len, 263.0, 1.0);
        let first = noise(len, 1.0, 11);
        let second = noise(len, 1.0, 23);
        let quiet: Vec<Vec<Complex<f32>>> =
            (0..3).map(|lane| noise(len, 1e-3, 31 + lane)).collect();
        let gain = |magnitude: f32, phase: f32| Complex::from_polar(magnitude, phase);
        let one = Complex::new(1.0, 0.0);
        let lanes = vec![
            mix(&[
                (&wanted, one),
                (&first, gain(1.0, 0.3)),
                (&second, gain(1.0, -1.2)),
                (&quiet[0], one),
            ]),
            mix(&[
                (&first, gain(0.9, 2.0)),
                (&second, gain(0.4, -0.4)),
                (&quiet[1], one),
            ]),
            mix(&[
                (&first, gain(0.5, -2.5)),
                (&second, gain(1.1, 1.1)),
                (&quiet[2], one),
            ]),
        ];
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(3).unwrap();
        let mut weights = WeightSet::zeros(3);
        solver.slc(&r, 0, &[1, 2], &mut weights).unwrap();
        let left = residual(&lanes, weights.as_slice(), &wanted);
        assert!(left < 0.01, "residual {left}");
        let old = residual(&lanes, &old_cancel(&r, &[1, 2]), &wanted);
        assert!(old > 1.0, "old formula residual {old}");
    }

    #[test]
    fn slc_matches_the_two_lane_result() {
        let len = 8_192;
        let wanted = tone(len, 11.0, 0.3);
        let interferer = tone(len, 53.0, 3.0);
        let leak = Complex::from_polar(0.8, 2.2);
        let one = Complex::new(1.0, 0.0);
        let lanes = vec![
            mix(&[(&wanted, one), (&interferer, one)]),
            mix(&[(&interferer, leak)]),
        ];
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(2).unwrap();
        let mut weights = WeightSet::zeros(2);
        solver.slc(&r, 0, &[1], &mut weights).unwrap();
        let old = old_cancel(&r, &[1]);
        for (new, old) in weights.as_slice().iter().zip(&old) {
            assert!((new - old).norm() < 1e-4, "{new} vs {old}");
        }
        assert!(residual(&lanes, weights.as_slice(), &wanted) < 1e-3);
    }

    #[test]
    fn slc_with_a_reference_subset_ignores_other_lanes() {
        let len = 4_096;
        let lanes: Vec<Vec<Complex<f32>>> = (0..4).map(|lane| noise(len, 1.0, 5 + lane)).collect();
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(4).unwrap();
        let mut weights = WeightSet::zeros(4);
        solver.slc(&r, 1, &[3], &mut weights).unwrap();
        let w = weights.as_slice();
        assert_eq!(w[1], Complex::new(1.0, 0.0));
        assert_eq!(w[0], Complex::new(0.0, 0.0));
        assert_eq!(w[2], Complex::new(0.0, 0.0));
        assert!(w[3].norm() > 0.0);
        for bad in [&[1usize][..], &[], &[3, 3], &[4], &[0, 2, 3, 1]] {
            assert!(matches!(
                solver.slc(&r, 1, bad, &mut weights),
                Err(BeamError::Lanes(_))
            ));
        }
    }

    #[test]
    fn das_has_unit_gain_toward_the_steer() {
        let manifold = kraken();
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        for azimuth in [0.0, 37.0, 137.0, 251.5, 359.0] {
            let a = steering(&manifold, azimuth);
            solver.das(&a, &mut weights).unwrap();
            let gain = weights.response(&a).norm();
            assert!((gain - 1.0).abs() < 1e-5, "{azimuth}: {gain}");
        }
        assert_eq!(
            solver.das(&[Complex::new(1.0, 0.0); 4], &mut weights),
            Err(BeamError::Lanes(4))
        );
        assert_eq!(
            solver.das(&[Complex::new(0.0, 0.0); 5], &mut weights),
            Err(BeamError::Linalg(LinalgError::RankDeficient(0)))
        );
    }

    fn scene_lanes(sources: &[(f64, f32)], noise_db: &[f32], len: usize) -> Vec<Vec<Complex<f32>>> {
        let mut scene = ArrayScene::new(kraken().geometry().clone(), FREQ, 1e6).with_seed(9);
        for (index, &(azimuth, power_db)) in sources.iter().enumerate() {
            let signal = if index == 0 {
                SceneSignal::Tone { offset_hz: 1.5e4 }
            } else {
                SceneSignal::Noise {
                    offset_hz: 0.0,
                    bandwidth_hz: 5e4,
                }
            };
            scene = scene.with_source(SceneSource::new(
                Direction::horizon(azimuth),
                power_db,
                signal,
            ));
        }
        scene.noise_db = noise_db.to_vec();
        scene.render(len).unwrap()
    }

    fn output_snr(weights: &WeightSet, a: &[Complex<f32>], signal: f32, noise: &[f32]) -> f32 {
        let wanted = weights.response(a).norm_sqr() * signal;
        let floor: f32 = weights
            .as_slice()
            .iter()
            .zip(noise)
            .map(|(w, sigma)| w.norm_sqr() * sigma)
            .sum();
        wanted / floor
    }

    #[test]
    fn mvdr_nulls_an_interferer_and_keeps_the_steer() {
        let manifold = kraken();
        let lanes = scene_lanes(&[(137.0, 0.0), (177.0, 30.0)], &[0.0; 5], 40_000);
        let r = covariance(&lanes);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        let a = steering(&manifold, 137.0);
        let delta = solver.mvdr(&r, &a, 1e-3, &mut weights).unwrap();
        assert!(delta > 0.0);
        let gain = weights.response(&a).norm();
        assert!((gain - 1.0).abs() < 0.05, "steer gain {gain}");
        let interferer = steering(&manifold, 177.0);
        let leaked = weights.response(&interferer).norm_sqr() * 1000.0;
        let noise = weights.norm_sqr();
        let sinr_out = gain * gain / (leaked + noise);
        let sinr_in = 1.0 / 1001.0;
        let improvement = db(sinr_out / sinr_in);
        assert!(improvement > 20.0, "improvement {improvement} dB");
    }

    #[test]
    fn lcmv_places_fixed_nulls_deeper_than_40_db() {
        let manifold = kraken();
        let mut constraints = Constraints::new(5);
        constraints
            .push(&steering(&manifold, 30.0), Complex::new(1.0, 0.0))
            .unwrap();
        for null in [70.0, 190.0] {
            constraints
                .push(&steering(&manifold, null), Complex::new(0.0, 0.0))
                .unwrap();
        }
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        let delta = solver.lcmv(None, &constraints, 0.1, &mut weights).unwrap();
        assert_eq!(delta, 0.0);
        let main = weights.response(&steering(&manifold, 30.0)).norm();
        assert!((main - 1.0).abs() < 1e-4, "main {main}");
        for null in [70.0, 190.0] {
            let depth = 20.0 * (weights.response(&steering(&manifold, null)).norm() / main).log10();
            assert!(depth < -40.0, "{null}: {depth} dB");
        }
    }

    #[test]
    fn lcmv_with_only_the_steer_matches_mvdr_and_holds_a_null_under_interference() {
        let manifold = kraken();
        let lanes = scene_lanes(&[(137.0, 0.0), (177.0, 30.0)], &[0.0; 5], 20_000);
        let r = covariance(&lanes);
        let steer = steering(&manifold, 137.0);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut mvdr = WeightSet::zeros(5);
        let mut lcmv = WeightSet::zeros(5);
        solver.mvdr(&r, &steer, 1e-3, &mut mvdr).unwrap();
        let mut constraints = Constraints::new(5);
        constraints.push(&steer, Complex::new(1.0, 0.0)).unwrap();
        solver
            .lcmv(Some(&r), &constraints, 1e-3, &mut lcmv)
            .unwrap();
        for (a, b) in lcmv.as_slice().iter().zip(mvdr.as_slice()) {
            assert!((a - b).norm() < 1e-4 * b.norm().max(1.0), "{a} vs {b}");
        }
        let fixed = steering(&manifold, 300.0);
        constraints.push(&fixed, Complex::new(0.0, 0.0)).unwrap();
        solver
            .lcmv(Some(&r), &constraints, 1e-3, &mut lcmv)
            .unwrap();
        assert!((lcmv.response(&steer) - Complex::new(1.0, 0.0)).norm() < 1e-3);
        assert!(lcmv.response(&fixed).norm() < 1e-3);
        let leak = db(lcmv.response(&steering(&manifold, 177.0)).norm_sqr());
        assert!(leak < -30.0, "interferer {leak} dB");
    }

    #[test]
    fn lcmv_refuses_a_null_in_the_main_lobe() {
        let manifold = kraken();
        let mut constraints = Constraints::new(5);
        constraints
            .push(&steering(&manifold, 80.0), Complex::new(1.0, 0.0))
            .unwrap();
        assert_eq!(
            constraints.push(&steering(&manifold, 83.0), Complex::new(0.0, 0.0)),
            Err(BeamError::NullInMainLobe(1))
        );
        assert_eq!(constraints.count(), 1);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        assert_eq!(
            solver.lcmv(None, &Constraints::new(5), 0.1, &mut weights),
            Err(BeamError::Linalg(LinalgError::Order(0)))
        );
        for _ in 0..2 {
            constraints
                .push(&steering(&manifold, 200.0), Complex::new(0.0, 0.0))
                .unwrap();
        }
        let before = weights.clone();
        assert!(matches!(
            solver.lcmv(None, &constraints, 0.0, &mut weights),
            Err(BeamError::Linalg(LinalgError::NotPositiveDefinite(_)))
        ));
        assert_eq!(weights, before);
    }

    #[test]
    fn mrc_with_unequal_noise_beats_the_principal_eigenvector() {
        let manifold = kraken();
        let noise_db = [0.0, 10.0, 0.0, 10.0, 10.0];
        let noise: Vec<f32> = noise_db
            .iter()
            .map(|db: &f32| 10f32.powf(db / 10.0))
            .collect();
        let lanes = scene_lanes(&[(137.0, 10.0)], &noise_db, 40_000);
        let r = covariance(&lanes);
        let a = steering(&manifold, 137.0);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut measured = WeightSet::zeros(5);
        let mut white = WeightSet::zeros(5);
        solver.mrc(&r, Some(&noise), &mut measured).unwrap();
        solver.mrc(&r, None, &mut white).unwrap();
        let best = db(noise.iter().map(|sigma| 10.0 / sigma).sum());
        let measured_snr = db(output_snr(&measured, &a, 10.0, &noise));
        let white_snr = db(output_snr(&white, &a, 10.0, &noise));
        assert!(
            measured_snr >= white_snr + 1.0,
            "{measured_snr} vs {white_snr}"
        );
        assert!(
            (measured_snr - best).abs() < 0.3,
            "{measured_snr} vs {best}"
        );
        assert!((measured.response(&a).norm() - 1.0).abs() < 0.05);
    }

    #[test]
    fn mrc_phase_stays_continuous_between_updates() {
        let manifold = kraken();
        let lanes = scene_lanes(&[(52.0, 5.0)], &[0.0; 5], 8_000);
        let first: Vec<Vec<Complex<f32>>> =
            lanes.iter().map(|lane| lane[..4_000].to_vec()).collect();
        let second: Vec<Vec<Complex<f32>>> =
            lanes.iter().map(|lane| lane[4_000..].to_vec()).collect();
        let a = steering(&manifold, 52.0);
        let mut solver = BlockSolver::new(5).unwrap();
        let mut before = WeightSet::zeros(5);
        let mut after = WeightSet::zeros(5);
        solver.mrc(&covariance(&first), None, &mut before).unwrap();
        solver.mrc(&covariance(&second), None, &mut after).unwrap();
        let turn = Complex::from_polar(1.0f32, 1.9);
        for value in after.as_mut_slice() {
            *value *= turn;
        }
        after.align_phase_to(&before);
        let step = (after.response(&a) / before.response(&a))
            .arg()
            .to_degrees()
            .abs();
        assert!(step < 5.0, "phase step {step} deg");
    }

    #[test]
    fn solver_refuses_wrong_shapes_and_settings() {
        assert!(matches!(BlockSolver::new(1), Err(BeamError::Lanes(1))));
        assert!(matches!(BlockSolver::new(17), Err(BeamError::Lanes(17))));
        let mut solver = BlockSolver::new(3).unwrap();
        let mut weights = WeightSet::zeros(3);
        let r = CMat::identity(4).unwrap();
        assert_eq!(solver.mrc(&r, None, &mut weights), Err(BeamError::Lanes(4)));
        let r = CMat::identity(3).unwrap();
        let a = [Complex::new(1.0f32, 0.0); 3];
        assert_eq!(
            solver.mvdr(&r, &a, f32::NAN, &mut weights),
            Err(BeamError::Setting("loading"))
        );
        assert_eq!(
            solver.mrc(&r, Some(&[1.0, 2.0]), &mut weights),
            Err(BeamError::Lanes(2))
        );
        let zero = CMat::zeros(3).unwrap();
        assert!(solver.mvdr(&zero, &a, 0.1, &mut weights).is_err());
        assert_eq!(
            solver.mrc(&zero, None, &mut weights),
            Err(BeamError::Linalg(LinalgError::RankDeficient(0)))
        );
        assert!(solver.slc(&zero, 0, &[1, 2], &mut weights).is_err());
        let mut constraints = Constraints::new(3);
        constraints.push(&a, Complex::new(1.0, 0.0)).unwrap();
        assert!(
            solver
                .lcmv(Some(&zero), &constraints, 0.1, &mut weights)
                .is_err()
        );
    }
}
