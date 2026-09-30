use std::time::{Duration, Instant};

use sdrmm_wire::{Attitude, HeadingSource, PositionFix, normalize_heading};

const HEADING_MAX_AGE: Duration = Duration::from_secs(2);
const KNOTS_TO_MPS: f64 = 0.514_444;
const GNSS_TALKERS: [&str; 7] = ["GP", "GN", "GA", "GB", "GL", "GQ", "GI"];
const COMPASS_TALKER: &str = "HC";
const THS_VALID_MODES: [&str; 4] = ["A", "E", "M", "S"];

#[derive(Default)]
pub(super) struct NmeaState {
    latitude: Option<f64>,
    longitude: Option<f64>,
    altitude_m: Option<f64>,
    speed_mps: Option<f64>,
    track_deg: Option<f64>,
    heading: Option<(f64, HeadingSource, Instant)>,
}

enum Heading {
    Set(f64),
    Cleared,
}

impl NmeaState {
    pub(super) fn parse(&mut self, sentence: &str, now: Instant) -> Option<PositionFix> {
        let body = checked_nmea(sentence)?;
        let fields: Vec<&str> = body.split(',').collect();
        let address = fields.first()?;
        let talker = address.get(..2)?;
        match address.get(2..)? {
            "GGA" => self.gga(&fields)?,
            "RMC" => self.rmc(&fields)?,
            "HDT" => self.take_heading(hdt(&fields)?, talker, now),
            "THS" => self.take_heading(ths(&fields)?, talker, now),
            _ => return None,
        }
        self.fix(now)
    }

    fn gga(&mut self, fields: &[&str]) -> Option<()> {
        if fields.get(6)?.parse::<u8>().ok()? == 0 {
            return None;
        }
        self.latitude = nmea_coordinate(fields.get(2)?, fields.get(3)?, false);
        self.longitude = nmea_coordinate(fields.get(4)?, fields.get(5)?, true);
        self.altitude_m = fields.get(9).and_then(|value| value.parse().ok());
        Some(())
    }

    fn rmc(&mut self, fields: &[&str]) -> Option<()> {
        if *fields.get(2)? != "A" {
            return None;
        }
        self.latitude = nmea_coordinate(fields.get(3)?, fields.get(4)?, false);
        self.longitude = nmea_coordinate(fields.get(5)?, fields.get(6)?, true);
        self.speed_mps = fields
            .get(7)
            .and_then(|value| value.parse::<f64>().ok())
            .map(|knots| knots * KNOTS_TO_MPS);
        self.track_deg = fields.get(8).and_then(|value| value.parse().ok());
        Some(())
    }

    fn take_heading(&mut self, heading: Heading, talker: &str, now: Instant) {
        self.heading = match heading {
            Heading::Set(deg) => Some((normalize_heading(deg), heading_source(talker), now)),
            Heading::Cleared => None,
        };
    }

    fn attitude(&self, now: Instant) -> Attitude {
        match self.heading {
            Some((deg, source, at)) if now.saturating_duration_since(at) < HEADING_MAX_AGE => {
                Attitude {
                    heading_deg: Some(deg),
                    heading_source: Some(source),
                    ..Attitude::default()
                }
            }
            _ => Attitude::default(),
        }
    }

    fn fix(&self, now: Instant) -> Option<PositionFix> {
        let fix = PositionFix {
            latitude: self.latitude?,
            longitude: self.longitude?,
            altitude_m: self.altitude_m,
            accuracy_m: None,
            speed_mps: self.speed_mps,
            track_deg: self.track_deg,
            time: super::now(),
            attitude: self.attitude(now),
        };
        fix.validate().ok()?;
        Some(fix)
    }
}

fn hdt(fields: &[&str]) -> Option<Heading> {
    let degrees = *fields.get(1)?;
    if degrees.is_empty() {
        return Some(Heading::Cleared);
    }
    if *fields.get(2)? != "T" {
        return None;
    }
    finite(degrees).map(Heading::Set)
}

fn ths(fields: &[&str]) -> Option<Heading> {
    let degrees = *fields.get(1)?;
    let mode = *fields.get(2)?;
    if degrees.is_empty() || mode == "V" {
        return Some(Heading::Cleared);
    }
    if !THS_VALID_MODES.contains(&mode) {
        return None;
    }
    finite(degrees).map(Heading::Set)
}

fn finite(text: &str) -> Option<f64> {
    text.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn heading_source(talker: &str) -> HeadingSource {
    if GNSS_TALKERS.contains(&talker) {
        HeadingSource::Gnss
    } else if talker == COMPASS_TALKER {
        HeadingSource::Compass
    } else {
        HeadingSource::Sensor
    }
}

pub(super) fn checked_nmea(sentence: &str) -> Option<&str> {
    let sentence = sentence.trim();
    let body = sentence.strip_prefix('$')?;
    let (payload, checksum) = body.rsplit_once('*')?;
    let expected = u8::from_str_radix(checksum.get(..2)?, 16).ok()?;
    let actual = payload.bytes().fold(0, |sum, byte| sum ^ byte);
    (actual == expected).then_some(payload)
}

pub(super) fn nmea_coordinate(value: &str, hemisphere: &str, longitude: bool) -> Option<f64> {
    let degree_digits = if longitude { 3 } else { 2 };
    let degrees: f64 = value.get(..degree_digits)?.parse().ok()?;
    let minutes: f64 = value.get(degree_digits..)?.parse().ok()?;
    if !(0.0..60.0).contains(&minutes) {
        return None;
    }
    let sign = match hemisphere {
        "N" | "E" => 1.0,
        "S" | "W" => -1.0,
        _ => return None,
    };
    Some(sign * (degrees + minutes / 60.0))
}
