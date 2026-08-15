//! The authoritative state model (: the server is the single source of truth).
//! `GET /api/state` returns [`StateSnapshot`]; clients converge via WS `StateChanged`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    decode::DvTrunkProtocol,
    device::{Capabilities, DeviceInfo, DeviceSettings},
    network::NetworkExportStatus,
    scan::ScannerStatus,
};

/// Runtime status of a device set's capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetStatus {
    Idle,
    Running,
    Error,
}

/// Live IQ recording on a device set (: the recording path is lossless, so a writer
/// fault must surface here rather than dropping samples silently).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingStatus {
    /// Recording stem: file name without directory or `.sigmf-*` extension.
    pub file: String,
    /// Which of the device's receive streams is being recorded. Defaults to 0 because a
    /// status from before multi-stream devices names no stream and means the only one its
    /// radio had.
    #[serde(default)]
    pub stream: u32,
    /// RFC3339 UTC.
    pub started_at: String,
    /// Samples written to the `.sigmf-data` file so far.
    pub samples: u64,
    pub bytes: u64,
    pub overruns: u64,
    /// Fatal recording fault (queue overflow, disk error); the writer has stopped but the
    /// cause stays visible (CLAUDE.md no-silent-failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Transport of a device set replaying a recording (`virtual:file:`). Absent on a live radio:
/// there is no position to seek in a signal that is still arriving.
///
/// Whether it loops is *not* here — that is `loop` in [`DeviceSettings::extra`], a setting the
/// radio carries and a workspace saves. Pause and position are the opposite: reopening a patch
/// must not restore a paused transport, so they live only in this live status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackStatus {
    /// Samples replayed from the start of the recording.
    pub position_samples: u64,
    /// Samples the recording holds. Read off the data file, so a crash-truncated pair reports
    /// what can actually be replayed rather than what its metadata claims.
    pub total_samples: u64,
    pub paused: bool,
}

/// One opened device and everything hosted on it (: "one device set per opened device").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSet {
    pub id: u32,
    pub device: DeviceInfo,
    pub capabilities: Capabilities,
    pub settings: DeviceSettings,
    pub status: DeviceSetStatus,
    pub channels: Vec<ChannelInfo>,
    /// Cumulative device samples dropped at the capture ring since the set opened. Growth
    /// means the DSP thread cannot keep up — audio and spectrum have gaps even while
    /// `status` stays `running` ( backpressure; CLAUDE.md no-silent-failure).
    #[serde(default)]
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Active IQ recording, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
    /// Active raw-IQ network export, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_export: Option<NetworkExportStatus>,
    /// Running frequency scan, if any. While a scan runs the set's
    /// `settings.center_hz` moves every dwell, so live progress arrives as
    /// [`crate::ServerEvent::ScannerUpdate`] rather than one `StateChanged` per step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanner: Option<ScannerStatus>,
    /// Replay transport, on a set whose device is a recording rather than a radio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback: Option<PlaybackStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkFollower {
    pub device_set: u32,
    /// The engine channel doing the following; it has no patch node of its own.
    pub channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_channel: Option<u16>,
    pub slot: u8,
    pub freq_hz: u64,
}

/// A grant the follower could not act on — commonly a traffic channel outside the sampled
/// bandwidth. Carried in state so the operator sees why a busy system produces no calls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkProblem {
    pub freq_hz: u64,
    pub slot: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_channel: Option<u16>,
    pub reason: String,
    /// RFC3339 UTC.
    pub since: String,
    pub attempts: u32,
}

/// What one `dmr_trunk` node is following. Also how the client attributes follower channels,
/// which have no patch node, to the system that owns them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TrunkSystemStatus {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected: Option<DvTrunkProtocol>,
    pub carriers: u32,
    pub followers: Vec<TrunkFollower>,
    pub problems: Vec<TrunkProblem>,
}

/// Full state snapshot for initial load ( `GET /api/state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunk_systems: Vec<TrunkSystemStatus>,
    /// Monotonic revision; bumps on every mutation so clients can detect missed events.
    pub revision: u64,
}
