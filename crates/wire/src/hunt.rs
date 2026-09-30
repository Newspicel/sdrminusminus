use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::limits::HUNT_LIMITS as LIMITS;

pub const HUNT_SWEEP_BINS: usize = 72;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct HuntSweepParams {
    pub beamwidth_deg: f64,
    pub front_back_db: f64,
    pub min_span_deg: f64,
    pub min_contrast_db: f32,
    pub mount_offset_deg: f64,
}

impl Default for HuntSweepParams {
    fn default() -> Self {
        Self {
            beamwidth_deg: 60.0,
            front_back_db: 15.0,
            min_span_deg: 180.0,
            min_contrast_db: 6.0,
            mount_offset_deg: 0.0,
        }
    }
}

impl HuntSweepParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (
                LIMITS.beamwidth_deg.contains(self.beamwidth_deg),
                "Beamwidth out of range",
            ),
            (
                LIMITS.front_back_db.contains(self.front_back_db),
                "Front/back out of range",
            ),
            (
                LIMITS.min_span_deg.contains(self.min_span_deg),
                "Min span out of range",
            ),
            (
                LIMITS.min_contrast_db.contains(self.min_contrast_db),
                "Min contrast out of range",
            ),
            (
                LIMITS.mount_offset_deg.contains(self.mount_offset_deg),
                "Mount out of range",
            ),
        ];
        checks.iter().find(|(ok, _)| !ok).map(|(_, text)| *text)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntSettings {
    pub channel: u32,
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default)]
    pub sweep: HuntSweepParams,
}

const fn default_interval_ms() -> u32 {
    50
}

impl HuntSettings {
    #[must_use]
    pub fn for_channel(channel: u32) -> Self {
        Self {
            channel,
            interval_ms: default_interval_ms(),
            node: None,
            sweep: HuntSweepParams::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SweepState {
    #[default]
    Off,
    Idle,
    Sweeping,
    NoHeading,
    ShortSpan,
    LowContrast,
    PoorFit,
    HeadingPoor,
    TooFast,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntSweep {
    pub bins: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_deg: Option<f32>,
    pub covered_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<f32>,
    #[serde(default)]
    pub state: SweepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast_db: Option<f32>,
    #[serde(default)]
    pub rate_dps: f32,
    #[serde(default)]
    pub lag_ms: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntStatus {
    pub settings: HuntSettings,
    #[serde(default)]
    pub freq_hz: f64,
    #[serde(default)]
    pub bw_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_db: Option<f32>,
    #[serde(default)]
    pub strength: f32,
    #[serde(default)]
    pub closing: bool,
    pub readings: u64,
    #[serde(default)]
    pub at_ms: u64,
    #[serde(default)]
    pub pose_drops: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sweep: Option<HuntSweep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HuntAction {
    Start,
    Stop,
    Sweep,
    Mark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntRequest {
    pub action: HuntAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<HuntSettings>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_hunt_settings_load_with_sweep_defaults() {
        let settings: HuntSettings = serde_json::from_str(r#"{"channel":4}"#).expect("settings");
        assert_eq!(settings, HuntSettings::for_channel(4));
        assert_eq!(settings.interval_ms, 50);
        assert!(settings.sweep.valid());
        let json = serde_json::to_value(&settings).expect("serialize");
        assert!(json.get("node").is_none());
    }

    #[test]
    fn hunt_sweep_params_refuse_their_edges() {
        let base = HuntSweepParams::default();
        let cases = [
            (
                HuntSweepParams {
                    beamwidth_deg: 9.0,
                    ..base
                },
                "Beamwidth out of range",
            ),
            (
                HuntSweepParams {
                    front_back_db: 40.5,
                    ..base
                },
                "Front/back out of range",
            ),
            (
                HuntSweepParams {
                    min_span_deg: 59.0,
                    ..base
                },
                "Min span out of range",
            ),
            (
                HuntSweepParams {
                    min_contrast_db: 0.5,
                    ..base
                },
                "Min contrast out of range",
            ),
            (
                HuntSweepParams {
                    mount_offset_deg: f64::NAN,
                    ..base
                },
                "Mount out of range",
            ),
        ];
        for (params, text) in cases {
            assert_eq!(params.problem(), Some(text));
        }
    }

    #[test]
    fn a_sweeping_status_round_trips_through_json() {
        let status = HuntStatus {
            settings: HuntSettings {
                node: Some("hunt-1".to_owned()),
                ..HuntSettings::for_channel(2)
            },
            freq_hz: 433.92e6,
            bw_hz: 12_500.0,
            level_db: Some(-50.0),
            smooth_db: Some(-51.0),
            floor_db: Some(-90.0),
            best_db: Some(-40.0),
            strength: 0.7,
            closing: true,
            readings: 12,
            at_ms: 1_790_000_000_000,
            pose_drops: 1,
            sweep: Some(HuntSweep {
                bins: vec![0; HUNT_SWEEP_BINS],
                peak_deg: Some(87.0),
                covered_deg: 270.0,
                heading_deg: Some(90.0),
                state: SweepState::Done,
                sigma_deg: Some(8.0),
                fit: Some(0.9),
                contrast_db: Some(12.0),
                rate_dps: 30.0,
                lag_ms: 40.0,
            }),
            error: None,
        };
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(json["sweep"]["state"], "done");
        assert_eq!(
            serde_json::from_value::<HuntStatus>(json).expect("back"),
            status
        );
        let request: HuntRequest = serde_json::from_str(r#"{"action":"mark"}"#).expect("mark");
        assert_eq!(request.action, HuntAction::Mark);
        assert_eq!(
            serde_json::to_value(HuntAction::Sweep).expect("json"),
            "sweep"
        );
    }
}
