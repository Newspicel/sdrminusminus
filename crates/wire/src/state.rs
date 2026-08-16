use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    decode::DvTrunkProtocol,
    device::{Capabilities, DeviceInfo, DeviceSettings},
    network::NetworkExportStatus,
    scan::ScannerStatus,
    timemachine::TimeMachineStatus,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetStatus {
    Idle,
    Running,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingStatus {
    pub file: String,
    #[serde(default)]
    pub stream: u32,
    pub started_at: String,
    pub samples: u64,
    pub bytes: u64,
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioRecordingStatus {
    pub file: String,
    pub started_at: String,
    pub channels: u8,
    pub frames: u64,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackStatus {
    pub position_samples: u64,
    pub total_samples: u64,
    pub paused: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSet {
    pub id: u32,
    pub device: DeviceInfo,
    pub capabilities: Capabilities,
    pub settings: DeviceSettings,
    pub status: DeviceSetStatus,
    pub channels: Vec<ChannelInfo>,
    #[serde(default)]
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_export: Option<NetworkExportStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_machine: Option<TimeMachineStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner: Option<ScannerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<PlaybackStatus>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelLevel {
    pub channel: u32,
    pub level_db: f32,
    pub peak_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_db: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkFollower {
    pub device_set: u32,
    pub channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_channel: Option<u16>,
    pub slot: u8,
    pub freq_hz: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkProblem {
    pub freq_hz: u64,
    pub slot: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_channel: Option<u16>,
    pub reason: String,
    pub since: String,
    pub attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkSystemStatus {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<DvTrunkProtocol>,
    pub carriers: u32,
    pub followers: Vec<TrunkFollower>,
    pub problems: Vec<TrunkProblem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunk_systems: Vec<TrunkSystemStatus>,
    pub revision: u64,
}
