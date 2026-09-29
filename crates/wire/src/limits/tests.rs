use serde_json::Value;

use super::*;
use crate::{
    ArrayCal, ArrayCalSource, ArrayGain, ArrayNode, CfarKind, FusionDecay, Illuminator,
    PassiveRadarParams, ReferenceCleaning, TriangulationParams,
};

const WRITTEN: &str = include_str!("../../../../web/src/generated/limits.json");

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).expect("limits json parses")
}

fn radar(edit: impl FnOnce(&mut PassiveRadarParams)) -> Option<&'static str> {
    let mut params = PassiveRadarParams::default();
    edit(&mut params);
    params.problem()
}

fn fusion(edit: impl FnOnce(&mut TriangulationParams)) -> Option<&'static str> {
    let mut params = TriangulationParams::default();
    edit(&mut params);
    params.problem()
}

fn pilot(offset_hz: f64, bandwidth_hz: f64) -> bool {
    ArrayNode {
        cal: ArrayCal {
            source: ArrayCalSource::Pilot {
                offset_hz,
                bandwidth_hz,
            },
            ..ArrayCal::default()
        },
        ..ArrayNode::default()
    }
    .valid()
}

#[test]
fn limits_json_matches_the_wire_constants() {
    assert_eq!(
        parsed(WRITTEN),
        parsed(&generated().expect("limits serialize")),
        "run cargo xtask codegen"
    );
}

#[test]
fn only_an_open_bound_names_itself() {
    let json = parsed(&generated().expect("limits serialize"));
    assert_eq!(
        json["radar"]["cpi_ms"],
        serde_json::json!({"min": 50, "max": 2000})
    );
    assert_eq!(json["radar"]["jerk"]["above"], true);
    assert_eq!(json["fusion"]["fixed_half_life_s"], 60);
    assert_eq!(json["light_speed_m_s"], 299_792_458.0);
    assert_eq!(json["radar"]["cma_step"]["max"], 0.1);
}

#[test]
fn bounds_hold_their_edges() {
    let closed = Bounds::new(1.0_f32, 2.0);
    assert!(closed.contains(1.0) && closed.contains(2.0));
    assert!(!closed.contains(0.99) && !closed.contains(2.01) && !closed.contains(f32::NAN));
    let open = Bounds::above(0.0_f32, 1.0);
    assert!(!open.contains(0.0) && open.contains(f32::MIN_POSITIVE) && open.contains(1.0));
}

#[test]
fn radar_validation_follows_the_limits() {
    let limits = RADAR_LIMITS;
    assert_eq!(radar(|p| p.cpi_ms = limits.cpi_ms.max), None);
    assert!(radar(|p| p.cpi_ms = limits.cpi_ms.max + 1).is_some());
    assert!(radar(|p| p.cpi_ms = limits.cpi_ms.min - 1).is_some());
    assert_eq!(radar(|p| p.overlap = limits.overlap.max), None);
    assert!(radar(|p| p.max_range_km = limits.max_range_km.min).is_some());
    assert_eq!(radar(|p| p.max_range_km = limits.max_range_km.max), None);
    assert_eq!(radar(|p| p.offset_hz = limits.offset_hz.min), None);
    assert!(radar(|p| p.clutter.step = limits.clutter_step.min).is_some());
    assert_eq!(radar(|p| p.clutter.reach_km = limits.reach_km.min), None);
    assert!(radar(|p| p.clutter.lead = limits.lead.max + 1).is_some());
    assert_eq!(radar(|p| p.cfar.train_range = limits.train_range.max), None);
    assert!(radar(|p| p.cfar.guard_doppler = limits.guard.max + 1).is_some());
    assert!(radar(|p| p.tracker.jerk = limits.jerk.min).is_some());
    assert_eq!(radar(|p| p.tracker.gate = limits.gate.max), None);
    assert!(radar(|p| p.tracker.coast_looks = limits.coast_looks.max + 1).is_some());
    assert!(
        radar(|p| {
            p.tracker.confirm_hits = limits.track_window.max;
            p.tracker.confirm_window = limits.track_window.max + 1;
        })
        .is_some()
    );
    assert!(radar(|p| p.assumed_altitude_m = limits.altitude_m.max + 1.0).is_some());
}

#[test]
fn radar_seeds_are_valid_settings() {
    let seed = RADAR_LIMITS.seed;
    assert_eq!(
        radar(|p| {
            p.illuminator = Illuminator::DvbtPartial {
                bandwidth_hz: seed.dvbt_bandwidth_hz,
            };
            p.reference = ReferenceCleaning::Off;
        }),
        None
    );
    assert_eq!(
        radar(|p| p.illuminator = Illuminator::Custom {
            bandwidth_hz: seed.custom_bandwidth_hz
        }),
        None
    );
    assert_eq!(
        radar(|p| p.reference = ReferenceCleaning::Cma {
            taps: seed.cma_taps,
            step: seed.cma_step
        }),
        None
    );
    assert_eq!(
        radar(|p| p.cfar.kind = CfarKind::Os { rank: seed.os_rank }),
        None
    );
    assert_eq!(
        PassiveRadarParams::default().reference,
        ReferenceCleaning::Cma {
            taps: seed.cma_taps,
            step: seed.cma_step
        }
    );
}

#[test]
fn fusion_validation_follows_the_limits() {
    let limits = FUSION_LIMITS;
    for seconds in [
        limits.half_life_s.min,
        limits.half_life_seed_s,
        limits.half_life_s.max,
    ] {
        assert_eq!(
            fusion(|p| p.decay = FusionDecay::HalfLife { seconds }),
            None
        );
    }
    assert!(
        fusion(|p| p.decay = FusionDecay::HalfLife {
            seconds: limits.half_life_s.max + 1
        })
        .is_some()
    );
    assert!(fusion(|p| p.extent_km = limits.extent_km.max + 0.1).is_some());
    assert_eq!(fusion(|p| p.probe_km = limits.probe_km.min), None);
    assert!(fusion(|p| p.min_confidence = limits.min_confidence.max + 0.1).is_some());
    assert_eq!(fusion(|p| p.max_emitters = limits.emitters.max), None);
    assert!(fusion(|p| p.max_emitters = limits.emitters.max + 1).is_some());
}

#[test]
fn array_limits_follow_the_validation() {
    let limits = ARRAY_LIMITS;
    assert!(pilot(limits.cal_offset_hz.min, limits.cal_bandwidth_hz.max));
    assert!(pilot(
        limits.cal_offset_hz.max,
        limits.cal_bandwidth_seed_hz
    ));
    assert!(!pilot(0.0, limits.cal_bandwidth_hz.min - 1.0));
    for db in [limits.gain_db.min, limits.gain_db.max] {
        assert_eq!(ArrayGain::Manual { db }.problem(), None);
    }
    assert!(
        ArrayGain::Manual {
            db: limits.gain_db.max + 0.5
        }
        .problem()
        .is_some()
    );
}
