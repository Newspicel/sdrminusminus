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

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ScannerNode {
    #[serde(default)]
    pub mode: ScanMode,
    #[serde(default = "default_node_ranges")]
    pub ranges: Vec<ScanRange>,
    #[serde(default = "default_threshold_db")]
    pub threshold_db: f32,
    #[serde(default = "default_margin_db")]
    pub margin_db: f32,
    #[serde(default = "default_hardware_sweep")]
    pub hardware_sweep: bool,
}

pub const DEFAULT_SCAN_RANGE: ScanRange = ScanRange {
    start_hz: 145_600_000.0,
    stop_hz: 145_800_000.0,
    step_hz: 12_500.0,
};

fn default_node_ranges() -> Vec<ScanRange> {
    vec![DEFAULT_SCAN_RANGE]
}

impl Default for ScannerNode {
    fn default() -> Self {
        Self {
            mode: ScanMode::default(),
            ranges: default_node_ranges(),
            threshold_db: default_threshold_db(),
            margin_db: default_margin_db(),
            hardware_sweep: default_hardware_sweep(),
        }
    }
}

impl<'de> Deserialize<'de> for ScannerNode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stated {
            #[serde(default)]
            mode: ScanMode,
            #[serde(default = "default_node_ranges")]
            ranges: Vec<ScanRange>,
            #[serde(default = "default_threshold_db")]
            threshold_db: f32,
            #[serde(default = "default_margin_db")]
            margin_db: f32,
            #[serde(default = "default_hardware_sweep")]
            hardware_sweep: bool,
        }
        Ok(
            Option::<Stated>::deserialize(deserializer)?.map_or_else(Self::default, |stated| {
                Self {
                    mode: stated.mode,
                    ranges: stated.ranges,
                    threshold_db: stated.threshold_db,
                    margin_db: stated.margin_db,
                    hardware_sweep: stated.hardware_sweep,
                }
            }),
        )
    }
}

impl ScannerNode {
    #[must_use]
    pub fn settings_for(&self, channel: u32) -> ScanSettings {
        ScanSettings {
            mode: self.mode,
            ranges: self.ranges.clone(),
            threshold_db: self.threshold_db,
            margin_db: self.margin_db,
            hardware_sweep: self.hardware_sweep,
            ..ScanSettings::for_channel(channel)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::NodeBody;

    #[test]
    fn a_scanner_saved_before_it_kept_settings_opens_on_the_defaults() {
        let legacy: NodeBody = serde_json::from_str(r#"{"kind":"scanner"}"#).unwrap();
        assert_eq!(legacy, NodeBody::Scanner(ScannerNode::default()));
        let empty: NodeBody = serde_json::from_str(r#"{"kind":"scanner","data":{}}"#).unwrap();
        assert_eq!(empty, NodeBody::Scanner(ScannerNode::default()));
    }

    #[test]
    fn a_scanner_keeps_what_it_was_set_to() {
        let node = ScannerNode {
            mode: ScanMode::CloseCall,
            ranges: vec![ScanRange {
                start_hz: 433_050_000.0,
                stop_hz: 434_790_000.0,
                step_hz: 25_000.0,
            }],
            threshold_db: -70.0,
            margin_db: 20.0,
            hardware_sweep: false,
        };
        let body = NodeBody::Scanner(node.clone());
        let back: NodeBody = serde_json::from_value(serde_json::to_value(&body).unwrap()).unwrap();
        assert_eq!(back, body);
        let settings = node.settings_for(4);
        assert_eq!(settings.channel, 4);
        assert_eq!(settings.mode, ScanMode::CloseCall);
        assert_eq!(settings.ranges, node.ranges);
        assert_eq!(settings.margin_db, 20.0);
        assert!(!settings.hardware_sweep);
        assert_eq!(settings.dwell_ms, 250);
        assert_eq!(settings.resume_ms, 1_500);
    }
}
