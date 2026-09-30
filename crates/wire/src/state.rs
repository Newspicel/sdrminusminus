use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    decode::DvTrunkProtocol,
    device::{AgcGain, Capabilities, DeviceInfo, DeviceSettings},
    hunt::HuntStatus,
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

/// Why a device set stopped, for the times a reader can act on it rather than only read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceFault {
    /// The radio is no longer attached. Plugging it back in resumes the device set.
    Unplugged,
    /// Another program holds the radio open.
    InUse,
    /// The operating system will not let this user open the radio.
    Permissions,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SettingsRefused {
    pub settings: Vec<String>,
    pub error: String,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fx: Vec<String>,
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
    pub loss: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clipping: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agc_gains: Vec<AgcGain>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<DeviceFault>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<SettingsRefused>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_export: Option<NetworkExportStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_machine: Option<TimeMachineStatus>,
    /// One scan per decoder that is being driven.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scanners: Vec<ScannerStatus>,
    /// One hunt per decoder that is being hunted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hunts: Vec<HuntStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<PlaybackStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtual_lanes: Vec<crate::array::VirtualLane>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<crate::array::HeldLane>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelLevel {
    pub channel: u32,
    pub level_db: f32,
    pub peak_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_db: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LaneLevel {
    pub stream: u32,
    pub peak_db: f32,
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
    /// Other frequencies the site runs a control channel on, found while searching.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub other_control_hz: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunk_systems: Vec<TrunkSystemStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arrays: Vec<crate::array::ArrayStatus>,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array::{ArrayStatus, HeldLane, VirtualLane};

    #[test]
    fn a_device_set_names_virtual_and_held_lanes_only_when_there_are_any() {
        let bare = crate::contract_tests::sample_device_set();
        let json = serde_json::to_value(&bare).expect("device set json");
        assert!(json.get("virtual_lanes").is_none());
        assert!(json.get("held").is_none());

        let lanes = DeviceSet {
            virtual_lanes: vec![VirtualLane {
                stream: 5,
                node: "beam".to_owned(),
                port: "beam".to_owned(),
                center_hz: 433_920_000.0,
                sample_rate: 48_000.0,
            }],
            held: vec![HeldLane {
                stream: 0,
                array: "array".to_owned(),
            }],
            ..bare
        };
        let json = serde_json::to_value(&lanes).expect("device set json");
        assert_eq!(json["virtual_lanes"][0]["stream"], 5);
        assert_eq!(json["held"][0]["array"], "array");
        let back: DeviceSet = serde_json::from_value(json).expect("device set back");
        assert_eq!(back, lanes);
    }

    #[test]
    fn a_snapshot_carries_array_status_and_loads_without_it() {
        let snapshot: StateSnapshot =
            serde_json::from_str(r#"{"device_sets":[],"revision":4}"#).expect("old snapshot");
        assert!(snapshot.arrays.is_empty());
        assert!(
            serde_json::to_value(&snapshot)
                .expect("snapshot json")
                .get("arrays")
                .is_none()
        );

        let with_array = StateSnapshot {
            arrays: vec![ArrayStatus {
                node: "array".to_owned(),
                ..ArrayStatus::default()
            }],
            ..snapshot
        };
        let json = serde_json::to_value(&with_array).expect("snapshot json");
        assert_eq!(json["arrays"][0]["node"], "array");
        let back: StateSnapshot = serde_json::from_value(json).expect("snapshot back");
        assert_eq!(back, with_array);
    }
}
