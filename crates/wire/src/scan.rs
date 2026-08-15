use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_SCAN_TARGETS: usize = 20_000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanRange {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub step_hz: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSettings {
    #[serde(default)]
    pub ranges: Vec<ScanRange>,
    #[serde(default)]
    pub frequencies: Vec<f64>,
    #[serde(default = "default_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "default_dwell_ms")]
    pub dwell_ms: u32,
    #[serde(default = "default_resume_ms")]
    pub resume_ms: u32,
    #[serde(default = "default_measure_bw_hz")]
    pub measure_bw_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_channel: Option<u32>,
}

fn default_threshold_db() -> f32 {
    -55.0
}
fn default_dwell_ms() -> u32 {
    250
}
fn default_resume_ms() -> u32 {
    1_500
}
fn default_measure_bw_hz() -> f64 {
    12_500.0
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            ranges: Vec::new(),
            frequencies: Vec::new(),
            threshold_db: default_threshold_db(),
            dwell_ms: default_dwell_ms(),
            resume_ms: default_resume_ms(),
            measure_bw_hz: default_measure_bw_hz(),
            hold_channel: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    Scanning,
    Holding,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScannerStatus {
    pub state: ScanState,
    pub settings: ScanSettings,
    pub targets: u32,
    pub current_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_db: Option<f32>,
    pub sweeps: u64,
    pub hits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanRequest {
    pub action: ScanAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ScanSettings>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanAction {
    Start,
    Stop,
}
