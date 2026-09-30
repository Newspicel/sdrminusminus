pub const SLIP_SAMPLES: f64 = 1.0;
pub const DRIFT_SAMPLES_PER_S: f64 = 0.01;
pub const DRIFT_MIN_POINTS: usize = 3;
pub const DRIFT_MIN_SPAN_S: f64 = 30.0;

const HISTORY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DriftClass {
    Locked,
    Drifting { rate: f64 },
    Slipped { by: f64 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriftTrack {
    points: [(f64, f64); HISTORY],
    len: usize,
}

impl DriftTrack {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            points: [(0.0, 0.0); HISTORY],
            len: 0,
        }
    }

    pub fn push(&mut self, t_s: f64, residual: f64) -> DriftClass {
        let by = residual - self.expected_at(t_s);
        if !by.is_finite() || by.abs() > SLIP_SAMPLES {
            self.clear();
            return DriftClass::Slipped { by };
        }
        if self.len == HISTORY {
            self.points.copy_within(1.., 0);
            self.len -= 1;
        }
        self.points[self.len] = (t_s, residual);
        self.len += 1;
        match self.slope() {
            Some(rate) if rate.abs() > DRIFT_SAMPLES_PER_S => DriftClass::Drifting { rate },
            _ => DriftClass::Locked,
        }
    }

    #[must_use]
    pub fn slope(&self) -> Option<f64> {
        self.fit().map(|(_, slope)| slope)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    fn expected_at(&self, t_s: f64) -> f64 {
        if let Some((intercept, slope)) = self.fit() {
            return intercept + slope * t_s;
        }
        self.points[..self.len]
            .last()
            .map_or(0.0, |&(_, residual)| residual)
    }

    fn fit(&self) -> Option<(f64, f64)> {
        let points = &self.points[..self.len];
        let (first, last) = (points.first()?.0, points.last()?.0);
        if points.len() < DRIFT_MIN_POINTS || last - first < DRIFT_MIN_SPAN_S {
            return None;
        }
        let count = points.len() as f64;
        let mean_t = points.iter().map(|p| p.0).sum::<f64>() / count;
        let mean_r = points.iter().map(|p| p.1).sum::<f64>() / count;
        let (mut spread, mut covariance) = (0.0, 0.0);
        for &(t, r) in points {
            spread += (t - mean_t) * (t - mean_t);
            covariance += (t - mean_t) * (r - mean_r);
        }
        if spread <= f64::MIN_POSITIVE {
            return None;
        }
        let slope = covariance / spread;
        Some((mean_r - slope * mean_t, slope))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_track_tells_locked_drifting_and_slipped_apart() {
        let mut locked = DriftTrack::new();
        for (step, wobble) in [0.02, -0.03, 0.01, -0.02, 0.03].into_iter().enumerate() {
            assert_eq!(locked.push(60.0 * step as f64, wobble), DriftClass::Locked);
        }
        assert!(locked.slope().is_some_and(|slope| slope.abs() < 1e-3));

        let mut drifting = DriftTrack::new();
        let mut last = DriftClass::Locked;
        for step in 0..12 {
            let t = 10.0 * f64::from(step);
            last = drifting.push(t, 0.02 * t);
            if step < 3 {
                assert_eq!(last, DriftClass::Locked, "step {step}");
            }
        }
        match last {
            DriftClass::Drifting { rate } => assert!((rate - 0.02).abs() < 1e-9, "{rate}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(drifting.len(), 8);

        let by = 1.5 - locked.expected_at(300.0);
        assert!(by > 1.4);
        assert_eq!(locked.push(300.0, 1.5), DriftClass::Slipped { by });
        assert!(locked.is_empty());
        assert!(locked.slope().is_none());
        assert_eq!(locked.push(360.0, 0.4), DriftClass::Locked);
    }

    #[test]
    fn a_first_residual_beyond_a_sample_is_a_slip() {
        let mut track = DriftTrack::new();
        assert_eq!(track.push(0.0, -1.2), DriftClass::Slipped { by: -1.2 });
        assert_eq!(track.push(1.0, 0.9), DriftClass::Locked);
        assert!(matches!(
            track.push(2.0, f64::NAN),
            DriftClass::Slipped { .. }
        ));
    }

    #[test]
    fn a_slope_needs_three_points_over_thirty_seconds() {
        let mut track = DriftTrack::new();
        for t in [0.0, 5.0, 10.0, 20.0] {
            assert_eq!(track.push(t, 0.05 * t), DriftClass::Locked);
        }
        assert!(track.slope().is_none());
        assert!(matches!(
            track.push(30.0, 1.5),
            DriftClass::Drifting { rate } if (rate - 0.05).abs() < 1e-9
        ));
    }
}
