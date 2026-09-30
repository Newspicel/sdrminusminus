use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_SURVEY_CELLS: usize = 5_000;
pub const SURVEY_CELL_M: f64 = 10.0;
pub const SURVEY_LEVEL_INTERVAL_MS: u64 = 250;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SurveyCell {
    pub latitude: f64,
    pub longitude: f64,
    pub frequency_hz: f64,
    pub level_dbfs: f32,
    pub measured_at: String,
    pub observations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SurveyGrid {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_hz: Option<f64>,
    pub offset_hz: i64,
    pub bandwidth_hz: u64,
    pub recording: bool,
    pub cells: Vec<SurveyCell>,
    #[serde(default)]
    pub dropped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurveyAction {
    Start,
    Stop,
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SurveyRequest {
    pub action: SurveyAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurveyStop {
    Retuned,
    Unwired,
    RadioGone,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SurveyUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_dbfs: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hz: Option<f64>,
    pub recording: bool,
    pub cells: u32,
    pub dropped: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<SurveyCell>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<SurveyStop>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn cell() -> SurveyCell {
        SurveyCell {
            latitude: 52.52,
            longitude: 13.405,
            frequency_hz: 145_500_000.0,
            level_dbfs: -42.5,
            measured_at: "2026-09-28T12:00:00Z".to_owned(),
            observations: 3,
            accuracy_m: Some(4.0),
        }
    }

    #[test]
    fn survey_update_round_trips() {
        let full = SurveyUpdate {
            level_dbfs: Some(-40.25),
            target_hz: Some(145_500_000.0),
            recording: true,
            cells: 12,
            dropped: 1,
            cell: Some(cell()),
            stopped: Some(SurveyStop::RadioGone),
        };
        let value = serde_json::to_value(&full).unwrap();
        assert_eq!(value["stopped"], "radio_gone");
        assert_eq!(value["cell"]["observations"], 3);
        assert_eq!(serde_json::from_value::<SurveyUpdate>(value).unwrap(), full);

        let quiet = SurveyUpdate {
            level_dbfs: None,
            target_hz: None,
            recording: false,
            cells: 0,
            dropped: 0,
            cell: None,
            stopped: None,
        };
        let value = serde_json::to_value(&quiet).unwrap();
        assert_eq!(value, json!({"recording": false, "cells": 0, "dropped": 0}));
        assert_eq!(
            serde_json::from_value::<SurveyUpdate>(value).unwrap(),
            quiet
        );
    }

    #[test]
    fn survey_grid_and_request_shapes() {
        let grid: SurveyGrid = serde_json::from_value(json!({
            "node": "survey1",
            "offset_hz": -25_000,
            "bandwidth_hz": 12_500,
            "recording": true,
            "cells": [serde_json::to_value(cell()).unwrap()]
        }))
        .unwrap();
        assert_eq!(grid.dropped, 0);
        assert_eq!(grid.frequency_hz, None);
        assert_eq!(grid.cells, [cell()]);
        for (action, text) in [
            (SurveyAction::Start, "start"),
            (SurveyAction::Stop, "stop"),
            (SurveyAction::Clear, "clear"),
        ] {
            assert_eq!(
                serde_json::to_value(SurveyRequest { action }).unwrap(),
                json!({"action": text})
            );
        }
    }
}
