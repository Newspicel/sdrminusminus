use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    decode::DvTrunkProtocol,
    device::{Capabilities, DeviceInfo, DeviceSettings},
    hunt::HuntStatus,
    network::NetworkExportStatus,
    scan::{ScanSession, ScannerStatus},
    timemachine::TimeMachineStatus,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetStatus {
    Idle,
    Running,
    Error,
}

/// Why a device set stopped, for the times a reader can act on it rather than only read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFault {
    /// The radio is no longer attached. Plugging it back in resumes the device set.
    Unplugged,
    /// Another program holds the radio open.
    InUse,
    Other,
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
    /// Where the LO actually sits relative to the tuned centre, which is not always what was
    /// asked for: the front end steps it aside when a decoder is parked on the DC artifact.
    #[serde(default)]
    pub lo_offset_in_force_hz: f64,
    pub channels: Vec<ChannelInfo>,
    #[serde(default)]
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<DeviceFault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_export: Option<NetworkExportStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_machine: Option<TimeMachineStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner: Option<ScannerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunt: Option<HuntStatus>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TrunkChannelSource {
    Announced,
    Manual,
    Learned,
    Predicted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkChannel {
    pub logical_channel: u16,
    pub freq_hz: u64,
    pub source: TrunkChannelSource,
    pub confidence: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkProbe {
    pub device_set: u32,
    pub channel: u32,
    pub freq_hz: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkControl {
    pub device_set: u32,
    pub channel: u32,
    pub freq_hz: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkSystemStatus {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<DvTrunkProtocol>,
    pub carriers: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<TrunkControl>,
    pub followers: Vec<TrunkFollower>,
    pub problems: Vec<TrunkProblem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_map: Vec<TrunkChannel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<TrunkProbe>,
    #[serde(default)]
    pub searching: u32,
    /// How many frequencies the search is covering, whether the operator named a band or left
    /// the radio's own reach to be swept.
    #[serde(default)]
    pub candidates: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_session: Option<ScanSession>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunk_systems: Vec<TrunkSystemStatus>,
    pub revision: u64,
}
