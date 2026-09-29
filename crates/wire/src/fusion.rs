use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::limits::FUSION_LIMITS as LIMITS;

pub const FUSION_FIXED_HALF_LIFE_S: u32 = 60;
pub const FUSION_MOVING_HALF_LIFE_S: u32 = 1_800;
pub const FUSION_GRID_CELLS: u16 = 256;
pub const FUSION_FRAME_CELLS: u16 = 128;
pub const MIN_FUSION_HALF_LIFE_S: u32 = 10;
pub const MAX_FUSION_HALF_LIFE_S: u32 = 86_400;
pub const DEFAULT_FUSION_HALF_LIFE_S: u32 = 300;
pub const MAX_FUSION_EMITTERS: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BearingSource {
    #[default]
    Array,
    Sweep,
    Mark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfOtherPeak {
    pub bearing_deg: f32,
    pub sigma_deg: f32,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfBearing {
    pub bearing_deg: f32,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_id: Option<String>,
    #[serde(default)]
    pub node: String,
    #[serde(default = "default_bearing_sigma")]
    pub sigma_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_sigma_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freq_hz: Option<f64>,
    #[serde(default)]
    pub source: BearingSource,
    #[serde(default)]
    pub moving: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<DfOtherPeak>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub likelihood: Vec<u8>,
}

const fn default_bearing_sigma() -> f32 {
    10.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FusionDecay {
    #[default]
    Auto,
    Fixed,
    Moving,
    HalfLife {
        seconds: u32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavMode {
    #[default]
    Auto,
    Direct,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct TriangulationParams {
    pub decay: FusionDecay,
    pub extent_km: f64,
    pub nav: NavMode,
    pub probe_km: f64,
    pub min_confidence: f32,
    pub max_emitters: u8,
}

impl Default for TriangulationParams {
    fn default() -> Self {
        Self {
            decay: FusionDecay::Auto,
            extent_km: 12.8,
            nav: NavMode::Auto,
            probe_km: 5.0,
            min_confidence: 0.05,
            max_emitters: 3,
        }
    }
}

impl TriangulationParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let fade_ok = match self.decay {
            FusionDecay::HalfLife { seconds } => LIMITS.half_life_s.contains(seconds),
            FusionDecay::Auto | FusionDecay::Fixed | FusionDecay::Moving => true,
        };
        let checks = [
            (fade_ok, "Fade out of range"),
            (
                LIMITS.extent_km.contains(self.extent_km),
                "Extent out of range",
            ),
            (
                LIMITS.probe_km.contains(self.probe_km),
                "Probe out of range",
            ),
            (
                LIMITS.min_confidence.contains(self.min_confidence),
                "Min confidence out of range",
            ),
            (
                LIMITS.emitters.contains(self.max_emitters),
                "Emitters out of range",
            ),
        ];
        checks.iter().find(|(ok, _)| !ok).map(|(_, text)| *text)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfEstimate {
    pub lat: f64,
    pub lon: f64,
    pub ellipse_major_m: f64,
    pub ellipse_minor_m: f64,
    pub ellipse_bearing_deg: f64,
    pub converged: bool,
    pub samples: u32,
    #[serde(default)]
    pub mass: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavTargetKind {
    Probe,
    Estimate,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NavTarget {
    pub lat: f64,
    pub lon: f64,
    pub kind: NavTargetKind,
    pub revision: u32,
    pub distance_m: f64,
    pub bearing_deg: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfStation {
    pub station_id: String,
    pub lat: f64,
    pub lon: f64,
    pub bearings: u32,
    pub last_seen: String,
    #[serde(default)]
    pub last_bearing_deg: f32,
    #[serde(default)]
    pub sigma_deg: f32,
    #[serde(default)]
    pub source: BearingSource,
    #[serde(default)]
    pub moving: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfFusionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<DfEstimate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emitters: Vec<DfEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav: Option<NavTarget>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<DfStation>,
    pub samples: u32,
    #[serde(default)]
    pub half_life_s: u32,
    #[serde(default)]
    pub no_guide_position: bool,
    #[serde(default)]
    pub no_bearings: bool,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub refused: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(bearing_deg: f32, confidence: f32) -> DfBearing {
        DfBearing {
            bearing_deg,
            confidence,
            lat: None,
            lon: None,
            station_id: None,
            node: String::new(),
            sigma_deg: 10.0,
            accuracy_m: None,
            heading_deg: None,
            heading_sigma_deg: None,
            relative_deg: None,
            mirror_deg: None,
            freq_hz: None,
            source: BearingSource::Array,
            moving: false,
            others: Vec::new(),
            likelihood: Vec::new(),
        }
    }

    #[test]
    fn df_bearing_legacy_json_loads() {
        let bearing: DfBearing =
            serde_json::from_str(r#"{"bearing_deg":45,"confidence":0.9,"lat":1,"lon":2}"#)
                .expect("legacy bearing");
        assert_eq!(bearing.bearing_deg, 45.0);
        assert_eq!(bearing.sigma_deg, 10.0);
        assert_eq!(bearing.source, BearingSource::Array);
        assert_eq!((bearing.lat, bearing.lon), (Some(1.0), Some(2.0)));
        assert!(bearing.node.is_empty());
        assert!(bearing.likelihood.is_empty());
        assert_eq!(
            DfBearing {
                lat: Some(1.0),
                lon: Some(2.0),
                ..plain(45.0, 0.9)
            },
            bearing
        );
    }

    #[test]
    fn a_full_bearing_round_trips_through_json() {
        let bearing = DfBearing {
            lat: Some(52.5),
            lon: Some(13.4),
            station_id: Some("roof".to_owned()),
            node: "df-1".to_owned(),
            sigma_deg: 2.5,
            accuracy_m: Some(4.0),
            heading_deg: Some(87.0),
            heading_sigma_deg: Some(3.0),
            relative_deg: Some(10.0),
            mirror_deg: Some(170.0),
            freq_hz: Some(433.92e6),
            source: BearingSource::Sweep,
            moving: true,
            others: vec![DfOtherPeak {
                bearing_deg: 200.0,
                sigma_deg: 6.0,
                confidence: 0.2,
            }],
            likelihood: vec![7; 360],
            ..plain(97.0, 0.8)
        };
        let json = serde_json::to_value(&bearing).expect("serialize");
        assert_eq!(json["source"], "sweep");
        assert_eq!(
            serde_json::from_value::<DfBearing>(json).expect("back"),
            bearing
        );
    }

    #[test]
    fn an_empty_fusion_state_reads_from_samples_alone() {
        let state: DfFusionState = serde_json::from_str(r#"{"samples":0}"#).expect("deserialize");
        assert_eq!(state, DfFusionState::default());
    }

    #[test]
    fn a_fusion_state_round_trips_with_nav_and_emitters() {
        let estimate = DfEstimate {
            lat: 52.0,
            lon: 13.0,
            ellipse_major_m: 800.0,
            ellipse_minor_m: 200.0,
            ellipse_bearing_deg: 45.0,
            converged: false,
            samples: 12,
            mass: 0.7,
        };
        let state = DfFusionState {
            estimate: Some(estimate),
            emitters: vec![estimate],
            nav: Some(NavTarget {
                lat: 52.1,
                lon: 13.1,
                kind: NavTargetKind::Probe,
                revision: 3,
                distance_m: 5_000.0,
                bearing_deg: 135.0,
            }),
            stations: vec![DfStation {
                station_id: "roof".to_owned(),
                lat: 52.0,
                lon: 13.0,
                bearings: 4,
                last_seen: "2026-09-28T12:00:00Z".to_owned(),
                last_bearing_deg: 44.0,
                sigma_deg: 3.0,
                source: BearingSource::Mark,
                moving: false,
            }],
            samples: 12,
            half_life_s: 60,
            no_guide_position: true,
            no_bearings: false,
            dropped: 1,
            refused: 2,
        };
        let json = serde_json::to_value(&state).expect("serialize");
        assert_eq!(json["nav"]["kind"], "probe");
        assert_eq!(
            serde_json::from_value::<DfFusionState>(json).expect("back"),
            state
        );
    }

    #[test]
    fn triangulation_params_default_and_refuse_their_edges() {
        let params = TriangulationParams::default();
        assert!(params.valid());
        assert_eq!(
            serde_json::from_str::<TriangulationParams>("{}").expect("empty"),
            params
        );
        let cases = [
            (
                TriangulationParams {
                    decay: FusionDecay::HalfLife { seconds: 9 },
                    ..params
                },
                "Fade out of range",
            ),
            (
                TriangulationParams {
                    decay: FusionDecay::HalfLife { seconds: 86_401 },
                    ..params
                },
                "Fade out of range",
            ),
            (
                TriangulationParams {
                    extent_km: 100.5,
                    ..params
                },
                "Extent out of range",
            ),
            (
                TriangulationParams {
                    probe_km: 0.4,
                    ..params
                },
                "Probe out of range",
            ),
            (
                TriangulationParams {
                    min_confidence: 1.5,
                    ..params
                },
                "Min confidence out of range",
            ),
            (
                TriangulationParams {
                    max_emitters: 0,
                    ..params
                },
                "Emitters out of range",
            ),
        ];
        for (bad, text) in cases {
            assert_eq!(bad.problem(), Some(text));
        }
        let decay = serde_json::to_value(FusionDecay::HalfLife { seconds: 120 }).expect("json");
        assert_eq!(decay["kind"], "half_life");
        assert_eq!(decay["seconds"], 120);
    }
}
