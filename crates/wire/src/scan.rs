use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_SCAN_TARGETS: usize = 20_000;
pub const MAX_SCAN_DEVICE_SETS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanRange {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub step_hz: f64,
}

/// What a scan is looking for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    /// Steps a list of frequencies and holds on the first one over `threshold_db`.
    #[default]
    Targets,
    /// Watches the whole span for the loudest carrier standing `margin_db` over the noise floor
    /// and holds on that, wherever it turns out to be. `threshold_db` plays no part.
    CloseCall,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSettings {
    #[serde(default)]
    pub mode: ScanMode,
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
    /// Whether to let a radio that sweeps in its own firmware do the sweeping. Radios without one
    /// retune for every step either way.
    #[serde(default = "default_hardware_sweep")]
    pub hardware_sweep: bool,
    /// How far over the noise floor a carrier has to stand to be called, in close-call mode.
    #[serde(default = "default_margin_db")]
    pub margin_db: f32,
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
const fn default_hardware_sweep() -> bool {
    true
}
fn default_margin_db() -> f32 {
    12.0
}

impl Default for ScanSettings {
    fn default() -> Self {
        Self {
            mode: ScanMode::default(),
            ranges: Vec::new(),
            frequencies: Vec::new(),
            threshold_db: default_threshold_db(),
            dwell_ms: default_dwell_ms(),
            resume_ms: default_resume_ms(),
            measure_bw_hz: default_measure_bw_hz(),
            hold_channel: None,
            hardware_sweep: default_hardware_sweep(),
            margin_db: default_margin_db(),
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
    #[serde(default)]
    pub first_hz: f64,
    #[serde(default)]
    pub last_hz: f64,
    pub current_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_db: Option<f32>,
    pub sweeps: u64,
    pub hits: u64,
    /// Whether the sweep in force is the radio's own. A scan that asked for one and did not get
    /// it says so here rather than looking like a firmware sweep that is merely slow.
    #[serde(default)]
    pub hardware_sweep: bool,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSessionRequest {
    pub action: ScanAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_sets: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ScanSettings>,
}

/// The device sets sweeping one plan together, so a client can tell a ganged scan from several
/// unrelated ones.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSession {
    pub device_sets: Vec<u32>,
    pub settings: ScanSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanMember {
    pub device_set: u32,
    pub status: ScannerStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSessionStatus {
    pub settings: ScanSettings,
    pub members: Vec<ScanMember>,
}
