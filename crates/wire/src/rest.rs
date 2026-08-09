//! REST request/response bodies (PLAN §5). Defined once here; TS is generated, never
//! hand-written (CLAUDE.md non-negotiable #1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::{ChannelDescriptor, ChannelSettings},
    device::{DeviceInfo, DeviceSettings},
};

/// `GET /api/devices` — discovered hardware across all drivers (PLAN §5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DevicesResponse {
    pub devices: Vec<DeviceInfo>,
}

/// `POST /api/devicesets` — open a device into a new device set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateDeviceSetRequest {
    /// `driver:key`, matching a [`DeviceInfo`] from `GET /api/devices`.
    pub device_id: String,
}

/// `POST /api/devicesets/{ds}/channels` — add a channel to a device set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateChannelRequest {
    pub settings: ChannelSettings,
}

/// The channel types this server build offers, driving the "add channel" UI (PLAN §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelTypesResponse {
    pub types: Vec<ChannelDescriptor>,
}

/// The stored body of a preset: a full device-set + channels snapshot (PLAN §11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetSnapshot {
    /// Snapshot schema version, currently 1. Bump on any incompatible shape change so
    /// stored presets can be migrated or rejected explicitly.
    pub version: u32,
    /// `driver:key` of the device the preset was taken from.
    pub device_id: String,
    pub settings: DeviceSettings,
    pub channels: Vec<ChannelSettings>,
}

/// `GET /api/presets` list entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetInfo {
    pub id: i64,
    pub name: String,
    /// RFC3339 UTC.
    pub created_at: String,
    /// `driver:key` of the device the preset applies to.
    pub device_id: String,
}

/// `POST /api/presets` — snapshot a live device set under a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatePresetRequest {
    pub name: String,
    pub device_set: u32,
}

/// Apply a stored preset to a live device set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyPresetRequest {
    pub device_set: u32,
}

/// A stored frequency bookmark (PLAN §11).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Bookmark {
    pub id: i64,
    pub label: String,
    pub freq_hz: f64,
    /// Suggested channel type id (e.g. `"nfm"`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// `POST /api/bookmarks`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateBookmarkRequest {
    pub label: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// `POST /api/devicesets/{ds}/record` — start or stop recording the set's raw IQ stream
/// (PLAN §5: the recording path is lossless).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordRequest {
    pub action: RecordAction,
}

/// What a [`RecordRequest`] should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordAction {
    Start,
    Stop,
}

/// One finalized SigMF recording in the library (PLAN §11: the files on disk are the source
/// of truth; this row is its SQLite index entry).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingInfo {
    pub id: i64,
    /// Recording stem: file name without directory or `.sigmf-*` extension.
    pub file: String,
    /// `driver:key` that replays this recording (`virtual:file:<stem>`), usable directly
    /// in `POST /api/devicesets`.
    pub device_id: String,
    /// Label of the device the recording was captured from (SigMF `core:hw`).
    pub device_label: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub samples: u64,
    pub bytes: u64,
    pub duration_s: f64,
    /// RFC3339 UTC.
    pub created_at: String,
}

/// `GET /api/recordings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingsResponse {
    pub recordings: Vec<RecordingInfo>,
}

/// Identifier returned when an engine resource (device set, channel) is created.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatedId {
    pub id: u32,
}

/// Identifier returned when a persistence row (preset, bookmark) is created.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatedRowId {
    pub id: i64,
}

/// Uniform error body for REST failures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
