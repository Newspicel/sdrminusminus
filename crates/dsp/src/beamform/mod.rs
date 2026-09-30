mod cma;
mod gsc;
mod metrics;
mod ramp;
mod tdl;
mod weights;

use num_complex::Complex;

use crate::linalg::{LinalgError, MAX_ORDER};

pub use cma::Cma;
pub use gsc::Gsc;
pub use metrics::{BeamMetrics, LaneNoise, NOISE_FRAME, beam_metrics};
pub use ramp::{WeightRamp, pattern};
pub use tdl::{Adaptation, TDL_MAX_CMAC_PER_S, TDL_MAX_TAPS, TdlCanceller, tdl_cmac_per_sample};
pub use weights::BlockSolver;

pub const MAX_CONSTRAINTS: usize = 4;
pub const NULL_OVERLAP_LIMIT: f32 = 0.7;

const ZERO: Complex<f32> = Complex::new(0.0, 0.0);

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum BeamError {
    #[error(transparent)]
    Linalg(#[from] LinalgError),
    #[error("null {0} sits in the main lobe")]
    NullInMainLobe(usize),
    #[error("at most {0} constraints fit this array")]
    TooManyConstraints(usize),
    #[error("weights diverged")]
    Diverged,
    #[error("{0} lanes do not fit")]
    Lanes(usize),
    #[error("lanes differ in length")]
    LaneLength,
    #[error("{0} out of range")]
    Setting(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct WeightSet {
    n: usize,
    w: [Complex<f32>; MAX_ORDER],
}

impl WeightSet {
    #[must_use]
    pub const fn zeros(n: usize) -> Self {
        Self {
            n: if n < MAX_ORDER { n } else { MAX_ORDER },
            w: [ZERO; MAX_ORDER],
        }
    }

    #[must_use]
    pub fn unit(n: usize, lane: usize) -> Self {
        let mut set = Self::zeros(n);
        if lane < set.n {
            set.w[lane] = Complex::new(1.0, 0.0);
        }
        set
    }

    #[must_use]
    pub fn from_textbook(textbook: &[Complex<f32>]) -> Self {
        let mut set = Self::zeros(textbook.len());
        for (applied, value) in set.w.iter_mut().zip(textbook) {
            *applied = value.conj();
        }
        set
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Complex<f32>] {
        &self.w[..self.n]
    }

    pub fn as_mut_slice(&mut self) -> &mut [Complex<f32>] {
        &mut self.w[..self.n]
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn textbook(&self, out: &mut [Complex<f32>]) {
        for (value, applied) in out.iter_mut().zip(self.as_slice()) {
            *value = applied.conj();
        }
    }

    #[must_use]
    pub fn response(&self, vector: &[Complex<f32>]) -> Complex<f32> {
        self.as_slice().iter().zip(vector).map(|(w, a)| w * a).sum()
    }

    pub fn align_phase_to(&mut self, previous: &Self) {
        if previous.n != self.n {
            return;
        }
        let overlap: Complex<f32> = self
            .as_slice()
            .iter()
            .zip(previous.as_slice())
            .map(|(new, old)| new.conj() * old)
            .sum();
        let magnitude = overlap.norm();
        if !(magnitude.is_finite() && magnitude > f32::MIN_POSITIVE) {
            return;
        }
        let turn = overlap / magnitude;
        for value in self.as_mut_slice() {
            *value *= turn;
        }
    }

    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.as_slice().iter().all(|value| value.is_finite())
    }

    #[must_use]
    pub fn norm_sqr(&self) -> f32 {
        self.as_slice().iter().map(Complex::norm_sqr).sum()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Constraints {
    n: usize,
    count: usize,
    vectors: [[Complex<f32>; MAX_ORDER]; MAX_CONSTRAINTS],
    response: [Complex<f32>; MAX_CONSTRAINTS],
}

impl Constraints {
    #[must_use]
    pub const fn new(n: usize) -> Self {
        Self {
            n: if n < MAX_ORDER { n } else { MAX_ORDER },
            count: 0,
            vectors: [[ZERO; MAX_ORDER]; MAX_CONSTRAINTS],
            response: [ZERO; MAX_CONSTRAINTS],
        }
    }

    pub fn push(
        &mut self,
        steering: &[Complex<f32>],
        response: Complex<f32>,
    ) -> Result<(), BeamError> {
        if steering.len() != self.n {
            return Err(BeamError::Lanes(steering.len()));
        }
        if !(steering.iter().all(|value| value.is_finite()) && response.is_finite()) {
            return Err(LinalgError::NonFinite.into());
        }
        let limit = self.limit();
        if self.count >= limit {
            return Err(BeamError::TooManyConstraints(limit));
        }
        if self.count > 0 && overlap(self.vector(0), steering) > NULL_OVERLAP_LIMIT {
            return Err(BeamError::NullInMainLobe(self.count));
        }
        self.vectors[self.count][..self.n].copy_from_slice(steering);
        self.response[self.count] = response;
        self.count += 1;
        Ok(())
    }

    #[must_use]
    pub const fn count(&self) -> usize {
        self.count
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.n
    }

    #[must_use]
    pub const fn limit(&self) -> usize {
        let free = self.n.saturating_sub(1);
        if free < MAX_CONSTRAINTS {
            free
        } else {
            MAX_CONSTRAINTS
        }
    }

    #[must_use]
    pub fn vector(&self, index: usize) -> &[Complex<f32>] {
        &self.vectors[index][..self.n]
    }

    #[must_use]
    pub fn response(&self, index: usize) -> Complex<f32> {
        self.response[index]
    }

    pub fn clear(&mut self) {
        self.count = 0;
        self.vectors = [[ZERO; MAX_ORDER]; MAX_CONSTRAINTS];
        self.response = [ZERO; MAX_CONSTRAINTS];
    }
}

fn overlap(a: &[Complex<f32>], b: &[Complex<f32>]) -> f32 {
    let dot: Complex<f32> = a.iter().zip(b).map(|(x, y)| x.conj() * y).sum();
    let norms =
        a.iter().map(Complex::norm_sqr).sum::<f32>() * b.iter().map(Complex::norm_sqr).sum::<f32>();
    if norms > 0.0 {
        dot.norm() / norms.sqrt()
    } else {
        0.0
    }
}

fn snapshot(lanes: &[&[Complex<f32>]], t: usize, out: &mut [Complex<f32>; MAX_ORDER]) -> f32 {
    let mut energy = 0.0f32;
    for (slot, lane) in out.iter_mut().zip(lanes) {
        *slot = lane[t];
        energy += slot.norm_sqr();
    }
    energy
}

fn common_len(lanes: &[&[Complex<f32>]], n: usize) -> Result<usize, BeamError> {
    if lanes.len() != n {
        return Err(BeamError::Lanes(lanes.len()));
    }
    equal_len(lanes.iter().map(|lane| lane.len()))
}

fn equal_len(mut lengths: impl Iterator<Item = usize>) -> Result<usize, BeamError> {
    let first = lengths.next().unwrap_or(0);
    if lengths.all(|len| len == first) {
        Ok(first)
    } else {
        Err(BeamError::LaneLength)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use num_complex::Complex;

    use crate::covariance::SampleCovariance;
    use crate::linalg::CMat;
    use crate::manifold::{Direction, Geometry, Manifold, Winding};

    pub(crate) const FREQ: f64 = 433.92e6;

    pub(crate) fn kraken() -> Manifold {
        Manifold::ideal(Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap())
    }

    pub(crate) fn steering(manifold: &Manifold, azimuth: f64) -> Vec<Complex<f32>> {
        let mut out = vec![Complex::new(0.0, 0.0); manifold.len()];
        manifold.steer(FREQ, Direction::horizon(azimuth), &mut out);
        out
    }

    pub(crate) fn views(lanes: &[Vec<Complex<f32>>]) -> Vec<&[Complex<f32>]> {
        lanes.iter().map(Vec::as_slice).collect()
    }

    pub(crate) fn covariance(lanes: &[Vec<Complex<f32>>]) -> CMat {
        let mut estimator = SampleCovariance::new(lanes.len()).unwrap();
        estimator.accumulate(&views(lanes));
        let mut r = CMat::zeros(lanes.len()).unwrap();
        assert!(estimator.matrix(&mut r));
        r
    }

    pub(crate) fn combine(
        lanes: &[Vec<Complex<f32>>],
        weights: &[Complex<f32>],
    ) -> Vec<Complex<f32>> {
        (0..lanes[0].len())
            .map(|t| lanes.iter().zip(weights).map(|(lane, w)| lane[t] * w).sum())
            .collect()
    }

    pub(crate) fn power(samples: &[Complex<f32>]) -> f32 {
        let sum: f64 = samples.iter().map(|v| f64::from(v.norm_sqr())).sum();
        (sum / samples.len() as f64) as f32
    }

    pub(crate) fn db(ratio: f32) -> f32 {
        10.0 * ratio.log10()
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::testing::{kraken, steering};
    use super::*;

    #[test]
    fn textbook_and_applied_weights_are_conjugates() {
        let textbook = [Complex::new(1.0f32, 2.0), Complex::new(-0.5, 0.25)];
        let set = WeightSet::from_textbook(&textbook);
        assert_eq!(
            set.as_slice(),
            [Complex::new(1.0, -2.0), Complex::new(-0.5, -0.25)]
        );
        let mut back = [Complex::new(0.0f32, 0.0); 2];
        set.textbook(&mut back);
        assert_eq!(back, textbook);
        assert_eq!(WeightSet::unit(3, 1).as_slice()[1], Complex::new(1.0, 0.0));
        assert_eq!(WeightSet::zeros(40).len(), MAX_ORDER);
    }

    #[test]
    fn align_phase_removes_a_common_rotation() {
        let old = WeightSet::from_textbook(&[Complex::new(1.0f32, 0.0), Complex::new(0.3, 0.4)]);
        let mut new = old.clone();
        let turn = Complex::from_polar(1.0f32, 2.1);
        for value in new.as_mut_slice() {
            *value *= turn;
        }
        new.align_phase_to(&old);
        for (a, b) in new.as_slice().iter().zip(old.as_slice()) {
            assert!((a - b).norm() < 1e-6);
        }
        let mut lone = WeightSet::zeros(2);
        lone.align_phase_to(&old);
        assert!(lone.is_finite());
    }

    #[test]
    fn constraints_refuse_too_many_and_wrong_lengths() {
        let manifold = kraken();
        let mut constraints = Constraints::new(5);
        assert_eq!(constraints.limit(), 4);
        assert_eq!(
            constraints.push(&[Complex::new(1.0, 0.0); 3], Complex::new(1.0, 0.0)),
            Err(BeamError::Lanes(3))
        );
        for azimuth in [0.0, 90.0, 140.0, 200.0] {
            constraints
                .push(&steering(&manifold, azimuth), Complex::new(0.0, 0.0))
                .unwrap();
        }
        assert_eq!(
            constraints.push(&steering(&manifold, 45.0), Complex::new(0.0, 0.0)),
            Err(BeamError::TooManyConstraints(4))
        );
        constraints.clear();
        assert_eq!(constraints.count(), 0);
        assert_eq!(constraints, Constraints::new(5));
        assert_eq!(Constraints::new(2).limit(), 1);
    }
}
