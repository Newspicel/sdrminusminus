use sdrmm_dsp::special::circular_mean_deg;

use crate::array_processor::Pose;

const MISSING_SHARE: f64 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeadingAverager {
    sin: f64,
    cos: f64,
    weight: f64,
    sigma_sq: f64,
    missing: f64,
}

impl HeadingAverager {
    pub fn add(&mut self, pose: &Pose, samples: usize) {
        let weight = samples as f64;
        match pose.heading_deg.filter(|heading| heading.is_finite()) {
            Some(heading) => {
                let (sin, cos) = heading.to_radians().sin_cos();
                let sigma = f64::from(pose.heading_sigma_deg).abs();
                self.sin += weight * sin;
                self.cos += weight * cos;
                self.weight += weight;
                self.sigma_sq += weight * sigma * sigma;
            }
            None => self.missing += weight,
        }
    }

    pub fn decay(&mut self, factor: f64) {
        self.sin *= factor;
        self.cos *= factor;
        self.weight *= factor;
        self.sigma_sq *= factor;
        self.missing *= factor;
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn mean_deg(&self) -> Option<f64> {
        let total = self.weight + self.missing;
        let known = self.weight > 0.0 && self.missing <= MISSING_SHARE * total;
        known.then(|| circular_mean_deg(self.sin, self.cos))
    }

    #[must_use]
    pub fn spread_deg(&self) -> f64 {
        if self.weight <= 0.0 {
            return 0.0;
        }
        let resultant = (self.sin.hypot(self.cos) / self.weight).clamp(f64::MIN_POSITIVE, 1.0);
        (-2.0 * resultant.ln()).sqrt().to_degrees()
    }

    #[must_use]
    pub fn sigma_deg(&self) -> f64 {
        if self.weight <= 0.0 {
            return 0.0;
        }
        let spread = self.spread_deg();
        (self.sigma_sq / self.weight + spread * spread).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(heading_deg: Option<f64>, sigma: f32) -> Pose {
        Pose {
            heading_deg,
            heading_sigma_deg: sigma,
            ..Pose::default()
        }
    }

    #[test]
    fn heading_mean_wraps_through_north() {
        let mut average = HeadingAverager::default();
        average.add(&pose(Some(350.0), 1.0), 100);
        average.add(&pose(Some(10.0), 1.0), 100);
        let mean = average.mean_deg().unwrap();
        assert!(!(1e-9..=360.0 - 1e-9).contains(&mean), "{mean}");
        assert!((average.spread_deg() - 10.0).abs() < 0.2);
        assert!(average.sigma_deg() > average.spread_deg());
    }

    #[test]
    fn heading_is_unknown_when_most_samples_lack_it() {
        let mut average = HeadingAverager::default();
        assert_eq!(average.mean_deg(), None);
        average.add(&pose(Some(90.0), 2.0), 100);
        average.add(&pose(None, 0.0), 300);
        assert_eq!(average.mean_deg(), None);
        average.add(&pose(Some(90.0), 2.0), 300);
        assert!((average.mean_deg().unwrap() - 90.0).abs() < 1e-9);
        assert!((average.sigma_deg() - 2.0).abs() < 1e-6);
        average.decay(0.5);
        assert!((average.mean_deg().unwrap() - 90.0).abs() < 1e-9);
        average.reset();
        assert_eq!(average.mean_deg(), None);
        assert_eq!(average.sigma_deg(), 0.0);
    }
}
