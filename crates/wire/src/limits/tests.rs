use serde_json::Value;

use super::*;
use crate::{
    ArrayCal, ArrayCalSource, ArrayGain, ArrayNode, CfarKind, FusionDecay, Illuminator,
    PassiveRadarParams, ReferenceCleaning, TriangulationParams,
    hunt::HuntSweepParams,
    processor::{
        beamformer::{BeamformerParams, SteerSource},
        correlator::CorrelatorParams,
        df::DfParams,
        polarimeter::PolarimeterParams,
        spatial::SpatialSpectrumParams,
    },
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

fn checked<T: Default>(
    edit: impl FnOnce(&mut T),
    problem: fn(&T) -> Option<&'static str>,
) -> Option<&'static str> {
    let mut params = T::default();
    edit(&mut params);
    problem(&params)
}

fn df(edit: impl FnOnce(&mut DfParams)) -> Option<&'static str> {
    checked(edit, DfParams::problem)
}

fn beam(edit: impl FnOnce(&mut BeamformerParams)) -> Option<&'static str> {
    checked(edit, BeamformerParams::problem)
}

fn spatial(edit: impl FnOnce(&mut SpatialSpectrumParams)) -> Option<&'static str> {
    checked(edit, SpatialSpectrumParams::problem)
}

fn correlator(edit: impl FnOnce(&mut CorrelatorParams)) -> Option<&'static str> {
    checked(edit, CorrelatorParams::problem)
}

fn polarimeter(edit: impl FnOnce(&mut PolarimeterParams)) -> Option<&'static str> {
    checked(edit, PolarimeterParams::problem)
}

fn hunt(edit: impl FnOnce(&mut HuntSweepParams)) -> Option<&'static str> {
    checked(edit, HuntSweepParams::problem)
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
    assert_eq!(
        json["df"]["report_ms"],
        serde_json::json!({"min": 100, "max": 10000})
    );
    assert_eq!(json["df"]["band"]["bandwidth_hz"]["min"], 100.0);
    assert_eq!(json["spatial"]["band"]["bandwidth_hz"]["min"], 1000.0);
    assert_eq!(json["beamformer"]["nulls"], 3);
    assert_eq!(json["band_seed_hz"], 200_000.0);
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

#[test]
fn df_validation_follows_the_limits() {
    let limits = DF_LIMITS;
    assert_eq!(df(|p| p.sources = Some(limits.sources.max)), None);
    assert!(df(|p| p.sources = Some(limits.sources.max + 1)).is_some());
    assert!(df(|p| p.max_peaks = limits.peaks.min - 1).is_some());
    assert_eq!(df(|p| p.max_peaks = limits.peaks.max), None);
    assert_eq!(df(|p| p.smoothing = limits.smoothing.max), None);
    assert!(df(|p| p.smoothing = limits.smoothing.max + 1).is_some());
    assert_eq!(df(|p| p.offset_hz = limits.band.offset_hz.min), None);
    assert!(df(|p| p.offset_hz = limits.band.offset_hz.max * 2.0).is_some());
    assert_eq!(df(|p| p.bandwidth_hz = limits.band.bandwidth_hz.min), None);
    assert!(df(|p| p.bandwidth_hz = limits.band.bandwidth_hz.min - 1.0).is_some());
    assert_eq!(df(|p| p.report_ms = limits.report_ms.min), None);
    assert!(df(|p| p.report_ms = limits.report_ms.max + 1).is_some());
    assert_eq!(df(|p| p.carry_over = limits.carry_over.max), None);
    assert!(df(|p| p.squelch_db = limits.squelch_db.max + 0.5).is_some());
    assert_eq!(df(|p| p.loading = limits.loading.max), None);
    assert!(df(|p| p.azimuth_step_deg = limits.azimuth_step_deg.min / 2.0).is_some());
    assert_eq!(df(|p| p.yaw_gate_dps = limits.yaw_gate_dps.max), None);
    assert!(df(|p| p.yaw_gate_dps = limits.yaw_gate_dps.min - 0.5).is_some());
    assert_eq!(
        df(|p| p.station_id = Some("s".repeat(limits.station_len))),
        None
    );
    assert!(df(|p| p.station_id = Some("s".repeat(limits.station_len + 1))).is_some());
}

#[test]
fn beamformer_validation_follows_the_limits() {
    let limits = BEAMFORMER_LIMITS;
    assert_eq!(beam(|p| p.nulls_deg = vec![10.0; limits.nulls]), None);
    assert!(beam(|p| p.nulls_deg = vec![10.0; limits.nulls + 1]).is_some());
    assert_eq!(beam(|p| p.taps = limits.taps.max), None);
    assert!(beam(|p| p.taps = limits.taps.max + 1).is_some());
    assert_eq!(beam(|p| p.step = limits.step.min), None);
    assert!(beam(|p| p.step = limits.step.max + 0.1).is_some());
    assert_eq!(beam(|p| p.forget = limits.forget.max), None);
    assert!(beam(|p| p.crossfade_ms = limits.crossfade_ms.max + 1).is_some());
    assert!(beam(|p| p.update_ms = limits.update_ms.min - 1).is_some());
    assert_eq!(beam(|p| p.carry_over = limits.carry_over.max), None);
    assert_eq!(beam(|p| p.loading = limits.loading.max), None);
    assert!(beam(|p| p.steer_timeout_ms = limits.steer_timeout_ms.max + 1).is_some());
    assert_eq!(
        beam(|p| p.bandwidth_hz = Some(limits.band.bandwidth_hz.min)),
        None
    );
    assert!(beam(|p| p.bandwidth_hz = Some(limits.band.bandwidth_hz.min - 1.0)).is_some());
    assert_eq!(beam(|p| p.bandwidth_hz = Some(LIMITS.band_seed_hz)), None);
    assert_eq!(
        BeamformerParams::default().bandwidth_hz,
        Some(LIMITS.band_seed_hz)
    );
    let steer = |elevation_deg| SteerSource::Fixed {
        azimuth_deg: 0.0,
        elevation_deg,
    };
    assert_eq!(beam(|p| p.steer = steer(limits.elevation_deg.max)), None);
    assert!(beam(|p| p.steer = steer(limits.elevation_deg.max + 1.0)).is_some());
}

#[test]
fn spatial_validation_follows_the_limits() {
    let limits = SPATIAL_LIMITS;
    for bins in [limits.bins.min, limits.bins.max] {
        assert_eq!(spatial(|p| p.bins = bins), None);
    }
    assert!(spatial(|p| p.bins = limits.bins.max * 2).is_some());
    assert_eq!(spatial(|p| p.columns = limits.columns.min), None);
    assert!(spatial(|p| p.columns = limits.columns.min / 2).is_some());
    assert_eq!(
        spatial(|p| {
            p.bins = limits.bins.max;
            p.columns = limits.columns.max;
        }),
        None
    );
    assert!(spatial(|p| p.average_ms = limits.average_ms.max + 1).is_some());
    assert_eq!(spatial(|p| p.report_ms = limits.report_ms.max), None);
    assert!(spatial(|p| p.report_ms = limits.report_ms.max + 1).is_some());
    for step in [limits.azimuth_step_deg.min, limits.azimuth_step_deg.max] {
        assert_eq!(spatial(|p| p.azimuth_step_deg = step), None);
    }
    assert!(spatial(|p| p.azimuth_step_deg = limits.azimuth_step_deg.max + 2.0).is_some());
    assert!(spatial(|p| p.span_db = limits.span_db.min - 1.0).is_some());
    assert_eq!(
        spatial(|p| p.bandwidth_hz = Some(LIMITS.band_seed_hz)),
        None
    );
    assert!(spatial(|p| p.offset_hz = limits.band.offset_hz.min - 1.0).is_some());
}

#[test]
fn correlator_validation_follows_the_limits() {
    let limits = CORRELATOR_LIMITS;
    for bins in [limits.bins.min, limits.bins.max] {
        assert_eq!(
            correlator(|p| {
                p.bins = bins;
                p.channels = limits.channels.min;
            }),
            None
        );
    }
    assert!(correlator(|p| p.bins = limits.bins.min / 2).is_some());
    assert_eq!(correlator(|p| p.channels = limits.channels.max), None);
    assert!(correlator(|p| p.channels = limits.channels.min / 2).is_some());
    assert_eq!(correlator(|p| p.integrate_s = limits.integrate_s.min), None);
    assert!(correlator(|p| p.integrate_s = limits.integrate_s.max + 1.0).is_some());
    assert_eq!(
        correlator(|p| p.bandwidth_hz = Some(LIMITS.band_seed_hz)),
        None
    );
    assert!(correlator(|p| p.bandwidth_hz = Some(limits.band.bandwidth_hz.max + 1.0)).is_some());
}

#[test]
fn polarimeter_validation_follows_the_limits() {
    let limits = POLARIMETER_LIMITS;
    assert_eq!(
        polarimeter(|p| p.bandwidth_hz = limits.band.bandwidth_hz.min),
        None
    );
    assert!(polarimeter(|p| p.bandwidth_hz = limits.band.bandwidth_hz.min - 1.0).is_some());
    assert_eq!(
        polarimeter(|p| p.offset_hz = limits.band.offset_hz.max),
        None
    );
    assert!(polarimeter(|p| p.report_ms = limits.report_ms.min - 1).is_some());
    assert_eq!(polarimeter(|p| p.average_ms = limits.average_ms.max), None);
    assert!(polarimeter(|p| p.crossfade_ms = limits.crossfade_ms.max + 1).is_some());
}

#[test]
fn hunt_sweep_validation_follows_the_limits() {
    let limits = HUNT_LIMITS;
    assert_eq!(hunt(|p| p.beamwidth_deg = limits.beamwidth_deg.min), None);
    assert!(hunt(|p| p.beamwidth_deg = limits.beamwidth_deg.max + 1.0).is_some());
    assert_eq!(hunt(|p| p.front_back_db = limits.front_back_db.max), None);
    assert!(hunt(|p| p.min_span_deg = limits.min_span_deg.min - 1.0).is_some());
    assert_eq!(
        hunt(|p| p.min_contrast_db = limits.min_contrast_db.max),
        None
    );
    assert!(hunt(|p| p.mount_offset_deg = limits.mount_offset_deg.max + 1.0).is_some());
    assert_eq!(
        hunt(|p| p.mount_offset_deg = limits.mount_offset_deg.min),
        None
    );
}
