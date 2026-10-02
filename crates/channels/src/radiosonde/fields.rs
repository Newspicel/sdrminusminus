use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use sdrmm_wire::{RadiosondeFrame, SondeType};

pub(crate) const WGS84_A: f64 = 6_378_137.0;
pub(crate) const WGS84_B: f64 = 6_356_752.314_245_18;
pub(crate) const GPS_EPOCH_UNIX: i64 = 315_964_800;
pub(crate) const GPS_UTC_LEAP_SECONDS: i64 = 18;
pub(crate) const SECONDS_PER_WEEK: i64 = 604_800;

const MIN_ALTITUDE_M: f64 = -1_000.0;
const MAX_ALTITUDE_M: f64 = 80_000.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geodetic {
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Motion {
    pub speed_ms: f64,
    pub heading_deg: f64,
    pub climb_ms: f64,
}

impl Motion {
    pub(crate) fn from_enu(east: f64, north: f64, up: f64) -> Self {
        Self {
            speed_ms: east.hypot(north),
            heading_deg: east.atan2(north).to_degrees().rem_euclid(360.0),
            climb_ms: up,
        }
    }
}

pub(crate) fn ecef_to_geodetic(x: f64, y: f64, z: f64) -> Geodetic {
    let e2 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_A * WGS84_A);
    let ee2 = (WGS84_A * WGS84_A - WGS84_B * WGS84_B) / (WGS84_B * WGS84_B);
    let lon = y.atan2(x);
    let p = x.hypot(y);
    let t = (z * WGS84_A).atan2(p * WGS84_B);
    let lat = (z + ee2 * WGS84_B * t.sin().powi(3)).atan2(p - e2 * WGS84_A * t.cos().powi(3));
    let radius = WGS84_A / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    Geodetic {
        lat: lat.to_degrees(),
        lon: lon.to_degrees(),
        alt: p / lat.cos() - radius,
    }
}

pub(crate) fn ecef_velocity_to_enu(at: Geodetic, v: [f64; 3]) -> [f64; 3] {
    let (phi, lam) = (at.lat.to_radians(), at.lon.to_radians());
    let north = -v[0] * phi.sin() * lam.cos() - v[1] * phi.sin() * lam.sin() + v[2] * phi.cos();
    let east = -v[0] * lam.sin() + v[1] * lam.cos();
    let up = v[0] * phi.cos() * lam.cos() + v[1] * phi.cos() * lam.sin() + v[2] * phi.sin();
    [east, north, up]
}

pub(crate) fn plausible(position: Geodetic) -> bool {
    position.lat.is_finite()
        && position.lon.is_finite()
        && position.lat.abs() <= 90.0
        && position.lon.abs() <= 180.0
        && (MIN_ALTITUDE_M..=MAX_ALTITUDE_M).contains(&position.alt)
}

pub(crate) fn gps_time(week: i64, seconds_of_week: i64, leap_seconds: i64) -> Option<String> {
    let unix = GPS_EPOCH_UNIX + week * SECONDS_PER_WEEK + seconds_of_week - leap_seconds;
    let stamp = Timestamp::from_second(unix).ok()?;
    Some(stamp.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
}

pub(crate) fn civil_time(
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    second: i8,
) -> Option<String> {
    let civil = DateTime::new(year, month, day, hour, minute, second, 0).ok()?;
    let stamp = civil.to_zoned(TimeZone::UTC).ok()?.timestamp();
    Some(stamp.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
}

pub(crate) fn today_at(hour: u8, minute: u8, second: u8) -> Option<String> {
    let now = Timestamp::now().to_zoned(TimeZone::UTC);
    let seconds_now = i64::from(now.hour()) * 3_600 + i64::from(now.minute()) * 60;
    let seconds_then = i64::from(hour) * 3_600 + i64::from(minute) * 60;
    let shift_days = match seconds_then - seconds_now {
        gap if gap > 43_200 => -1,
        gap if gap < -43_200 => 1,
        _ => 0,
    };
    let date = now
        .date()
        .checked_add(jiff::Span::new().days(shift_days))
        .ok()?;
    civil_time(
        date.year(),
        date.month(),
        date.day(),
        i8::try_from(hour).ok()?,
        i8::try_from(minute).ok()?,
        i8::try_from(second).ok()?,
    )
}

pub(crate) fn empty_frame(sonde: SondeType, serial: String) -> RadiosondeFrame {
    RadiosondeFrame {
        sonde,
        serial,
        frame: None,
        time: None,
        lat: None,
        lon: None,
        altitude_m: None,
        speed_ms: None,
        heading_deg: None,
        climb_ms: None,
        temperature_c: None,
        humidity_pct: None,
        pressure_hpa: None,
        satellites: None,
        battery_v: None,
        errors_corrected: 0,
        rejected: 0,
    }
}

pub(crate) fn set_position(frame: &mut RadiosondeFrame, position: Geodetic) {
    frame.lat = Some(position.lat);
    frame.lon = Some(position.lon);
    frame.altitude_m = Some(position.alt);
}

pub(crate) fn set_motion(frame: &mut RadiosondeFrame, motion: Motion) {
    frame.speed_ms = Some(motion.speed_ms);
    frame.heading_deg = Some(motion.heading_deg);
    frame.climb_ms = Some(motion.climb_ms);
}

pub(crate) fn steinhart_hart(coefficients: [f64; 4], resistance: f64) -> Option<f64> {
    if !(resistance.is_finite() && resistance > 0.0) {
        return None;
    }
    let ln = resistance.ln();
    let inverse = coefficients[0]
        + coefficients[1] * ln
        + coefficients[2] * ln * ln
        + coefficients[3] * ln * ln * ln;
    let kelvin = 1.0 / inverse;
    kelvin.is_finite().then_some(kelvin - 273.15)
}

pub(crate) fn finite_f32(value: f64) -> Option<f32> {
    value.is_finite().then_some(value as f32)
}
