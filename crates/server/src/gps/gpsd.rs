use std::time::{Duration, Instant};

use sdrmm_wire::{Attitude, HeadingSource, PositionFix, normalize_heading};
use serde::Deserialize;

const ATTITUDE_MAX_AGE: Duration = Duration::from_secs(2);

#[derive(Deserialize)]
struct GpsdReport {
    class: String,
    mode: Option<u8>,
    lat: Option<f64>,
    lon: Option<f64>,
    alt: Option<f64>,
    epx: Option<f64>,
    epy: Option<f64>,
    speed: Option<f64>,
    track: Option<f64>,
    time: Option<String>,
    heading: Option<f64>,
    pitch: Option<f64>,
    roll: Option<f64>,
    mag_st: Option<String>,
}

#[derive(Debug, PartialEq)]
pub(super) enum GpsdOutcome {
    Fix(PositionFix),
    NoFix,
    Nothing,
}

#[derive(Default)]
pub(super) struct GpsdState {
    last_fix: Option<PositionFix>,
    attitude: Option<(Attitude, Instant)>,
}

impl GpsdState {
    pub(super) fn line(&mut self, line: &str, now: Instant) -> GpsdOutcome {
        let Ok(report) = serde_json::from_str::<GpsdReport>(line) else {
            return GpsdOutcome::Nothing;
        };
        match report.class.as_str() {
            "TPV" => self.position(&report, now),
            "ATT" => self.turn(&report, now),
            _ => GpsdOutcome::Nothing,
        }
    }

    fn position(&mut self, report: &GpsdReport, now: Instant) -> GpsdOutcome {
        if report.mode.unwrap_or_default() < 2 {
            self.last_fix = None;
            return GpsdOutcome::NoFix;
        }
        let (Some(latitude), Some(longitude)) = (report.lat, report.lon) else {
            return GpsdOutcome::Nothing;
        };
        let fix = PositionFix {
            latitude,
            longitude,
            altitude_m: report.alt,
            accuracy_m: report.epx.into_iter().chain(report.epy).reduce(f64::max),
            speed_mps: report.speed,
            track_deg: report.track,
            time: report.time.clone().unwrap_or_else(super::now),
            attitude: self.attitude(now),
        };
        if fix.validate().is_err() {
            return GpsdOutcome::Nothing;
        }
        self.last_fix = Some(fix.clone());
        GpsdOutcome::Fix(fix)
    }

    fn turn(&mut self, report: &GpsdReport, now: Instant) -> GpsdOutcome {
        let source = if report.mag_st.is_some() {
            HeadingSource::Compass
        } else {
            HeadingSource::Sensor
        };
        let attitude = Attitude {
            heading_deg: report.heading.map(normalize_heading),
            heading_source: report.heading.map(|_| source),
            pitch_deg: report.pitch,
            roll_deg: report.roll,
            ..Attitude::default()
        };
        if let Err(problem) = attitude.validate() {
            tracing::debug!(problem, "gpsd attitude dropped");
            return GpsdOutcome::Nothing;
        }
        self.attitude = Some((attitude, now));
        match &mut self.last_fix {
            Some(fix) => {
                fix.attitude = attitude;
                GpsdOutcome::Fix(fix.clone())
            }
            None => GpsdOutcome::Nothing,
        }
    }

    fn attitude(&self, now: Instant) -> Attitude {
        match self.attitude {
            Some((attitude, at)) if now.saturating_duration_since(at) < ATTITUDE_MAX_AGE => {
                attitude
            }
            _ => Attitude::default(),
        }
    }
}
