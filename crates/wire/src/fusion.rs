use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfEstimate {
    pub lat: f64,
    pub lon: f64,
    pub ellipse_major_m: f64,
    pub ellipse_minor_m: f64,
    pub ellipse_bearing_deg: f64,
    pub converged: bool,
    pub samples: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GuidanceMode {
    Cross,
    Approach,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavTargetKind {
    Cross,
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NavTarget {
    pub lat: f64,
    pub lon: f64,
    pub kind: NavTargetKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfGuidance {
    pub heading_deg: f64,
    pub mode: GuidanceMode,
    pub distance_m: f64,
    pub nav_target: NavTarget,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfStation {
    pub station_id: String,
    pub lat: f64,
    pub lon: f64,
    pub bearings: u32,
    pub last_seen: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfFusionState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<DfEstimate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guidance: Option<DfGuidance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stations: Vec<DfStation>,
    pub samples: u32,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bearing_without_a_place_round_trips_through_json() {
        let bearing = DfBearing {
            bearing_deg: 137.5,
            confidence: 0.62,
            lat: None,
            lon: None,
            station_id: Some("north".to_owned()),
        };
        let json = serde_json::to_value(&bearing).expect("serialize");
        assert!(json.get("lat").is_none());
        let back: DfBearing = serde_json::from_value(json).expect("deserialize");
        assert_eq!(back, bearing);
    }

    #[test]
    fn an_empty_fusion_state_reads_from_samples_alone() {
        let state: DfFusionState = serde_json::from_str(r#"{"samples":0}"#).expect("deserialize");
        assert_eq!(state, DfFusionState::default());
    }
}
