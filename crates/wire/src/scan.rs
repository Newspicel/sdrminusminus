//! Frequency-scanner types ( P2, M5). The scanner is app-level, not a channel: it
//! steps the *device* across a set of target frequencies, measures each one against the
//! spectrum tap, and parks a hosted channel on anything that breaks the threshold.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Hard ceiling on the targets one scan may expand to. A scan is a bounded list held in
/// memory and walked every sweep; an unbounded `step_hz` typo would otherwise turn a range
/// into gigabytes.
pub const MAX_SCAN_TARGETS: usize = 20_000;

/// One contiguous span to sweep, expanded to `start_hz, start_hz + step_hz, …` up to and
/// including `stop_hz` when it lands on a step.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanRange {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub step_hz: f64,
}

/// What a scan covers and how it behaves on a hit (: "frequency scanner").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanSettings {
    /// Ranges to expand into targets. May be empty if `frequencies` is not.
    #[serde(default)]
    pub ranges: Vec<ScanRange>,
    /// Individual target frequencies (bookmarks, memory channels).
    #[serde(default)]
    pub frequencies: Vec<f64>,
    /// A target counts as active when its measured power reaches this level, in dBFS on the
    /// device's spectrum tap.
    #[serde(default = "default_threshold_db")]
    pub threshold_db: f32,
    /// Measurement window per device tuning. Every target inside the tuning's passband is
    /// measured from the same spectrum frames, so this is per *tuning*, not per target.
    #[serde(default = "default_dwell_ms")]
    pub dwell_ms: u32,
    /// How long a held target must stay below the threshold before the sweep resumes.
    #[serde(default = "default_resume_ms")]
    pub resume_ms: u32,
    /// Bandwidth measured around each target; also the width the hold channel is judged over.
    #[serde(default = "default_measure_bw_hz")]
    pub measure_bw_hz: f64,
    /// Channel retuned onto a hit so its audio (or decoder) follows the scan. `None` scans
    /// without listening — the hit log alone.
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

/// What the scanner is doing right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanState {
    /// Stepping through targets.
    Scanning,
    /// Parked on an active target.
    Holding,
}

/// Live scanner state, projected onto the device set and pushed as
/// [`crate::ServerEvent::ScannerUpdate`] while a scan runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScannerStatus {
    pub state: ScanState,
    pub settings: ScanSettings,
    /// Targets the settings expanded to.
    pub targets: u32,
    /// Target the scanner is measuring (scanning) or parked on (holding).
    pub current_hz: f64,
    /// Measured level of `current_hz` at the last measurement, dBFS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_db: Option<f32>,
    /// Completed passes over the whole target list.
    pub sweeps: u64,
    /// Targets that broke the threshold since the scan started.
    pub hits: u64,
    /// Fatal scanner fault (the device stopped accepting retunes); the scan has stopped but
    /// the cause stays visible (CLAUDE.md no-silent-failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `POST /api/devicesets/{ds}/scanner` — start or stop a scan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScanRequest {
    pub action: ScanAction,
    /// Required for `start`, ignored by `stop`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ScanSettings>,
}

/// What a [`ScanRequest`] should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanAction {
    Start,
    Stop,
}
