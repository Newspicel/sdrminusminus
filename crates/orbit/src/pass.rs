use crate::{Observer, OrbitError, Satellite};

const COARSE_S: f64 = 20.0;
const FINE_S: f64 = 0.5;
const LOOK_BACK_S: f64 = 3.0 * 3_600.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pass {
    pub aos_unix: Option<f64>,
    pub los_unix: Option<f64>,
    pub max_elevation_deg: f64,
    pub max_unix: f64,
}

impl Satellite {
    pub fn next_pass(
        &self,
        observer: &Observer,
        from_unix: f64,
        horizon_s: f64,
        min_elevation_deg: f64,
    ) -> Result<Option<Pass>, OrbitError> {
        let above = |t: f64| -> Result<bool, OrbitError> {
            Ok(self.look(observer, t)?.elevation_deg >= min_elevation_deg)
        };
        let end = from_unix + horizon_s;
        let mut start = from_unix;
        let aos = if above(from_unix)? {
            let earliest = from_unix - LOOK_BACK_S;
            let mut t = from_unix;
            while t > earliest && above(t - COARSE_S)? {
                t -= COARSE_S;
            }
            (t > earliest)
                .then(|| edge(&above, t - COARSE_S, t))
                .transpose()?
        } else {
            let Some(rise) = first(&above, from_unix, end, true)? else {
                return Ok(None);
            };
            start = rise;
            Some(rise)
        };
        let los = first(&above, start, end, false)?;
        let (max_unix, max_elevation_deg) =
            self.culmination(observer, aos.unwrap_or(from_unix), los.unwrap_or(end))?;
        Ok(Some(Pass {
            aos_unix: aos,
            los_unix: los,
            max_elevation_deg,
            max_unix,
        }))
    }

    fn culmination(
        &self,
        observer: &Observer,
        from: f64,
        to: f64,
    ) -> Result<(f64, f64), OrbitError> {
        let elevation = |t: f64| self.look(observer, t).map(|look| look.elevation_deg);
        let mut best = (from, elevation(from)?);
        let mut t = from;
        while t < to {
            t = (t + COARSE_S).min(to);
            let here = elevation(t)?;
            if here > best.1 {
                best = (t, here);
            }
        }
        let (mut low, mut high) = (best.0 - COARSE_S, best.0 + COARSE_S);
        while high - low > FINE_S {
            let a = low + (high - low) / 3.0;
            let b = high - (high - low) / 3.0;
            if elevation(a)? < elevation(b)? {
                low = a;
            } else {
                high = b;
            }
        }
        let peak = (low + high) / 2.0;
        let at_peak = elevation(peak)?;
        Ok(if at_peak > best.1 {
            (peak, at_peak)
        } else {
            best
        })
    }
}

fn first(
    above: &impl Fn(f64) -> Result<bool, OrbitError>,
    from: f64,
    to: f64,
    rising: bool,
) -> Result<Option<f64>, OrbitError> {
    let mut t = from;
    while t < to {
        let next = t + COARSE_S;
        if above(next)? == rising {
            return edge(above, t, next).map(Some);
        }
        t = next;
    }
    Ok(None)
}

fn edge(
    above: &impl Fn(f64) -> Result<bool, OrbitError>,
    mut before: f64,
    mut after: f64,
) -> Result<f64, OrbitError> {
    let rising = above(after)?;
    while after - before > FINE_S {
        let middle = (before + after) / 2.0;
        if above(middle)? == rising {
            after = middle;
        } else {
            before = middle;
        }
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Tle, time::unix_seconds};

    const GEO: &str = "1 43700U 18090A   24001.50000000  .00000100  00000-0  00000-0 0  9995
2 43700   0.0543  87.2931 0001822 312.7102 147.3500  1.00271332000017";

    fn satellite(text: &str) -> Satellite {
        Satellite::new(Tle::parse(text).expect("valid set")).expect("usable")
    }

    const BERLIN: Observer = Observer {
        latitude_deg: 52.52,
        longitude_deg: 13.405,
        altitude_m: 40.0,
    };

    #[test]
    fn a_low_orbit_rises_culminates_and_sets() {
        let iss = satellite(crate::tle::tests::ISS);
        let epoch = unix_seconds(iss.tle.epoch_jd);
        let pass = iss
            .next_pass(&BERLIN, epoch, 86_400.0, 0.0)
            .expect("propagates")
            .expect("the station passes over Berlin within a day");
        let (Some(aos), Some(los)) = (pass.aos_unix, pass.los_unix) else {
            panic!("{pass:?}");
        };
        assert!(aos < pass.max_unix && pass.max_unix < los, "{pass:?}");
        assert!((120.0..900.0).contains(&(los - aos)), "{pass:?}");
        let rise = iss.look(&BERLIN, aos).expect("look").elevation_deg;
        assert!(rise.abs() < 0.1, "{rise}");
        let top = iss
            .look(&BERLIN, pass.max_unix)
            .expect("look")
            .elevation_deg;
        assert!((top - pass.max_elevation_deg).abs() < 1e-9);
        assert!(top > 0.0 && top <= 90.0);
    }

    #[test]
    fn a_pass_already_underway_reports_when_it_rose() {
        let iss = satellite(crate::tle::tests::ISS);
        let epoch = unix_seconds(iss.tle.epoch_jd);
        let first = iss
            .next_pass(&BERLIN, epoch, 86_400.0, 0.0)
            .expect("propagates")
            .expect("a pass");
        let midway = iss
            .next_pass(&BERLIN, first.max_unix, 86_400.0, 0.0)
            .expect("propagates")
            .expect("still passing");
        let (Some(aos), Some(was)) = (midway.aos_unix, first.aos_unix) else {
            panic!("{midway:?}");
        };
        assert!((aos - was).abs() < 1.0, "{aos} vs {was}");
    }

    #[test]
    fn a_satellite_that_never_sets_has_no_edges() {
        let geo = satellite(GEO);
        let epoch = unix_seconds(geo.tle.epoch_jd);
        let under = (0..12)
            .map(|step| Observer {
                latitude_deg: 0.0,
                longitude_deg: -180.0 + 30.0 * f64::from(step),
                altitude_m: 0.0,
            })
            .find(|site| {
                geo.look(site, epoch)
                    .is_ok_and(|look| look.elevation_deg > 30.0)
            })
            .expect("somewhere on the equator sees it high");
        let pass = geo
            .next_pass(&under, epoch, 86_400.0, 0.0)
            .expect("propagates")
            .expect("visible");
        assert_eq!((pass.aos_unix, pass.los_unix), (None, None), "{pass:?}");
    }
}
