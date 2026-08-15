use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::RecordingStatus;

pub const MIN_TIME_MACHINE_SECONDS: u32 = 1;
pub const MAX_TIME_MACHINE_SECONDS: u32 = 120;
pub const DEFAULT_TIME_MACHINE_SECONDS: u32 = 10;
pub const MAX_TIME_MACHINE_BYTES: u64 = 1 << 30;

const fn default_history_seconds() -> u32 {
    DEFAULT_TIME_MACHINE_SECONDS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct TimeMachineNode {
    pub history_seconds: u32,
}

impl Default for TimeMachineNode {
    fn default() -> Self {
        Self {
            history_seconds: default_history_seconds(),
        }
    }
}

impl TimeMachineNode {
    #[must_use]
    pub const fn valid(&self) -> bool {
        MIN_TIME_MACHINE_SECONDS <= self.history_seconds
            && self.history_seconds <= MAX_TIME_MACHINE_SECONDS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeMachineAction {
    Arm,
    Capture,
    Stop,
    Disarm,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TimeMachineRequest {
    pub action: TimeMachineAction,
    pub node: String,
    #[serde(default)]
    pub stream: u32,
    #[serde(default)]
    pub settings: TimeMachineNode,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TimeMachineStatus {
    pub node: String,
    pub stream: u32,
    pub history_seconds: u32,
    pub sample_rate: u64,
    pub center_hz: i64,
    pub held_samples: u64,
    pub capacity_samples: u64,
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture: Option<RecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TimeMachineStatus {
    #[must_use]
    pub fn held_seconds(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.held_samples as f64 / self.sample_rate as f64
        }
    }
}

#[must_use]
pub fn history_capacity_samples(seconds: u32, sample_rate: f64) -> u64 {
    (f64::from(seconds) * sample_rate).ceil().max(0.0) as u64
}
