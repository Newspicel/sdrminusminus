use sdrmm_wire::geo::{wrap_180, wrap_360};

pub(crate) const Q_GYRO: f64 = 0.25;
pub(crate) const Q_BLIND: f64 = 25.0;
pub(crate) const REVERSE_BAND_DEG: f64 = 25.0;
pub(crate) const GATE_FLOOR_DEG: f64 = 10.0;
pub(crate) const REACQUIRE_AFTER: u32 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Update {
    Accepted,
    Initialised,
    Rejected,
    Reset,
    Ignored,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HeadingFilter {
    psi: f64,
    p: f64,
    valid: bool,
    rejects: u32,
}

impl HeadingFilter {
    pub(crate) fn predict_gyro(&mut self, rate_deg_s: f64, dt_s: f64) {
        if !self.valid {
            return;
        }
        self.psi = wrap_360(rate_deg_s.mul_add(dt_s, self.psi));
        self.p = Q_GYRO.mul_add(dt_s, self.p);
    }

    pub(crate) fn predict_blind(&mut self, dt_s: f64) {
        if self.valid {
            self.p = Q_BLIND.mul_add(dt_s, self.p);
        }
    }

    pub(crate) fn inflate(&mut self, sigma_deg: f64) {
        if self.valid {
            self.p = sigma_deg.mul_add(sigma_deg, self.p);
        }
    }

    pub(crate) fn shift(&mut self, deg: f64) {
        if self.valid {
            self.psi = wrap_360(self.psi + deg);
        }
    }

    pub(crate) fn update(&mut self, z_deg: f64, sigma_deg: f64, reverse_band: bool) -> Update {
        let r = sigma_deg * sigma_deg;
        if !self.valid {
            self.restart(z_deg, r);
            return Update::Initialised;
        }
        let innovation = wrap_180(z_deg - self.psi);
        if reverse_band && innovation.abs() >= 180.0 - REVERSE_BAND_DEG {
            return Update::Ignored;
        }
        let s = self.p + r;
        if innovation.abs() > (3.0 * s.sqrt()).max(GATE_FLOOR_DEG) {
            self.rejects += 1;
            if self.rejects >= REACQUIRE_AFTER {
                self.restart(z_deg, r);
                return Update::Reset;
            }
            return Update::Rejected;
        }
        let gain = self.p / s;
        self.psi = wrap_360(gain.mul_add(innovation, self.psi));
        self.p *= 1.0 - gain;
        self.rejects = 0;
        Update::Accepted
    }

    fn restart(&mut self, z_deg: f64, r: f64) {
        self.psi = wrap_360(z_deg);
        self.p = r;
        self.valid = true;
        self.rejects = 0;
    }

    pub(crate) fn heading(&self) -> Option<(f64, f64)> {
        self.valid.then(|| (self.psi, self.p.sqrt()))
    }

    pub(crate) fn valid(&self) -> bool {
        self.valid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(psi: f64, sigma: f64) -> HeadingFilter {
        let mut filter = HeadingFilter::default();
        filter.update(psi, sigma, false);
        filter
    }

    #[test]
    fn a_first_measurement_initialises_the_heading() {
        let mut filter = HeadingFilter::default();
        assert_eq!(filter.heading(), None);
        filter.predict_gyro(10.0, 1.0);
        assert_eq!(filter.update(370.0, 5.0, false), Update::Initialised);
        assert_eq!(filter.heading(), Some((10.0, 5.0)));
    }

    #[test]
    fn innovations_wrap_across_north() {
        let mut filter = at(359.0, 5.0);
        assert_eq!(filter.update(1.0, 5.0, false), Update::Accepted);
        let (psi, _) = filter.heading().expect("valid");
        assert!(wrap_180(psi).abs() < 0.5, "{psi}");
    }

    #[test]
    fn a_right_turn_increases_heading_and_wraps() {
        let mut filter = at(350.0, 2.0);
        let rate = 0.349f64.to_degrees();
        for _ in 0..100 {
            filter.predict_gyro(rate, 0.01);
        }
        let (psi, _) = filter.heading().expect("valid");
        assert!((psi - 10.0).abs() < 0.1, "{psi}");
    }

    #[test]
    fn outliers_are_gated_then_reacquired_after_five() {
        let mut filter = at(90.0, 2.0);
        for _ in 0..4 {
            assert_eq!(filter.update(200.0, 2.0, false), Update::Rejected);
        }
        assert_eq!(filter.heading().map(|(psi, _)| psi), Some(90.0));
        assert_eq!(filter.update(200.0, 2.0, false), Update::Reset);
        assert_eq!(filter.heading(), Some((200.0, 2.0)));
    }

    #[test]
    fn a_reversing_course_is_ignored_and_not_counted() {
        let mut filter = at(90.0, 2.0);
        for _ in 0..10 {
            assert_eq!(filter.update(272.0, 2.0, true), Update::Ignored);
        }
        assert_eq!(filter.heading(), Some((90.0, 2.0)));
        assert_eq!(filter.update(92.0, 2.0, true), Update::Accepted);
    }

    #[test]
    fn sigma_grows_while_holding() {
        let mut filter = at(10.0, 2.0);
        for _ in 0..600 {
            filter.predict_gyro(0.0, 0.1);
        }
        let (_, sigma) = filter.heading().expect("valid");
        assert!(
            (sigma - Q_GYRO.mul_add(60.0, 4.0).sqrt()).abs() < 1e-9,
            "{sigma}"
        );
    }

    #[test]
    fn blind_prediction_grows_faster() {
        let mut gyro = at(10.0, 2.0);
        let mut blind = gyro;
        gyro.predict_gyro(0.0, 2.0);
        blind.predict_blind(2.0);
        let sigma = |filter: HeadingFilter| filter.heading().map_or(0.0, |(_, sigma)| sigma);
        assert!(sigma(blind) > 2.0 * sigma(gyro));
        blind.inflate(30.0);
        assert!((sigma(blind) - (4.0 + 50.0 + 900.0f64).sqrt()).abs() < 1e-9);
    }
}
