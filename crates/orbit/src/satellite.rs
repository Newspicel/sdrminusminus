use crate::{Look, Observer, OrbitError, Propagator, Tle, time::julian_date};

#[derive(Clone, Debug)]
pub struct Satellite {
    pub tle: Tle,
    propagator: Propagator,
}

impl Satellite {
    pub fn new(tle: Tle) -> Result<Self, OrbitError> {
        let propagator = Propagator::new(&tle)?;
        Ok(Self { tle, propagator })
    }

    pub fn look(&self, observer: &Observer, unix_seconds: f64) -> Result<Look, OrbitError> {
        let jd = julian_date(unix_seconds);
        let state = self
            .propagator
            .propagate(self.propagator.minutes_since_epoch(jd))?;
        Ok(observer.look(&state, jd))
    }

    #[must_use]
    pub fn age_days(&self, unix_seconds: f64) -> f64 {
        julian_date(unix_seconds) - self.propagator.epoch_jd
    }
}
