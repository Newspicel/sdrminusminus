use std::{cell::RefCell, collections::HashMap};

use crate::{
    socket::Spectrum,
    ui::{
        kit_maps::iso_of,
        map::{
            Geo,
            heat::Ramp,
            overlay::{Dot, EDGE, Heat, Overlay},
        },
    },
};

pub const SIGNAL_MIN_DBFS: f64 = -120.0;
pub const SIGNAL_MAX_DBFS: f64 = -20.0;
pub const RAMP_CSS: &str = "linear-gradient(to right, #231942, #5e2b83, #b33f62, #ef8354, #f6d365)";
const CELL_SIZE_M: f64 = 10.0;
const MAX_CELLS: usize = 5_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub at: Geo,
    pub frequency_hz: f64,
    pub level_dbfs: f64,
    pub measured_at: i64,
    pub observations: u32,
    pub accuracy_m: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub recording: bool,
    pub samples: Vec<Sample>,
}

thread_local! {
    static SESSIONS: RefCell<HashMap<String, Session>> = RefCell::new(HashMap::new());
}

#[must_use]
pub fn session_of(node: &str) -> Session {
    SESSIONS.with(|sessions| sessions.borrow().get(node).cloned().unwrap_or_default())
}

pub fn keep(node: &str, session: Session) {
    SESSIONS.with(|sessions| {
        sessions.borrow_mut().insert(node.to_owned(), session);
    });
}

#[must_use]
pub fn measure(frame: &Spectrum, frequency_hz: f64, bandwidth_hz: f64) -> Option<f64> {
    let count = frame.bins.len();
    let span = f64::from(frame.span_hz);
    let (low, high) = (f64::from(frame.db_min), f64::from(frame.db_max));
    let usable = span > 0.0 && high > low && frequency_hz.is_finite() && bandwidth_hz > 0.0;
    if count == 0 || !usable {
        return None;
    }
    let frame_low = frame.center_hz - span / 2.0;
    let frame_high = frame.center_hz + span / 2.0;
    if frequency_hz < frame_low || frequency_hz > frame_high {
        return None;
    }
    let bin_hz = span / count as f64;
    let slice_low = frame_low.max(frequency_hz - bandwidth_hz / 2.0);
    let slice_high = frame_high.min(frequency_hz + bandwidth_hz / 2.0);
    let last_bin = count as i64 - 1;
    let first = (((slice_low - frame_low) / bin_hz).floor() as i64).clamp(0, last_bin);
    let last = ((((slice_high - frame_low) / bin_hz).ceil() as i64) - 1)
        .clamp(0, last_bin)
        .max(first);
    let peak = frame.bins[first as usize..=last as usize]
        .iter()
        .copied()
        .max()
        .unwrap_or(0);
    Some(low + f64::from(peak) / 255.0 * (high - low))
}

#[must_use]
pub fn offset_limit_hz(span_hz: f64, bandwidth_hz: f64) -> f64 {
    if !span_hz.is_finite() || !bandwidth_hz.is_finite() {
        return 0.0;
    }
    ((span_hz - bandwidth_hz) / 2.0).floor().max(0.0)
}

fn cell_key(at: Geo, frequency_hz: f64) -> (i64, i64, i64) {
    let x = at.lon * 111_320.0 * at.lat.to_radians().cos().max(0.01);
    let y = at.lat * 110_540.0;
    (
        frequency_hz.round() as i64,
        (x / CELL_SIZE_M).round() as i64,
        (y / CELL_SIZE_M).round() as i64,
    )
}

fn power(db: f64) -> f64 {
    10f64.powf(db / 10.0)
}

pub fn merge(samples: &mut Vec<Sample>, incoming: Sample) {
    let key = cell_key(incoming.at, incoming.frequency_hz);
    let Some(previous) = samples
        .iter_mut()
        .find(|sample| cell_key(sample.at, sample.frequency_hz) == key)
    else {
        samples.push(Sample {
            observations: 1,
            ..incoming
        });
        let overflow = samples.len().saturating_sub(MAX_CELLS);
        samples.drain(..overflow);
        return;
    };
    let seen = f64::from(previous.observations);
    let observations = previous.observations + 1;
    let count = f64::from(observations);
    let mean = (power(previous.level_dbfs) * seen + power(incoming.level_dbfs)) / count;
    *previous = Sample {
        at: Geo::new(
            (previous.at.lat * seen + incoming.at.lat) / count,
            (previous.at.lon * seen + incoming.at.lon) / count,
        ),
        frequency_hz: incoming.frequency_hz,
        level_dbfs: 10.0 * mean.log10(),
        measured_at: incoming.measured_at,
        observations,
        accuracy_m: incoming.accuracy_m,
    };
}

fn fixed(value: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    format!("{:.digits$}", (value * scale).round() / scale)
}

#[must_use]
pub fn csv(samples: &[Sample], offset_hz: i64, bandwidth_hz: u64) -> String {
    let mut out = String::from(
        "time,frequency_hz,offset_hz,bandwidth_hz,latitude,longitude,accuracy_m,level_dbfs,observations",
    );
    for sample in samples {
        let accuracy = sample
            .accuracy_m
            .map(|value| fixed(value, 1))
            .unwrap_or_default();
        out.push('\n');
        out.push_str(&format!(
            "{},{},{offset_hz},{bandwidth_hz},{},{},{accuracy},{},{}",
            iso_of(sample.measured_at),
            sample.frequency_hz.round() as i64,
            fixed(sample.at.lat, 7),
            fixed(sample.at.lon, 7),
            fixed(sample.level_dbfs, 2),
            sample.observations,
        ));
    }
    out
}

#[must_use]
pub fn status(has_fix: bool, level: Option<f64>, recording: bool, moved: bool) -> &'static str {
    if !has_fix {
        "Waiting for GPS"
    } else if level.is_none() {
        "Offset is outside the IQ span"
    } else if moved {
        "IQ centre changed: clear to start a new survey"
    } else if recording {
        "Recording each new GPS fix"
    } else {
        "Ready"
    }
}

fn level_colour(level: f64) -> u32 {
    let ramp = Ramp {
        stops: vec![
            (SIGNAL_MIN_DBFS, 0x23_19_42, 1.0),
            (-95.0, 0x5e_2b_83, 1.0),
            (-70.0, 0xb3_3f_62, 1.0),
            (-45.0, 0xef_83_54, 1.0),
            (SIGNAL_MAX_DBFS, 0xf6_d3_65, 1.0),
        ],
    };
    ramp.at(level).0
}

#[must_use]
pub fn overlay(samples: &[Sample]) -> Overlay {
    let span = SIGNAL_MAX_DBFS - SIGNAL_MIN_DBFS;
    let weight = |level: f64| 0.05 + 0.95 * ((level - SIGNAL_MIN_DBFS) / span).clamp(0.0, 1.0);
    let mut out = Overlay::default();
    if samples.is_empty() {
        return out;
    }
    out.heat.push(Heat {
        points: samples
            .iter()
            .map(|sample| (sample.at, weight(sample.level_dbfs)))
            .collect(),
        radius: vec![(0.0, 3.0), (15.0, 20.0)],
        intensity: vec![(0.0, 0.35), (15.0, 1.25)],
        opacity: vec![(13.0, 0.8), (17.0, 0.2)],
        ramp: Ramp {
            stops: vec![
                (0.0, 0x23_19_42, 0.0),
                (0.15, 0x23_19_42, 1.0),
                (0.35, 0x5e_2b_83, 1.0),
                (0.55, 0xb3_3f_62, 1.0),
                (0.75, 0xef_83_54, 1.0),
                (1.0, 0xf6_d3_65, 1.0),
            ],
        },
    });
    for sample in samples {
        out.dots.push(Dot {
            radius: vec![(13.0, 2.0), (17.0, 6.0)],
            stroke: EDGE,
            min_zoom: 13.0,
            ..Dot::plain(sample.at, 2.0, level_colour(sample.level_dbfs))
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::kit_maps::millis_of;

    fn frame() -> Spectrum {
        Spectrum {
            stream_id: 1,
            seq: 1,
            center_hz: 100_000_000.0,
            span_hz: 1_000_000.0,
            db_min: -120.0,
            db_max: -20.0,
            bins: vec![0, 51, 102, 153, 204],
        }
    }

    fn sample(lat: f64, lon: f64, frequency_hz: f64, level_dbfs: f64, measured_at: i64) -> Sample {
        Sample {
            at: Geo::new(lat, lon),
            frequency_hz,
            level_dbfs,
            measured_at,
            observations: 0,
            accuracy_m: None,
        }
    }

    #[test]
    fn the_peak_bin_inside_the_bandwidth_is_taken() {
        let level = measure(&frame(), 100_000_000.0, 200_000.0).expect("a level");
        assert!((level + 80.0).abs() < 1e-6);
        let level = measure(&frame(), 100_300_000.0, 400_000.0).expect("a level");
        assert!((level + 40.0).abs() < 1e-6);
    }

    #[test]
    fn a_target_outside_the_span_and_an_empty_frame_are_refused() {
        assert!(measure(&frame(), 101_000_000.0, 12_500.0).is_none());
        let empty = Spectrum {
            bins: Vec::new(),
            ..frame()
        };
        assert!(measure(&empty, 100_000_000.0, 12_500.0).is_none());
    }

    #[test]
    fn the_offset_keeps_the_whole_width_inside_the_span() {
        assert_eq!(offset_limit_hz(1_000_000.0, 12_500.0), 493_750.0);
        assert_eq!(offset_limit_hz(10_000.0, 12_500.0), 0.0);
    }

    #[test]
    fn repeated_locations_average_in_power_not_in_decibels() {
        let mut samples = Vec::new();
        merge(&mut samples, sample(52.52, 13.405, 145_500_000.0, -40.0, 1));
        merge(
            &mut samples,
            Sample {
                accuracy_m: Some(3.0),
                ..sample(52.520_001, 13.405_001, 145_500_000.0, -60.0, 2)
            },
        );
        assert_eq!(samples.len(), 1);
        assert!((samples[0].level_dbfs + 42.967).abs() < 1e-3);
        assert_eq!(samples[0].measured_at, 2);
        assert_eq!(samples[0].observations, 2);
        assert_eq!(samples[0].accuracy_m, Some(3.0));
    }

    #[test]
    fn one_place_at_two_frequencies_is_two_cells() {
        let mut samples = Vec::new();
        merge(&mut samples, sample(52.52, 13.405, 145_500_000.0, -40.0, 1));
        merge(&mut samples, sample(52.52, 13.405, 145_525_000.0, -41.0, 2));
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn the_export_carries_units_and_observation_counts() {
        let exported = csv(
            &[Sample {
                observations: 3,
                accuracy_m: Some(4.25),
                ..sample(
                    52.52,
                    13.405,
                    145_500_000.0,
                    -67.125,
                    millis_of("2026-08-15T10:00:00Z").expect("a time"),
                )
            }],
            -25_000,
            12_500,
        );
        assert!(exported.contains("frequency_hz,offset_hz,bandwidth_hz"));
        assert!(
            exported.contains(
                "2026-08-15T10:00:00.000Z,145500000,-25000,12500,52.5200000,13.4050000,4.3,-67.13,3"
            ),
            "{exported}"
        );
    }

    #[test]
    fn the_status_names_what_is_missing_first() {
        assert_eq!(status(false, None, false, false), "Waiting for GPS");
        assert_eq!(
            status(true, None, false, false),
            "Offset is outside the IQ span"
        );
        assert_eq!(
            status(true, Some(-50.0), true, true),
            "IQ centre changed: clear to start a new survey"
        );
        assert_eq!(
            status(true, Some(-50.0), true, false),
            "Recording each new GPS fix"
        );
        assert_eq!(status(true, Some(-50.0), false, false), "Ready");
    }

    #[test]
    fn a_survey_draws_a_heatmap_and_its_cells() {
        let drawn = overlay(&[Sample {
            observations: 1,
            ..sample(52.52, 13.405, 145_500_000.0, -50.0, 1)
        }]);
        assert_eq!(drawn.heat.len(), 1);
        assert_eq!(drawn.dots.len(), 1);
        assert!(overlay(&[]).heat.is_empty());
    }
}
