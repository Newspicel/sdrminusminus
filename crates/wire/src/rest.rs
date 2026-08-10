//! REST request/response bodies (PLAN §5). Defined once here; TS is generated, never
//! hand-written (CLAUDE.md non-negotiable #1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::{ChannelDescriptor, ChannelSettings},
    decode::DecoderEvent,
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

/// One stored decoder frame (PLAN §11: decoder logs are queryable and exportable, not
/// scroll-back-only). The typed `event` is stored verbatim so an export loses nothing;
/// `kind`, `summary` and `station` are the indexed projections the list view filters on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecoderLogEntry {
    pub id: i64,
    /// RFC3339 UTC.
    pub at: String,
    pub device_set: u32,
    pub channel: u32,
    /// [`crate::DecoderEvent::kind`] of `event`.
    pub kind: String,
    pub freq_hz: f64,
    /// Emitter identity within the decoder (ICAO, MMSI, callsign, pager address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    pub summary: String,
    pub event: DecoderEvent,
}

/// `GET /api/decoderlog` — newest first, bounded by the requested `limit`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecoderLogResponse {
    pub entries: Vec<DecoderLogEntry>,
    /// Rows matching the filter, ignoring `limit`.
    pub total: u64,
    /// Frames dropped on the way to the log since the server started, because a consumer
    /// fell behind (PLAN §5: bounded queues surface their loss).
    pub dropped: u64,
}

/// Export format for `GET /api/decoderlog/export/{format}` (PLAN §11: CSV/JSON). It is a
/// path segment, not a query field: `serde_urlencoded` cannot flatten a struct, so sharing
/// [`DecoderLogQuery`] across list/export/clear requires the format to live elsewhere.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Csv,
    Json,
}

/// Filters shared by the decoder-log list, export and clear endpoints. Every field is
/// optional; an empty query means "everything".
#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct DecoderLogQuery {
    /// Restrict to one decoder ([`crate::DecoderEvent::kind`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Restrict to one device set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_set: Option<u32>,
    /// Only entries at or after this RFC3339 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Only entries at or before this RFC3339 timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    /// Substring match against `station` and `summary`, case-insensitive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    /// Maximum rows returned by the list endpoint (server-clamped). Ignored by export.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `DELETE /api/decoderlog` — how many rows the filtered clear removed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCount {
    pub deleted: u64,
}

/// One built-in station template (PLAN §10: the template gallery). Read-only and
/// device-agnostic — unlike a [`PresetSnapshot`] it names no device, so the same entry
/// applies to whatever hardware is open, provided the device can tune it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TemplateInfo {
    /// Stable slug used in `POST /api/templates/{id}/apply`.
    pub id: String,
    pub name: String,
    /// One line for the gallery card.
    pub description: String,
    /// The "what am I looking at" text shown once it is applied (PLAN §10).
    pub explainer: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    /// Channels the template creates on the target device set.
    pub channels: Vec<ChannelSettings>,
    /// Tuning span the template needs, so the gallery can mark entries the open device
    /// cannot reach instead of failing on apply.
    pub min_freq_hz: f64,
    pub max_freq_hz: f64,
    /// The patch the template draws into the active workspace (CANVAS §8 phase ④): a receiver,
    /// the channels above, their wiring and the faces to operate them. Its channel nodes are the
    /// `channels` list in order, so the n-th node binds the n-th channel the apply creates. A
    /// template that names no patch leaves the station alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<crate::patch::PatchGraph>,
}

/// `GET /api/templates`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TemplatesResponse {
    pub templates: Vec<TemplateInfo>,
}

/// Apply a built-in template to a live device set.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyTemplateRequest {
    pub device_set: u32,
}

/// `GET /api/clients` — how many clients share this server right now (PLAN §16 M5
/// multi-client). Includes the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClientsResponse {
    pub clients: u32,
}

/// `GET /api/auth` — unauthenticated, so a client knows whether to ask for a token before
/// its first real request (PLAN §12: optional single shared token).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AuthInfo {
    /// Whether this server rejects requests without the shared token.
    pub token_required: bool,
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
