use crate::{State, time::sidereal_angle};

const WGS84_A_KM: f64 = 6_378.137;
const WGS84_F: f64 = 1.0 / 298.257_223_563;
const EARTH_ROTATION_RAD_S: f64 = 7.292_115_146_706_979e-5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observer {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub azimuth_deg: f64,
    pub elevation_deg: f64,
    pub range_km: f64,
    pub range_rate_km_s: f64,
}

impl Observer {
    #[must_use]
    pub fn look(&self, state: &State, julian_date: f64) -> Look {
        let (position, velocity) = earth_fixed(state, sidereal_angle(julian_date));
        let site = self.earth_fixed();
        let rho = [0, 1, 2].map(|i| position[i] - site[i]);
        let range = rho.iter().map(|v| v * v).sum::<f64>().sqrt();
        let (sin_lat, cos_lat) = self.latitude_deg.to_radians().sin_cos();
        let (sin_lon, cos_lon) = self.longitude_deg.to_radians().sin_cos();
        let south = sin_lat * cos_lon * rho[0] + sin_lat * sin_lon * rho[1] - cos_lat * rho[2];
        let east = -sin_lon * rho[0] + cos_lon * rho[1];
        let up = cos_lat * cos_lon * rho[0] + cos_lat * sin_lon * rho[1] + sin_lat * rho[2];
        Look {
            azimuth_deg: east.atan2(-south).to_degrees().rem_euclid(360.0),
            elevation_deg: (up / range).asin().to_degrees(),
            range_km: range,
            range_rate_km_s: (0..3).map(|i| rho[i] * velocity[i]).sum::<f64>() / range,
        }
    }

    fn earth_fixed(&self) -> [f64; 3] {
        let e2 = WGS84_F * (2.0 - WGS84_F);
        let (sin_lat, cos_lat) = self.latitude_deg.to_radians().sin_cos();
        let (sin_lon, cos_lon) = self.longitude_deg.to_radians().sin_cos();
        let n = WGS84_A_KM / (1.0 - e2 * sin_lat * sin_lat).sqrt();
        let h = self.altitude_m / 1_000.0;
        [
            (n + h) * cos_lat * cos_lon,
            (n + h) * cos_lat * sin_lon,
            (n * (1.0 - e2) + h) * sin_lat,
        ]
    }
}

fn earth_fixed(state: &State, sidereal: f64) -> ([f64; 3], [f64; 3]) {
    let (sin, cos) = sidereal.sin_cos();
    let rotate = |v: [f64; 3]| [cos * v[0] + sin * v[1], -sin * v[0] + cos * v[1], v[2]];
    let position = rotate(state.position_km);
    let turned = rotate(state.velocity_km_s);
    let velocity = [
        turned[0] + EARTH_ROTATION_RAD_S * position[1],
        turned[1] - EARTH_ROTATION_RAD_S * position[0],
        turned[2],
    ];
    (position, velocity)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JD: f64 = 2_460_311.3;

    fn inertial(position: [f64; 3], velocity: [f64; 3]) -> State {
        let (sin, cos) = sidereal_angle(JD).sin_cos();
        let unrotate = |v: [f64; 3]| [cos * v[0] - sin * v[1], sin * v[0] + cos * v[1], v[2]];
        let carried = [
            velocity[0] - EARTH_ROTATION_RAD_S * position[1],
            velocity[1] + EARTH_ROTATION_RAD_S * position[0],
            velocity[2],
        ];
        State {
            position_km: unrotate(position),
            velocity_km_s: unrotate(carried),
        }
    }

    fn above(observer: &Observer, altitude_km: f64, climb_km_s: f64) -> State {
        let site = observer.earth_fixed();
        let norm = site.iter().map(|v| v * v).sum::<f64>().sqrt();
        let up = site.map(|v| v / norm);
        inertial(
            [0, 1, 2].map(|i| site[i] + up[i] * altitude_km),
            up.map(|v| v * climb_km_s),
        )
    }

    const EQUATOR: Observer = Observer {
        latitude_deg: 0.0,
        longitude_deg: 30.0,
        altitude_m: 0.0,
    };

    #[test]
    fn a_point_straight_up_is_at_the_zenith() {
        let look = EQUATOR.look(&above(&EQUATOR, 500.0, 0.0), JD);
        assert!(look.elevation_deg > 89.99, "{look:?}");
        assert!((look.range_km - 500.0).abs() < 1e-6, "{look:?}");
        assert!(look.range_rate_km_s.abs() < 1e-9, "{look:?}");
    }

    #[test]
    fn a_receding_satellite_opens_its_range() {
        let look = EQUATOR.look(&above(&EQUATOR, 500.0, 3.0), JD);
        assert!((look.range_rate_km_s - 3.0).abs() < 1e-9, "{look:?}");
    }

    #[test]
    fn north_of_the_site_reads_as_azimuth_zero() {
        let site = EQUATOR.earth_fixed();
        let state = inertial([site[0], site[1], site[2] + 1_000.0], [0.0; 3]);
        let look = EQUATOR.look(&state, JD);
        assert!(
            look.azimuth_deg < 1e-6 || look.azimuth_deg > 360.0 - 1e-6,
            "{look:?}"
        );
    }
}
