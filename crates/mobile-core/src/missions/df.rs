use std::collections::VecDeque;

use sdrmm_wire::{
    array::ArrayStatus,
    fusion::DfFusionState,
    geo::{self, wrap_360},
    processor::df::DfReading,
};

use super::{
    array, fusion,
    views::{DfOverlay, DfState, DfView, HeatBand, NavPoint, Ray, TargetMode},
};
use crate::{guidance, pose::PoseSnapshot, records::LatLon};

pub(crate) const RAY_LENGTH_M: f64 = 20_000.0;
pub(crate) const RAY_FADE_MS: i64 = 600_000;
pub(crate) const MAX_RAYS: usize = 200;
pub(crate) const MIN_RAY_WEIGHT: f32 = 0.05;
pub(crate) const DF_STALE_MS: i64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Bearing {
    from: LatLon,
    deg: f64,
    confidence: f32,
    t_ms: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DfDrive {
    df: Option<String>,
    freq_hz: f64,
    reading: Option<(DfReading, i64)>,
    gate: Option<DfState>,
    fusion: Option<DfFusionState>,
    heat: Vec<HeatBand>,
    history: VecDeque<Bearing>,
    pub(crate) target_mode: TargetMode,
}

impl DfDrive {
    pub(crate) fn new(df: Option<String>, freq_hz: f64, fusion: Option<DfFusionState>) -> Self {
        Self {
            df,
            freq_hz,
            reading: None,
            gate: None,
            fusion,
            heat: Vec::new(),
            history: VecDeque::with_capacity(MAX_RAYS),
            target_mode: TargetMode::Auto,
        }
    }

    pub(crate) fn reading(&mut self, reading: DfReading, pose: Option<PoseSnapshot>, now_ms: i64) {
        if reading.freq_hz > 0.0 {
            self.freq_hz = reading.freq_hz;
        }
        let peak = reading.peaks.first();
        let from = reading
            .station
            .map(LatLon::from)
            .or_else(|| pose.map(|pose| pose.at));
        if let (false, Some(peak), Some(from)) = (reading.squelched, peak, from)
            && let Some(true_deg) = peak.true_deg
        {
            if self.history.len() == MAX_RAYS {
                self.history.pop_front();
            }
            self.history.push_back(Bearing {
                from,
                deg: f64::from(true_deg),
                confidence: peak.confidence,
                t_ms: now_ms,
            });
        }
        self.reading = Some((reading, now_ms));
    }

    pub(crate) fn array(&mut self, status: &ArrayStatus) {
        if let Some(df) = &self.df {
            self.gate = array::gate(status, df);
        }
    }

    pub(crate) fn fusion(&mut self, state: DfFusionState) {
        self.fusion = Some(state);
    }

    pub(crate) fn heat(&mut self, bands: Vec<HeatBand>) {
        self.heat = bands;
    }

    pub(crate) fn target(&self) -> Option<NavPoint> {
        fusion::target(self.fusion.as_ref()?, self.target_mode)
    }

    pub(crate) fn state(&self, now_ms: i64) -> DfState {
        if self.df.is_none() {
            return match &self.fusion {
                Some(state) if state.samples > 0 => DfState::Live,
                _ => DfState::Waiting,
            };
        }
        if let Some(gate) = self.gate {
            return gate;
        }
        match &self.reading {
            None => DfState::Waiting,
            Some((_, at)) if now_ms - at > DF_STALE_MS => DfState::Waiting,
            Some((reading, _)) if reading.squelched => DfState::Squelched,
            Some((reading, _)) if reading.azimuth_deg.is_none() => DfState::NoHeading,
            Some((reading, _)) => match reading.peaks.first().and_then(|peak| peak.true_deg) {
                Some(_) => DfState::Live,
                None => DfState::NoHeading,
            },
        }
    }

    pub(crate) fn view(&self, mission: &str, pose: Option<PoseSnapshot>, now_ms: i64) -> DfView {
        let peak = self
            .reading
            .as_ref()
            .filter(|(reading, _)| !reading.squelched)
            .and_then(|(reading, _)| reading.peaks.first());
        let bearing_true_deg = peak.and_then(|peak| peak.true_deg);
        let heading = pose.and_then(|pose| pose.heading_deg);
        let target = self.target();
        DfView {
            mission: mission.to_owned(),
            state: self.state(now_ms),
            bearing_true_deg,
            bearing_rel_deg: bearing_true_deg
                .zip(heading)
                .map(|(bearing, heading)| guidance::relative(f64::from(bearing), heading) as f32),
            confidence: peak.map_or(0.0, |peak| peak.confidence),
            sigma_deg: peak.map(|peak| peak.sigma_deg),
            freq_hz: self.freq_hz,
            target_mode: self.target_mode,
            guidance: target.and_then(|target| guidance::view(target, pose)),
            target,
            estimate: self.fusion.as_ref().and_then(fusion::estimate),
            overlay: self.overlay(now_ms),
        }
    }

    fn overlay(&self, now_ms: i64) -> DfOverlay {
        let stations = self
            .fusion
            .as_ref()
            .map(fusion::stations)
            .unwrap_or_default();
        let mut rays: Vec<Ray> = self
            .fusion
            .iter()
            .flat_map(|state| state.stations.iter())
            .filter_map(|station| {
                let seen = station
                    .last_seen
                    .parse::<jiff::Timestamp>()
                    .map_or(now_ms, |at| at.as_millisecond());
                ray(
                    LatLon {
                        lat: station.lat,
                        lon: station.lon,
                    },
                    f64::from(station.last_bearing_deg),
                    1.0,
                    now_ms - seen,
                )
            })
            .collect();
        rays.extend(self.history.iter().rev().filter_map(|bearing| {
            ray(
                bearing.from,
                bearing.deg,
                bearing.confidence,
                now_ms - bearing.t_ms,
            )
        }));
        rays.truncate(MAX_RAYS);
        DfOverlay {
            rays,
            stations,
            ellipse: self
                .fusion
                .as_ref()
                .map(fusion::ellipse)
                .unwrap_or_default(),
            heat: self.heat.clone(),
        }
    }
}

fn ray(from: LatLon, deg: f64, confidence: f32, age_ms: i64) -> Option<Ray> {
    if !deg.is_finite() || !from.lat.is_finite() || !from.lon.is_finite() || age_ms >= RAY_FADE_MS {
        return None;
    }
    let fade = 1.0 - age_ms.max(0) as f32 / RAY_FADE_MS as f32;
    let confidence = if confidence.is_finite() {
        confidence.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Some(Ray {
        from,
        to: geo::destination(from.into(), wrap_360(deg), RAY_LENGTH_M).into(),
        weight: (confidence * fade).clamp(MIN_RAY_WEIGHT, 1.0),
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{array::ProcessorGate, fusion::NavTargetKind, processor::df::DfPeak};

    use super::*;
    use crate::missions::{array::tests as arrays, fusion::tests as fusions, views::GuidanceKind};

    const HOME: LatLon = LatLon {
        lat: 52.52,
        lon: 13.405,
    };

    pub(crate) fn reading(true_deg: Option<f32>, squelched: bool) -> DfReading {
        DfReading {
            peaks: vec![DfPeak {
                relative_deg: 40.0,
                true_deg,
                power_db: -20.0,
                confidence: 0.8,
                sigma_deg: 4.0,
                ..DfPeak::default()
            }],
            azimuth_deg: true_deg.map(|_| 50.0),
            station: Some(geo::LatLon {
                lat: HOME.lat,
                lon: HOME.lon,
            }),
            squelched,
            freq_hz: 433.92e6,
            ..DfReading::default()
        }
    }

    fn pose(heading_deg: Option<f64>) -> PoseSnapshot {
        PoseSnapshot {
            at: HOME,
            heading_deg,
            t_ms: 0,
        }
    }

    #[test]
    fn a_heading_gives_a_relative_bearing() {
        let mut drive = DfDrive::new(Some("df1".to_owned()), 0.0, None);
        drive.reading(reading(Some(90.0), false), Some(pose(Some(30.0))), 1_000);
        let view = drive.view("df1", Some(pose(Some(30.0))), 1_000);
        assert_eq!(view.state, DfState::Live);
        assert_eq!(view.bearing_true_deg, Some(90.0));
        assert_eq!(view.bearing_rel_deg, Some(60.0));
        assert_eq!(view.freq_hz, 433.92e6);
        assert_eq!(view.overlay.rays.len(), 1);
        assert_eq!(view.overlay.rays[0].from, HOME);
        let no_heading = drive.view("df1", Some(pose(None)), 1_000);
        assert_eq!(no_heading.bearing_rel_deg, None);
    }

    #[test]
    fn states_follow_the_gate_squelch_and_heading() {
        let mut drive = DfDrive::new(Some("df1".to_owned()), 0.0, None);
        assert_eq!(drive.view("df1", None, 0).state, DfState::Waiting);
        drive.reading(reading(None, false), None, 0);
        assert_eq!(drive.view("df1", None, 0).state, DfState::NoHeading);
        drive.reading(reading(Some(10.0), true), None, 0);
        let squelched = drive.view("df1", None, 0);
        assert_eq!(squelched.state, DfState::Squelched);
        assert_eq!(squelched.bearing_true_deg, None);
        drive.reading(reading(Some(10.0), false), None, 0);
        assert_eq!(
            drive.view("df1", None, DF_STALE_MS + 1).state,
            DfState::Waiting
        );
        let mut status = arrays::status("arr");
        status.processors = vec![arrays::processor("df1", Some(ProcessorGate::Calibrating))];
        drive.array(&status);
        assert_eq!(drive.view("df1", None, 0).state, DfState::Calibrating);
        drive.array(&arrays::status("arr"));
        assert_eq!(drive.view("df1", None, 0).state, DfState::Live);
    }

    #[test]
    fn a_fusion_only_drive_guides_to_the_server_target() {
        let mut drive = DfDrive::new(None, 145.5e6, None);
        assert_eq!(drive.view("tri1", None, 0).state, DfState::Waiting);
        drive.fusion(fusions::state(Some(NavTargetKind::Probe)));
        let view = drive.view("tri1", Some(pose(Some(0.0))), 0);
        assert_eq!(view.state, DfState::Live);
        assert_eq!(
            view.target.map(|target| target.kind),
            Some(GuidanceKind::Probe)
        );
        assert!(
            view.guidance
                .is_some_and(|guide| guide.distance_m > 1_000.0)
        );
        assert_eq!(view.overlay.ellipse.len(), 49);
        assert_eq!(view.overlay.stations.len(), 1);
        assert!(
            view.overlay
                .rays
                .iter()
                .all(|ray| (MIN_RAY_WEIGHT..=1.0).contains(&ray.weight))
        );
        drive.target_mode = TargetMode::Direct;
        let direct = drive.view("tri1", None, 0);
        assert_eq!(
            direct.target.map(|target| target.kind),
            Some(GuidanceKind::Estimate)
        );
        assert_eq!(direct.target_mode, TargetMode::Direct);
    }

    #[test]
    fn rays_fade_with_age_and_stay_bounded() {
        let mut drive = DfDrive::new(Some("df1".to_owned()), 0.0, None);
        for step in 0..(MAX_RAYS as i64 + 20) {
            drive.reading(reading(Some(45.0), false), None, step * 1_000);
        }
        let now = (MAX_RAYS as i64 + 20) * 1_000;
        let rays = drive.view("df1", None, now).overlay.rays;
        assert_eq!(rays.len(), MAX_RAYS);
        assert!(rays[0].weight > rays[MAX_RAYS - 1].weight);
        assert!(
            rays.iter()
                .all(|ray| (MIN_RAY_WEIGHT..=1.0).contains(&ray.weight))
        );
        assert!(
            (geo::distance_m(rays[0].from.into(), rays[0].to.into()) - RAY_LENGTH_M).abs() < 1.0
        );
        assert!(
            drive
                .view("df1", None, now + RAY_FADE_MS)
                .overlay
                .rays
                .is_empty()
        );
    }
}
