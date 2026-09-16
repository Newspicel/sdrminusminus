use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_SCAN_TARGETS: usize = 20_000;

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
    /// The decoder the scan feeds. It is parked on every hit, and the radio carrying it follows.
    pub channel: u32,
    #[serde(default)]
    pub mode: ScanMode,
    #[serde(default)]
    pub ranges: Vec<ScanRange>,
    #[serde(default)]
    pub frequencies: Vec<f64>,
    /// Frequencies the scan steps over without ever holding on them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip: Vec<f64>,
    #[serde(default = "default_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "default_dwell_ms")]
    pub dwell_ms: u32,
    #[serde(default = "default_resume_ms")]
    pub resume_ms: u32,
    /// The slice measured around each target. Left out, the decoder's own bandwidth is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure_bw_hz: Option<f64>,
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
const fn default_hardware_sweep() -> bool {
    true
}
fn default_margin_db() -> f32 {
    12.0
}

impl ScanSettings {
    #[must_use]
    pub fn for_channel(channel: u32) -> Self {
        Self {
            channel,
            mode: ScanMode::default(),
            ranges: Vec::new(),
            frequencies: Vec::new(),
            skip: Vec::new(),
            threshold_db: default_threshold_db(),
            dwell_ms: default_dwell_ms(),
            resume_ms: default_resume_ms(),
            measure_bw_hz: None,
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
    /// Leaves the frequency the scan is holding on and never holds on it again this scan.
    Skip,
}
