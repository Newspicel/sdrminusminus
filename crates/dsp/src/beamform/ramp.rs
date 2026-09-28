use num_complex::Complex;

use super::{BeamError, MAX_ORDER, WeightSet, ZERO, common_len, snapshot};
use crate::manifold::{Manifold, SteeringGrid};

pub struct WeightRamp {
    current: WeightSet,
    start: WeightSet,
    target: WeightSet,
    remaining: u32,
    total: u32,
}

impl WeightRamp {
    #[must_use]
    pub const fn new(n: usize) -> Self {
        Self {
            current: WeightSet::zeros(n),
            start: WeightSet::zeros(n),
            target: WeightSet::zeros(n),
            remaining: 0,
            total: 0,
        }
    }

    pub fn set_target(&mut self, target: &WeightSet, ramp_samples: u32) {
        self.start.clone_from(&self.current);
        self.target.clone_from(target);
        if ramp_samples == 0 || self.start.len() != target.len() {
            self.current.clone_from(target);
            self.remaining = 0;
            self.total = 0;
        } else {
            self.remaining = ramp_samples;
            self.total = ramp_samples;
        }
    }

    pub fn apply(
        &mut self,
        lanes: &[&[Complex<f32>]],
        out: &mut Vec<Complex<f32>>,
    ) -> Result<(), BeamError> {
        let n = self.current.len();
        let len = common_len(lanes, n)?;
        let ramped = len.min(self.remaining as usize);
        let mut x = [ZERO; MAX_ORDER];
        for t in 0..ramped {
            self.step();
            snapshot(lanes, t, &mut x);
            out.push(self.current.response(&x[..n]));
        }
        let from = out.len();
        out.resize(from + len - ramped, ZERO);
        let steady = &mut out[from..];
        for (lane, &weight) in lanes.iter().zip(self.current.as_slice()) {
            for (value, sample) in steady.iter_mut().zip(&lane[ramped..len]) {
                *value += sample * weight;
            }
        }
        Ok(())
    }

    #[must_use]
    pub const fn current(&self) -> &WeightSet {
        &self.current
    }

    #[must_use]
    pub const fn ramping(&self) -> bool {
        self.remaining > 0
    }

    fn step(&mut self) {
        self.remaining -= 1;
        if self.remaining == 0 {
            self.current.clone_from(&self.target);
            return;
        }
        let fraction = 1.0 - self.remaining as f32 / self.total as f32;
        for ((value, start), target) in self
            .current
            .as_mut_slice()
            .iter_mut()
            .zip(self.start.as_slice())
            .zip(self.target.as_slice())
        {
            *value = start + (target - start) * fraction;
        }
    }
}

pub fn pattern(
    manifold: &Manifold,
    freq_hz: f64,
    weights: &WeightSet,
    ring: &SteeringGrid,
    out: &mut [f32],
) -> Result<(), BeamError> {
    let n = weights.len();
    if manifold.len() != n || ring.elements() != n {
        return Err(BeamError::Lanes(manifold.len()));
    }
    if out.len() != ring.points() {
        return Err(BeamError::Setting("pattern length"));
    }
    if !(freq_hz.is_finite() && freq_hz > 0.0) {
        return Err(BeamError::Setting("frequency"));
    }
    let same = (ring.freq_hz() - freq_hz).abs() <= 1e-9 * freq_hz;
    let mut a = [ZERO; MAX_ORDER];
    let mut peak = 0.0f32;
    for (point, slot) in out.iter_mut().enumerate() {
        *slot = if same {
            weights.response(ring.vector(point)).norm_sqr()
        } else {
            manifold.steer(freq_hz, ring.direction(point), &mut a[..n]);
            weights.response(&a[..n]).norm_sqr()
        };
        peak = peak.max(*slot);
    }
    if peak.is_finite() && peak > 0.0 {
        let scale = peak.recip();
        for value in out.iter_mut() {
            *value *= scale;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use num_complex::Complex;

    use super::*;
    use crate::beamform::testing::{FREQ, kraken, steering};
    use crate::beamform::{BlockSolver, Constraints};
    use crate::manifold::GridSpec;

    fn close(a: &WeightSet, b: &[Complex<f32>]) -> bool {
        a.as_slice()
            .iter()
            .zip(b)
            .all(|(x, y)| (x - y).norm() < 1e-6)
    }

    #[test]
    fn weight_ramp_is_linear_and_lands_on_target() {
        let start = WeightSet::unit(2, 0);
        let target = WeightSet::from_textbook(&[Complex::new(0.2, 0.6), Complex::new(-1.0, 0.4)]);
        let mut ramp = WeightRamp::new(2);
        ramp.set_target(&start, 0);
        assert_eq!(ramp.current(), &start);
        ramp.set_target(&target, 100);
        let ones = vec![Complex::new(1.0f32, 0.0); 50];
        let zeros = vec![Complex::new(0.0f32, 0.0); 50];
        let mut out = Vec::with_capacity(200);
        ramp.apply(&[&ones, &zeros], &mut out).unwrap();
        let mean: Vec<Complex<f32>> = start
            .as_slice()
            .iter()
            .zip(target.as_slice())
            .map(|(a, b)| (a + b) * 0.5)
            .collect();
        assert!(close(ramp.current(), &mean));
        assert!((out[49] - mean[0]).norm() < 1e-6);
        let steps: Vec<f32> = out
            .windows(2)
            .map(|pair| (pair[1] - pair[0]).norm())
            .collect();
        assert!(steps.iter().all(|step| (step - steps[0]).abs() < 1e-5));
        ramp.apply(&[&ones, &zeros], &mut out).unwrap();
        assert_eq!(ramp.current(), &target);
        assert!(!ramp.ramping());
        ramp.apply(&[&ones, &zeros], &mut out).unwrap();
        assert_eq!(out.len(), 150);
        assert!((out[149] - target.as_slice()[0]).norm() < 1e-6);
        assert_eq!(ramp.apply(&[&ones], &mut out), Err(BeamError::Lanes(1)));
        assert_eq!(
            ramp.apply(&[&ones, &zeros[..10]], &mut out),
            Err(BeamError::LaneLength)
        );
        assert_eq!(out.len(), 150);
    }

    #[test]
    fn pattern_is_unity_at_the_steer_and_deep_at_a_null() {
        let manifold = kraken();
        let ring = SteeringGrid::new(&manifold, GridSpec::ring(1.0), FREQ).unwrap();
        let mut solver = BlockSolver::new(5).unwrap();
        let mut weights = WeightSet::zeros(5);
        solver
            .das(&steering(&manifold, 90.0), &mut weights)
            .unwrap();
        let mut out = [0.0f32; 360];
        pattern(&manifold, FREQ, &weights, &ring, &mut out).unwrap();
        let top = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index);
        assert_eq!(top, Some(90));
        assert!((out[90] - 1.0).abs() < 1e-6);
        let moved = SteeringGrid::new(&manifold, GridSpec::ring(1.0), FREQ * 1.01).unwrap();
        pattern(&manifold, FREQ, &weights, &moved, &mut out).unwrap();
        assert!((out[90] - 1.0).abs() < 1e-6);

        let mut constraints = Constraints::new(5);
        constraints
            .push(&steering(&manifold, 90.0), Complex::new(1.0, 0.0))
            .unwrap();
        constraints
            .push(&steering(&manifold, 200.0), Complex::new(0.0, 0.0))
            .unwrap();
        solver.lcmv(None, &constraints, 0.0, &mut weights).unwrap();
        pattern(&manifold, FREQ, &weights, &ring, &mut out).unwrap();
        assert!(10.0 * out[200].log10() < -40.0, "{}", out[200]);
        assert!(out[90] > 0.1);
        assert_eq!(
            pattern(&manifold, FREQ, &weights, &ring, &mut [0.0; 10]),
            Err(BeamError::Setting("pattern length"))
        );
    }
}
