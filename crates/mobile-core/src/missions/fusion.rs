use std::f64::consts::TAU;

use sdrmm_wire::{
    fusion::{DfEstimate, DfFusionState, NavTargetKind},
    geo,
};

use super::views::{EstimateView, GuidanceKind, NavPoint, Station, TargetMode};
use crate::records::LatLon;

pub(crate) const ELLIPSE_SEGMENTS: usize = 48;

pub(crate) fn ellipse_ring(
    center: LatLon,
    major_m: f64,
    minor_m: f64,
    bearing_deg: f64,
) -> Vec<LatLon> {
    let (a, b) = (major_m / 2.0, minor_m / 2.0);
    let (sin, cos) = bearing_deg.to_radians().sin_cos();
    let mut ring: Vec<LatLon> = (0..ELLIPSE_SEGMENTS)
        .map(|k| {
            let theta = TAU * k as f64 / ELLIPSE_SEGMENTS as f64;
            let (u, v) = (a * theta.cos(), b * theta.sin());
            let east = u.mul_add(sin, v * cos);
            let north = u.mul_add(cos, -(v * sin));
            geo::offset_m(center.into(), east, north).into()
        })
        .collect();
    if let Some(first) = ring.first().copied() {
        ring.push(first);
    }
    ring
}

fn usable(estimate: &DfEstimate) -> bool {
    estimate.lat.is_finite()
        && estimate.lon.is_finite()
        && estimate.ellipse_major_m.is_finite()
        && estimate.ellipse_minor_m.is_finite()
        && estimate.ellipse_bearing_deg.is_finite()
}

pub(crate) fn estimate(state: &DfFusionState) -> Option<EstimateView> {
    let estimate = state.estimate.filter(usable)?;
    Some(EstimateView {
        at: LatLon {
            lat: estimate.lat,
            lon: estimate.lon,
        },
        major_m: estimate.ellipse_major_m,
        minor_m: estimate.ellipse_minor_m,
        axis_deg: estimate.ellipse_bearing_deg,
        converged: estimate.converged,
        samples: estimate.samples,
    })
}

pub(crate) fn ellipse(state: &DfFusionState) -> Vec<LatLon> {
    estimate(state).map_or_else(Vec::new, |view| {
        ellipse_ring(view.at, view.major_m, view.minor_m, view.axis_deg)
    })
}

pub(crate) fn stations(state: &DfFusionState) -> Vec<Station> {
    state
        .stations
        .iter()
        .filter(|station| station.lat.is_finite() && station.lon.is_finite())
        .map(|station| Station {
            id: station.station_id.clone(),
            at: LatLon {
                lat: station.lat,
                lon: station.lon,
            },
            bearings: station.bearings,
        })
        .collect()
}

pub(crate) fn target(state: &DfFusionState, mode: TargetMode) -> Option<NavPoint> {
    match mode {
        TargetMode::Auto => state.nav.map(|nav| NavPoint {
            at: LatLon {
                lat: nav.lat,
                lon: nav.lon,
            },
            kind: match nav.kind {
                NavTargetKind::Probe => GuidanceKind::Probe,
                NavTargetKind::Estimate => GuidanceKind::Estimate,
            },
        }),
        TargetMode::Direct => estimate(state).map(|view| NavPoint {
            at: view.at,
            kind: GuidanceKind::Estimate,
        }),
    }
    .filter(|point| point.at.lat.is_finite() && point.at.lon.is_finite())
}

#[cfg(test)]
pub(crate) mod tests {
    use sdrmm_wire::fusion::{DfStation, NavTarget};

    use super::*;

    const CENTER: LatLon = LatLon {
        lat: 52.52,
        lon: 13.405,
    };

    pub(crate) fn state(nav: Option<NavTargetKind>) -> DfFusionState {
        DfFusionState {
            estimate: Some(DfEstimate {
                lat: CENTER.lat,
                lon: CENTER.lon,
                ellipse_major_m: 800.0,
                ellipse_minor_m: 200.0,
                ellipse_bearing_deg: 45.0,
                converged: true,
                samples: 12,
                mass: 0.9,
            }),
            emitters: Vec::new(),
            nav: nav.map(|kind| NavTarget {
                lat: 52.6,
                lon: 13.5,
                kind,
                revision: 3,
                distance_m: 9_000.0,
                bearing_deg: 30.0,
            }),
            stations: vec![DfStation {
                station_id: "car".to_owned(),
                lat: 52.4,
                lon: 13.3,
                bearings: 5,
                last_seen: "2026-09-28T12:00:00Z".to_owned(),
                last_bearing_deg: 40.0,
                sigma_deg: 3.0,
                source: Default::default(),
                moving: true,
            }],
            samples: 12,
            half_life_s: 60,
            no_guide_position: false,
            no_bearings: false,
            dropped: 0,
            refused: 0,
        }
    }

    #[test]
    fn ellipse_is_a_closed_ring_of_49_points_along_the_bearing() {
        let ring = ellipse(&state(None));
        assert_eq!(ring.len(), 49);
        assert_eq!(ring.first(), ring.last());
        let first = ring[0];
        assert!((geo::distance_m(CENTER.into(), first.into()) - 400.0).abs() < 0.5);
        assert!((geo::bearing_deg(CENTER.into(), first.into()) - 45.0).abs() < 0.1);
        let quarter = ring[12];
        assert!((geo::distance_m(CENTER.into(), quarter.into()) - 100.0).abs() < 0.5);
        assert!((geo::bearing_deg(CENTER.into(), quarter.into()) - 135.0).abs() < 0.1);
    }

    #[test]
    fn the_target_follows_the_mode() {
        let fused = state(Some(NavTargetKind::Probe));
        assert_eq!(
            target(&fused, TargetMode::Auto).map(|point| point.kind),
            Some(GuidanceKind::Probe)
        );
        assert_eq!(
            target(&fused, TargetMode::Direct),
            Some(NavPoint {
                at: CENTER,
                kind: GuidanceKind::Estimate
            })
        );
        assert_eq!(target(&state(None), TargetMode::Auto), None);
        let mut empty = state(None);
        empty.estimate = None;
        assert_eq!(target(&empty, TargetMode::Direct), None);
        assert!(ellipse(&empty).is_empty());
        assert_eq!(stations(&fused)[0].bearings, 5);
        assert_eq!(estimate(&fused).map(|view| view.axis_deg), Some(45.0));
    }
}
