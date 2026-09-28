pub const WGS84_A_M: f64 = 6_378_137.0;
pub const WGS84_F: f64 = 1.0 / 298.257_223_563;

const WGS84_E2: f64 = WGS84_F * (2.0 - WGS84_F);
const LATITUDE_STEPS: usize = 16;
const LATITUDE_TOLERANCE_RAD: f64 = 1e-15;
const ALTITUDE_STEPS: usize = 8;
const ALTITUDE_TOLERANCE_M: f64 = 1e-9;
const BISECTION_STEPS: usize = 60;
const BRACKET_DOUBLINGS: usize = 8;
const RANGE_STEP_M: f64 = 1.0;
const BEARING_STEP_DEG: f64 = 0.01;

type Vec3 = [f64; 3];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Geodetic {
    pub lat_deg: f64,
    pub lon_deg: f64,
    pub alt_m: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Located {
    pub point: Geodetic,
    pub major_m: f64,
    pub minor_m: f64,
    pub orientation_deg: f64,
}

struct Frame {
    east: Vec3,
    north: Vec3,
    up: Vec3,
}

impl Frame {
    fn at(origin: Geodetic) -> Self {
        let (sin_lat, cos_lat) = origin.lat_deg.to_radians().sin_cos();
        let (sin_lon, cos_lon) = origin.lon_deg.to_radians().sin_cos();
        Self {
            east: [-sin_lon, cos_lon, 0.0],
            north: [-sin_lat * cos_lon, -sin_lat * sin_lon, cos_lat],
            up: [cos_lat * cos_lon, cos_lat * sin_lon, sin_lat],
        }
    }
}

fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: Vec3, factor: f64) -> Vec3 {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

fn unit(a: Vec3) -> Vec3 {
    let length = norm(a);
    if length > 0.0 {
        scale(a, 1.0 / length)
    } else {
        [0.0; 3]
    }
}

fn prime_vertical_m(sin_lat: f64) -> f64 {
    WGS84_A_M / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt()
}

#[must_use]
pub fn ecef(point: Geodetic) -> Vec3 {
    let (sin_lat, cos_lat) = point.lat_deg.to_radians().sin_cos();
    let (sin_lon, cos_lon) = point.lon_deg.to_radians().sin_cos();
    let n = prime_vertical_m(sin_lat);
    let horizontal = (n + point.alt_m) * cos_lat;
    [
        horizontal * cos_lon,
        horizontal * sin_lon,
        (n * (1.0 - WGS84_E2) + point.alt_m) * sin_lat,
    ]
}

fn geodetic(point: Vec3) -> Geodetic {
    let [x, y, z] = point;
    let p = x.hypot(y);
    let mut lat = z.atan2(p * (1.0 - WGS84_E2));
    for _ in 0..LATITUDE_STEPS {
        let sin_lat = lat.sin();
        let next = (z + WGS84_E2 * prime_vertical_m(sin_lat) * sin_lat).atan2(p);
        let settled = (next - lat).abs() < LATITUDE_TOLERANCE_RAD;
        lat = next;
        if settled {
            break;
        }
    }
    let (sin_lat, cos_lat) = lat.sin_cos();
    Geodetic {
        lat_deg: lat.to_degrees(),
        lon_deg: y.atan2(x).to_degrees(),
        alt_m: p * cos_lat + z * sin_lat - WGS84_A_M * (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt(),
    }
}

#[must_use]
pub fn enu(origin: Geodetic, point: Vec3) -> Vec3 {
    let frame = Frame::at(origin);
    let offset = sub(point, ecef(origin));
    [
        dot(frame.east, offset),
        dot(frame.north, offset),
        dot(frame.up, offset),
    ]
}

#[must_use]
pub fn enu_vector_to_ecef(origin: Geodetic, v: Vec3) -> Vec3 {
    let frame = Frame::at(origin);
    add(
        add(scale(frame.east, v[0]), scale(frame.north, v[1])),
        scale(frame.up, v[2]),
    )
}

#[must_use]
pub fn geodetic_from_enu(origin: Geodetic, enu: Vec3) -> Geodetic {
    geodetic(add(ecef(origin), enu_vector_to_ecef(origin, enu)))
}

#[must_use]
pub fn bistatic_range_m(tx: Vec3, rx: Vec3, target: Vec3) -> f64 {
    norm(sub(target, tx)) + norm(sub(target, rx)) - norm(sub(tx, rx))
}

#[must_use]
pub fn bistatic_rate_mps(tx: Vec3, rx: Vec3, target: Vec3, velocity: Vec3) -> f64 {
    dot(velocity, add(unit(sub(target, tx)), unit(sub(target, rx))))
}

#[must_use]
pub fn bearing_deg(from: Geodetic, to: Geodetic) -> f64 {
    let [east, north, _] = enu(from, ecef(to));
    east.atan2(north).to_degrees().rem_euclid(360.0)
}

#[must_use]
pub fn locate(
    rx: Geodetic,
    tx: Geodetic,
    bistatic_range_m: f64,
    bearing_deg: f64,
    target_alt_m: f64,
) -> Option<Geodetic> {
    let usable = bistatic_range_m > 0.0
        && [bistatic_range_m, bearing_deg, target_alt_m]
            .into_iter()
            .chain(finite_parts(rx))
            .chain(finite_parts(tx))
            .all(f64::is_finite);
    if !usable {
        return None;
    }
    let transmitter = enu(rx, ecef(tx));
    let path_sum = bistatic_range_m + norm(transmitter);
    let heading = bearing_deg.to_radians().sin_cos();
    let path = |ground: f64| {
        let target = along(rx, heading, ground, target_alt_m);
        norm(target) + norm(sub(target, transmitter))
    };
    if path(0.0) >= path_sum {
        return None;
    }
    let mut high = path_sum;
    let mut doublings = 0;
    while path(high) < path_sum {
        if doublings == BRACKET_DOUBLINGS {
            return None;
        }
        high *= 2.0;
        doublings += 1;
    }
    let mut low = 0.0;
    for _ in 0..BISECTION_STEPS {
        let middle = 0.5 * (low + high);
        if path(middle) < path_sum {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(geodetic_from_enu(
        rx,
        along(rx, heading, 0.5 * (low + high), target_alt_m),
    ))
}

#[must_use]
pub fn locate_with_sigma(
    rx: Geodetic,
    tx: Geodetic,
    bistatic_range_m: f64,
    bearing_deg: f64,
    target_alt_m: f64,
    sigma_range_m: f64,
    sigma_bearing_deg: f64,
) -> Option<Located> {
    let sigmas_usable = [sigma_range_m, sigma_bearing_deg]
        .into_iter()
        .all(|sigma| sigma >= 0.0 && sigma.is_finite());
    if !sigmas_usable {
        return None;
    }
    let point = locate(rx, tx, bistatic_range_m, bearing_deg, target_alt_m)?;
    let longer = locate(
        rx,
        tx,
        bistatic_range_m + RANGE_STEP_M,
        bearing_deg,
        target_alt_m,
    )?;
    let turned = locate(
        rx,
        tx,
        bistatic_range_m,
        bearing_deg + BEARING_STEP_DEG,
        target_alt_m,
    )?;
    let center = enu(rx, ecef(point));
    let by_range = scale(sub(enu(rx, ecef(longer)), center), 1.0 / RANGE_STEP_M);
    let by_bearing = scale(sub(enu(rx, ecef(turned)), center), 1.0 / BEARING_STEP_DEG);
    let (range_var, bearing_var) = (sigma_range_m.powi(2), sigma_bearing_deg.powi(2));
    let east = by_range[0].powi(2) * range_var + by_bearing[0].powi(2) * bearing_var;
    let north = by_range[1].powi(2) * range_var + by_bearing[1].powi(2) * bearing_var;
    let cross = by_range[0] * by_range[1] * range_var + by_bearing[0] * by_bearing[1] * bearing_var;
    Some(ellipse(point, east, north, cross))
}

fn ellipse(point: Geodetic, east: f64, north: f64, cross: f64) -> Located {
    let mean = 0.5 * (east + north);
    let radius = (0.5 * (east - north)).hypot(cross);
    let from_east = 0.5 * (2.0 * cross).atan2(east - north);
    Located {
        point,
        major_m: (mean + radius).max(0.0).sqrt(),
        minor_m: (mean - radius).max(0.0).sqrt(),
        orientation_deg: (90.0 - from_east.to_degrees()).rem_euclid(180.0),
    }
}

fn finite_parts(point: Geodetic) -> [f64; 3] {
    [point.lat_deg, point.lon_deg, point.alt_m]
}

fn along(rx: Geodetic, heading: (f64, f64), ground: f64, target_alt_m: f64) -> Vec3 {
    let (sin, cos) = heading;
    let mut target = [ground * sin, ground * cos, target_alt_m - rx.alt_m];
    for _ in 0..ALTITUDE_STEPS {
        let error = geodetic_from_enu(rx, target).alt_m - target_alt_m;
        target[2] -= error;
        if error.abs() < ALTITUDE_TOLERANCE_M {
            break;
        }
    }
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    const MUNICH: Geodetic = Geodetic {
        lat_deg: 48.137,
        lon_deg: 11.575,
        alt_m: 519.0,
    };

    fn at(origin: Geodetic, east: f64, north: f64, alt_m: f64) -> Geodetic {
        Geodetic {
            alt_m,
            ..geodetic_from_enu(origin, [east, north, 0.0])
        }
    }

    fn distance(a: Geodetic, b: Geodetic) -> f64 {
        norm(sub(ecef(a), ecef(b)))
    }

    fn horizontal_error(truth: Geodetic, got: Geodetic) -> f64 {
        let [east, north, _] = enu(truth, ecef(got));
        east.hypot(north)
    }

    #[test]
    fn ecef_round_trips_through_enu() {
        let origin_equator = ecef(Geodetic::default());
        assert!((origin_equator[0] - WGS84_A_M).abs() < 1e-9);
        let pole = ecef(Geodetic {
            lat_deg: 90.0,
            lon_deg: 0.0,
            alt_m: 0.0,
        });
        assert!((pole[2] - WGS84_A_M * (1.0 - WGS84_F)).abs() < 1e-6);
        let origins = [
            MUNICH,
            Geodetic::default(),
            Geodetic {
                lat_deg: -33.87,
                lon_deg: 151.21,
                alt_m: 30.0,
            },
            Geodetic {
                lat_deg: 89.95,
                lon_deg: -40.0,
                alt_m: 2_000.0,
            },
            Geodetic {
                lat_deg: -60.0,
                lon_deg: -179.9,
                alt_m: -20.0,
            },
        ];
        let offsets = [
            [0.0, 0.0, 0.0],
            [1_000.0, -2_000.0, 50.0],
            [-30_000.0, 45_000.0, 10_000.0],
            [150_000.0, 80_000.0, -200.0],
        ];
        for origin in origins {
            for offset in offsets {
                let point = geodetic_from_enu(origin, offset);
                let back = enu(origin, ecef(point));
                assert!(
                    norm(sub(back, offset)) < 1e-6,
                    "{origin:?} {offset:?}: {back:?}"
                );
                let again = geodetic_from_enu(origin, enu(origin, ecef(point)));
                assert!((again.lat_deg - point.lat_deg).abs() < 1e-10);
                assert!((again.lon_deg - point.lon_deg).abs() < 1e-10);
                assert!((again.alt_m - point.alt_m).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn enu_vectors_rotate_back() {
        let equator = Geodetic::default();
        let east = enu_vector_to_ecef(equator, [1.0, 0.0, 0.0]);
        let up = enu_vector_to_ecef(equator, [0.0, 0.0, 1.0]);
        assert!(norm(sub(east, [0.0, 1.0, 0.0])) < 1e-12);
        assert!(norm(sub(up, [1.0, 0.0, 0.0])) < 1e-12);
        let v = [3.0, -4.0, 12.0];
        let rotated = enu_vector_to_ecef(MUNICH, v);
        assert!((norm(rotated) - 13.0).abs() < 1e-12);
        let back = enu(MUNICH, add(ecef(MUNICH), rotated));
        assert!(norm(sub(back, v)) < 1e-6);
    }

    #[test]
    fn bearing_follows_the_compass() {
        for (east, north, expected) in [
            (0.0, 5_000.0, 0.0),
            (5_000.0, 0.0, 90.0),
            (0.0, -5_000.0, 180.0),
            (-5_000.0, 0.0, 270.0),
            (3_000.0, 3_000.0, 45.0),
        ] {
            let target = geodetic_from_enu(MUNICH, [east, north, 0.0]);
            let got = bearing_deg(MUNICH, target);
            let diff = (got - expected).rem_euclid(360.0);
            assert!(diff.min(360.0 - diff) < 1e-6, "{east}, {north}: {got}");
            assert!((0.0..360.0).contains(&got));
        }
    }

    #[test]
    fn range_is_zero_on_the_baseline() {
        let rx = ecef(MUNICH);
        let tx = ecef(at(MUNICH, 25_000.0, 12_000.0, 900.0));
        for fraction in [0.0, 0.3, 0.77, 1.0] {
            let target = add(rx, scale(sub(tx, rx), fraction));
            assert!(bistatic_range_m(tx, rx, target).abs() < 1e-6);
        }
        let off = ecef(at(MUNICH, 10_000.0, -8_000.0, 3_000.0));
        assert!(bistatic_range_m(tx, rx, off) > 1_000.0);
    }

    #[test]
    fn rate_matches_the_numeric_derivative() {
        let rx = ecef(MUNICH);
        let tx = ecef(at(MUNICH, 40_000.0, -5_000.0, 300.0));
        let dt = 1e-3;
        for (east, north, up, velocity) in [
            (10_000.0, 20_000.0, 9_000.0, [220.0, -40.0, 5.0]),
            (-35_000.0, 3_000.0, 11_000.0, [-100.0, 180.0, -12.0]),
            (60_000.0, -1_000.0, 2_000.0, [0.0, 0.0, 30.0]),
        ] {
            let site = at(MUNICH, east, north, up);
            let target = ecef(site);
            let v = enu_vector_to_ecef(site, velocity);
            let ahead = bistatic_range_m(tx, rx, add(target, scale(v, dt)));
            let behind = bistatic_range_m(tx, rx, sub(target, scale(v, dt)));
            let numeric = (ahead - behind) / (2.0 * dt);
            let got = bistatic_rate_mps(tx, rx, target, v);
            assert!(
                (got - numeric).abs() < 1e-5 * numeric.abs().max(1.0),
                "{got} vs {numeric}"
            );
        }
    }

    #[test]
    fn ellipse_and_bearing_meet_at_the_target() {
        let tx = at(MUNICH, 26_000.0, 15_000.0, 800.0);
        let (rx_ecef, tx_ecef) = (ecef(MUNICH), ecef(tx));
        let mut found_count = 0;
        for (dlat, dlon) in [
            (0.05, 0.02),
            (0.3, -0.4),
            (-0.2, 0.6),
            (-0.5, -0.1),
            (0.01, 0.45),
        ] {
            for alt_m in [300.0, 3_000.0, 11_000.0] {
                let target = Geodetic {
                    lat_deg: MUNICH.lat_deg + dlat,
                    lon_deg: MUNICH.lon_deg + dlon,
                    alt_m,
                };
                let range = bistatic_range_m(tx_ecef, rx_ecef, ecef(target));
                let bearing = bearing_deg(MUNICH, target);
                match locate(MUNICH, tx, range, bearing, alt_m) {
                    Some(found) => {
                        found_count += 1;
                        assert!(
                            distance(found, target) < 1.0,
                            "{dlat}, {dlon}, {alt_m}: {} m off",
                            distance(found, target)
                        );
                    }
                    None => {
                        let overhead = ecef(at(MUNICH, 0.0, 0.0, alt_m));
                        let overhead_range = bistatic_range_m(tx_ecef, rx_ecef, overhead);
                        assert!(
                            overhead_range >= range,
                            "{dlat}, {dlon}, {alt_m}: no fix although the ray is unambiguous"
                        );
                    }
                }
            }
        }
        assert!(found_count >= 12, "only {found_count} fixes");
    }

    #[test]
    fn no_fix_for_non_positive_range() {
        let tx = at(MUNICH, 30_000.0, 0.0, 500.0);
        for range in [0.0, -250.0, f64::NAN, f64::INFINITY] {
            assert_eq!(locate(MUNICH, tx, range, 45.0, 1_000.0), None);
            assert_eq!(
                locate_with_sigma(MUNICH, tx, range, 45.0, 1_000.0, 100.0, 1.0),
                None
            );
        }
        assert_eq!(locate(MUNICH, tx, 5_000.0, f64::NAN, 1_000.0), None);
        assert_eq!(locate(MUNICH, tx, 1_000.0, 90.0, 20_000.0), None);
        assert_eq!(
            locate_with_sigma(MUNICH, tx, 5_000.0, 45.0, 1_000.0, -1.0, 1.0),
            None
        );
    }

    #[test]
    fn sigma_ellipse_grows_with_bearing_sigma() {
        let tx = at(MUNICH, 30_000.0, 0.0, 500.0);
        let fix = |sigma_bearing| {
            locate_with_sigma(MUNICH, tx, 25_000.0, 20.0, 3_000.0, 150.0, sigma_bearing).unwrap()
        };
        let [tight, wide, wider] = [fix(0.5), fix(2.0), fix(6.0)];
        assert!(tight.major_m < wide.major_m && wide.major_m < wider.major_m);
        assert!(wider.major_m > 2.0 * wide.major_m);
        for located in [tight, wide, wider] {
            assert!(located.minor_m >= 0.0 && located.minor_m <= located.major_m);
            assert!((0.0..180.0).contains(&located.orientation_deg));
        }
        let point_only = locate(MUNICH, tx, 25_000.0, 20.0, 3_000.0).unwrap();
        assert_eq!(tight.point, point_only);
        let target = enu(MUNICH, ecef(point_only));
        let transmitter = enu(MUNICH, ecef(tx));
        let normal = add(unit(target), unit(sub(target, transmitter)));
        let contour = (normal[0].atan2(normal[1]).to_degrees() + 90.0).rem_euclid(180.0);
        let skew = (wider.orientation_deg - contour).abs();
        assert!(
            skew.min(180.0 - skew) < 3.0,
            "{wider:?} vs iso-range contour {contour}"
        );
        let still = locate_with_sigma(MUNICH, tx, 25_000.0, 20.0, 3_000.0, 0.0, 0.0).unwrap();
        assert!(still.major_m < 1e-9);
    }

    #[test]
    fn an_aircraft_at_10_km_altitude_is_located_within_200_m() {
        let tx = geodetic_from_enu(MUNICH, [30_000.0, 0.0, 0.0]);
        let aircraft = geodetic_from_enu(MUNICH, [0.0, 20_000.0, 10_000.0]);
        let range = bistatic_range_m(ecef(tx), ecef(MUNICH), ecef(aircraft));
        assert!((range - 29_777.3).abs() < 1.0, "range {range}");
        let bearing = bearing_deg(MUNICH, aircraft);
        let found = locate(MUNICH, tx, range, bearing, aircraft.alt_m).unwrap();
        assert!(horizontal_error(aircraft, found) < 200.0);
        let flat = locate(MUNICH, tx, range, bearing, MUNICH.alt_m).unwrap();
        assert!(horizontal_error(aircraft, flat) > 1_000.0);
    }
}
