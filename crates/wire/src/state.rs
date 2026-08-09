//! The authoritative state model (PLAN §2: the server is the single source of truth).
//! `GET /api/state` returns [`StateSnapshot`]; clients converge via WS `StateChanged`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    device::{Capabilities, DeviceInfo, DeviceSettings},
};

/// Runtime status of a device set's capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetStatus {
    Idle,
    Running,
    Error,
}

/// Live IQ recording on a device set (PLAN §5: the recording path is lossless, so a writer
/// fault must surface here rather than dropping samples silently).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingStatus {
    /// Recording stem: file name without directory or `.sigmf-*` extension.
    pub file: String,
    /// RFC3339 UTC.
    pub started_at: String,
    /// Samples written to the `.sigmf-data` file so far.
    pub samples: u64,
    pub bytes: u64,
    /// Capture-ring drops while this recording ran. The file stays contiguous as the DSP
    /// plane saw the stream, so growth means the recording has upstream gaps (PLAN §5).
    pub overruns: u64,
    /// Fatal recording fault (queue overflow, disk error); the writer has stopped but the
    /// cause stays visible (CLAUDE.md no-silent-failure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One opened device and everything hosted on it (PLAN §2: "one device set per opened device").
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
    /// `status` stays `running` (PLAN §5 backpressure; CLAUDE.md no-silent-failure).
    #[serde(default)]
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Active IQ recording, if any (M3, PLAN §5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingStatus>,
}

/// Full state snapshot for initial load (PLAN §5 `GET /api/state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    /// Monotonic revision; bumps on every mutation so clients can detect missed events.
    pub revision: u64,
}
