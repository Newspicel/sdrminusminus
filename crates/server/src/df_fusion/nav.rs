use std::collections::VecDeque;

use sdrmm_wire::{
    DfEstimate, NavMode, NavTarget, NavTargetKind, PositionFix,
    geo::{self, LatLon},
};

use super::observation::Observation;

pub(crate) const RECENT_S: f64 = 20.0;
pub(crate) const NEAR_GUIDE_M: f64 = 300.0;
pub(crate) const RETARGET_M: f64 = 50.0;
pub(crate) const CONCENTRATED_MASS: f32 = 0.6;
pub(crate) const CONCENTRATED_MAJOR_M: f64 = 2_000.0;
pub(crate) const CONCENTRATED_SAMPLES: u32 = 6;
pub(crate) const TO_ESTIMATE_RUNS: u8 = 2;
pub(crate) const TO_PROBE_RUNS: u8 = 5;

const MAX_RECENT: usize = 1_024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RecentBearing {
    at_s: f64,
    bearing_deg: f32,
    confidence: f32,
    lat: f64,
    lon: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NavFlags {
    pub(crate) no_guide_position: bool,
    pub(crate) no_bearings: bool,
}

pub(crate) struct Nav {
    mode: NavMode,
    probe_km: f64,
    kind: NavTargetKind,
    issued: Option<(NavTargetKind, LatLon)>,
    published: bool,
    concentrated_runs: u8,
    loose_runs: u8,
    revision: u32,
    recent: VecDeque<RecentBearing>,
}

impl Nav {
    pub(crate) fn new(mode: NavMode, probe_km: f64) -> Self {
        Self {
            mode,
            probe_km,
            kind: NavTargetKind::Probe,
            issued: None,
            published: false,
            concentrated_runs: 0,
            loose_runs: 0,
            revision: 0,
            recent: VecDeque::new(),
        }
    }

    pub(crate) fn configure(&mut self, mode: NavMode, probe_km: f64) {
        if mode != self.mode {
            self.kind = NavTargetKind::Probe;
            self.concentrated_runs = 0;
            self.loose_runs = 0;
        }
        self.mode = mode;
        self.probe_km = probe_km;
    }

    pub(crate) fn clear(&mut self) {
        self.kind = NavTargetKind::Probe;
        self.issued = None;
        self.concentrated_runs = 0;
        self.loose_runs = 0;
        self.recent.clear();
    }

    pub(crate) fn remember(&mut self, observation: &Observation) {
        if self.recent.len() == MAX_RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(RecentBearing {
            at_s: observation.at_s,
            bearing_deg: observation.bearing_deg,
            confidence: observation.confidence,
            lat: observation.lat,
            lon: observation.lon,
        });
    }

    pub(crate) fn update(
        &mut self,
        guided: Option<&PositionFix>,
        estimate: Option<&DfEstimate>,
        now_s: f64,
    ) -> (Option<NavTarget>, NavFlags) {
        while self
            .recent
            .front()
            .is_some_and(|recent| now_s - recent.at_s > RECENT_S)
        {
            self.recent.pop_front();
        }
        self.count_runs(estimate);
        let mut flags = NavFlags::default();
        let Some(here) = guided.map(|fix| LatLon {
            lat: fix.latitude,
            lon: fix.longitude,
        }) else {
            flags.no_guide_position = true;
            return (self.publish(None), flags);
        };
        let kind = match self.mode {
            NavMode::Off => return (self.publish(None), flags),
            NavMode::Direct if estimate.is_some() => NavTargetKind::Estimate,
            NavMode::Direct => NavTargetKind::Probe,
            NavMode::Auto => self.kind,
        };
        let point = match (kind, estimate) {
            (NavTargetKind::Estimate, Some(estimate)) => Some((
                NavTargetKind::Estimate,
                LatLon {
                    lat: estimate.lat,
                    lon: estimate.lon,
                },
            )),
            _ => self
                .probe(here, estimate)
                .map(|point| (NavTargetKind::Probe, point)),
        };
        let Some((kind, point)) = point else {
            flags.no_bearings = true;
            return (self.publish(None), flags);
        };
        let target = NavTarget {
            lat: point.lat,
            lon: point.lon,
            kind,
            revision: self.revise(kind, point),
            distance_m: geo::distance_m(here, point),
            bearing_deg: geo::bearing_deg(here, point),
        };
        (self.publish(Some(target)), flags)
    }

    fn count_runs(&mut self, estimate: Option<&DfEstimate>) {
        let concentrated = estimate.is_some_and(|estimate| {
            estimate.mass >= CONCENTRATED_MASS
                && estimate.ellipse_major_m <= CONCENTRATED_MAJOR_M
                && estimate.samples >= CONCENTRATED_SAMPLES
        });
        if concentrated {
            self.concentrated_runs = self.concentrated_runs.saturating_add(1);
            self.loose_runs = 0;
        } else {
            self.loose_runs = self.loose_runs.saturating_add(1);
            self.concentrated_runs = 0;
        }
        if self.mode != NavMode::Auto {
            return;
        }
        match self.kind {
            NavTargetKind::Probe if self.concentrated_runs >= TO_ESTIMATE_RUNS => {
                self.kind = NavTargetKind::Estimate;
            }
            NavTargetKind::Estimate if self.loose_runs >= TO_PROBE_RUNS => {
                self.kind = NavTargetKind::Probe;
            }
            _ => {}
        }
    }

    fn probe(&self, here: LatLon, estimate: Option<&DfEstimate>) -> Option<LatLon> {
        let (east, north, weight) = self
            .recent
            .iter()
            .filter(|recent| {
                geo::distance_m(
                    here,
                    LatLon {
                        lat: recent.lat,
                        lon: recent.lon,
                    },
                ) <= NEAR_GUIDE_M
            })
            .fold((0.0f64, 0.0f64, 0.0f64), |(east, north, weight), recent| {
                let angle = f64::from(recent.bearing_deg).to_radians();
                let confidence = f64::from(recent.confidence);
                (
                    confidence.mul_add(angle.sin(), east),
                    confidence.mul_add(angle.cos(), north),
                    weight + confidence,
                )
            });
        let bearing = if weight > 0.0 && east.hypot(north) > f64::EPSILON {
            east.atan2(north).to_degrees()
        } else {
            let estimate = estimate?;
            geo::bearing_deg(
                here,
                LatLon {
                    lat: estimate.lat,
                    lon: estimate.lon,
                },
            )
        };
        Some(geo::destination(
            here,
            geo::wrap_360(bearing),
            self.probe_km * 1_000.0,
        ))
    }

    fn revise(&mut self, kind: NavTargetKind, point: LatLon) -> u32 {
        let changed = match self.issued {
            None => self.published,
            Some((issued_kind, issued_at)) => {
                issued_kind != kind || geo::distance_m(issued_at, point) > RETARGET_M
            }
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        if changed || self.issued.is_none() {
            self.issued = Some((kind, point));
        }
        self.published = true;
        self.revision
    }

    fn publish(&mut self, target: Option<NavTarget>) -> Option<NavTarget> {
        if target.is_none() {
            self.issued = None;
        }
        target
    }
}
