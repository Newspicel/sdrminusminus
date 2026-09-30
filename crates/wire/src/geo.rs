use std::f64::consts::PI;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const EARTH_RADIUS_M: f64 = 6_371_000.0;

const METRES_PER_DEGREE: f64 = EARTH_RADIUS_M * PI / 180.0;
const MIN_COS_LAT: f64 = 1e-6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Enu {
    origin: LatLon,
    m_per_lon: f64,
}

impl Enu {
    #[must_use]
    pub fn new(origin: LatLon) -> Self {
        Self {
            origin,
            m_per_lon: METRES_PER_DEGREE * cos_lat(origin.lat),
        }
    }

    #[must_use]
    pub fn origin(&self) -> LatLon {
        self.origin
    }

    #[must_use]
    pub fn to_enu(&self, point: LatLon) -> (f64, f64) {
        let east = wrap_180(point.lon - self.origin.lon) * self.m_per_lon;
        let north = (point.lat - self.origin.lat) * METRES_PER_DEGREE;
        (east, north)
    }

    #[must_use]
    pub fn to_latlon(&self, east_m: f64, north_m: f64) -> LatLon {
        LatLon {
            lat: self.origin.lat + north_m / METRES_PER_DEGREE,
            lon: wrap_180(self.origin.lon + east_m / self.m_per_lon),
        }
    }
}

fn cos_lat(lat_deg: f64) -> f64 {
    lat_deg.to_radians().cos().max(MIN_COS_LAT)
}

#[must_use]
pub fn distance_m(from: LatLon, to: LatLon) -> f64 {
    let phi1 = from.lat.to_radians();
    let phi2 = to.lat.to_radians();
    let dphi = phi2 - phi1;
    let dlambda = (to.lon - from.lon).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().clamp(0.0, 1.0).asin()
}

#[must_use]
pub fn bearing_deg(from: LatLon, to: LatLon) -> f64 {
    let phi1 = from.lat.to_radians();
    let phi2 = to.lat.to_radians();
    let dlambda = (to.lon - from.lon).to_radians();
    let y = dlambda.sin() * phi2.cos();
    let x = phi1
        .cos()
        .mul_add(phi2.sin(), -(phi1.sin() * phi2.cos() * dlambda.cos()));
    wrap_360(y.atan2(x).to_degrees())
}

#[must_use]
pub fn destination(from: LatLon, bearing_deg: f64, distance_m: f64) -> LatLon {
    let bearing = bearing_deg.to_radians();
    let angular = distance_m / EARTH_RADIUS_M;
    let phi = from.lat.to_radians();
    let lambda = from.lon.to_radians();
    let sin_phi2 = phi
        .sin()
        .mul_add(angular.cos(), phi.cos() * angular.sin() * bearing.cos());
    let phi2 = sin_phi2.clamp(-1.0, 1.0).asin();
    let lambda2 = lambda
        + (bearing.sin() * angular.sin() * phi.cos()).atan2(angular.cos() - phi.sin() * sin_phi2);
    LatLon {
        lat: phi2.to_degrees(),
        lon: wrap_180(lambda2.to_degrees()),
    }
}

#[must_use]
pub fn offset_m(anchor: LatLon, east_m: f64, north_m: f64) -> LatLon {
    LatLon {
        lat: anchor.lat + (north_m / EARTH_RADIUS_M).to_degrees(),
        lon: wrap_180(anchor.lon + (east_m / (EARTH_RADIUS_M * cos_lat(anchor.lat))).to_degrees()),
    }
}

#[must_use]
pub fn wrap_180(deg: f64) -> f64 {
    let wrapped = wrap_360(deg);
    if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

#[must_use]
pub fn wrap_360(deg: f64) -> f64 {
    let wrapped = deg.rem_euclid(360.0);
    if wrapped >= 360.0 { 0.0 } else { wrapped + 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONDON: LatLon = LatLon {
        lat: 51.5074,
        lon: -0.1278,
    };
    const PARIS: LatLon = LatLon {
        lat: 48.8566,
        lon: 2.3522,
    };

    fn close(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn geo_distance_and_bearing_match_known_points() {
        let origin = LatLon::default();
        let one_degree = METRES_PER_DEGREE;
        assert!(close(
            distance_m(origin, LatLon { lat: 0.0, lon: 1.0 }),
            one_degree,
            1e-6
        ));
        assert!(close(
            distance_m(origin, LatLon { lat: 1.0, lon: 0.0 }),
            one_degree,
            1e-6
        ));
        assert!(close(
            bearing_deg(origin, LatLon { lat: 0.0, lon: 1.0 }),
            90.0,
            1e-9
        ));
        assert!(close(
            bearing_deg(origin, LatLon { lat: 1.0, lon: 0.0 }),
            0.0,
            1e-9
        ));
        assert!(close(
            bearing_deg(
                origin,
                LatLon {
                    lat: -1.0,
                    lon: 0.0
                }
            ),
            180.0,
            1e-9
        ));
        assert!(close(
            bearing_deg(
                origin,
                LatLon {
                    lat: 0.0,
                    lon: -1.0
                }
            ),
            270.0,
            1e-9
        ));
        assert!(close(distance_m(LONDON, PARIS), 343_556.0, 5.0));
        assert!(close(bearing_deg(LONDON, PARIS), 148.1, 0.05));
        assert_eq!(distance_m(PARIS, PARIS), 0.0);
    }

    #[test]
    fn destination_walks_back_to_the_known_point() {
        let there = destination(
            LONDON,
            bearing_deg(LONDON, PARIS),
            distance_m(LONDON, PARIS),
        );
        assert!(distance_m(there, PARIS) < 1e-3);
        let across = destination(
            LatLon {
                lat: 0.0,
                lon: 179.5,
            },
            90.0,
            METRES_PER_DEGREE,
        );
        assert!(close(across.lon, -179.5, 1e-9));
        assert!(close(across.lat, 0.0, 1e-9));
    }

    #[test]
    fn enu_round_trips_within_a_millimetre() {
        for origin in [
            LatLon {
                lat: 52.52,
                lon: 13.405,
            },
            LatLon {
                lat: -33.87,
                lon: 151.21,
            },
            LatLon {
                lat: 0.0,
                lon: 179.99,
            },
        ] {
            let enu = Enu::new(origin);
            assert_eq!(enu.origin(), origin);
            assert_eq!(enu.to_enu(origin), (0.0, 0.0));
            for (east, north) in [
                (0.0, 0.0),
                (1_234.5, -987.6),
                (-30_000.0, 30_000.0),
                (0.001, 0.001),
            ] {
                let point = enu.to_latlon(east, north);
                let (e, n) = enu.to_enu(point);
                assert!(close(e, east, 1e-3), "{origin:?} east {e} != {east}");
                assert!(close(n, north, 1e-3), "{origin:?} north {n} != {north}");
                let back = enu.to_latlon(e, n);
                assert!(distance_m(back, point) < 1e-3);
            }
        }
    }

    #[test]
    fn offset_moves_by_metres_on_a_flat_earth() {
        let anchor = LatLon {
            lat: 52.52,
            lon: 13.405,
        };
        let north = offset_m(anchor, 0.0, 1_000.0);
        assert!(close(distance_m(anchor, north), 1_000.0, 0.01));
        assert!(close(bearing_deg(anchor, north), 0.0, 1e-6));
        let east = offset_m(anchor, 1_000.0, 0.0);
        assert!(close(distance_m(anchor, east), 1_000.0, 0.01));
        assert!(close(bearing_deg(anchor, east), 90.0, 0.01));
        let pole = offset_m(
            LatLon {
                lat: 90.0,
                lon: 0.0,
            },
            1.0,
            0.0,
        );
        assert!(pole.lon.is_finite());
    }

    #[test]
    fn wrap_helpers_cover_their_edges() {
        assert_eq!(wrap_360(0.0), 0.0);
        assert_eq!(wrap_360(360.0), 0.0);
        assert_eq!(wrap_360(720.0), 0.0);
        assert_eq!(wrap_360(-10.0), 350.0);
        assert_eq!(wrap_360(370.0), 10.0);
        assert!(wrap_360(-0.0).is_sign_positive());
        assert!((0.0..360.0).contains(&wrap_360(-1e-18)));
        assert!((0.0..360.0).contains(&wrap_360(359.999_999_999_999_9)));
        assert_eq!(wrap_180(180.0), 180.0);
        assert_eq!(wrap_180(-180.0), 180.0);
        assert_eq!(wrap_180(190.0), -170.0);
        assert_eq!(wrap_180(-190.0), 170.0);
        assert_eq!(wrap_180(540.0), 180.0);
        assert!(wrap_180(-0.0).is_sign_positive());
        assert!(wrap_180(f64::NAN).is_nan());
    }

    #[test]
    fn lat_lon_json_is_two_numbers() {
        let json = serde_json::to_value(PARIS).unwrap();
        assert_eq!(json, serde_json::json!({"lat": 48.8566, "lon": 2.3522}));
        assert_eq!(serde_json::from_value::<LatLon>(json).unwrap(), PARIS);
    }
}
