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
    /// Which of the device's receive streams the channel taps. Defaults to 0 so a client that
    /// predates multi-stream devices keeps meaning the only stream its radio has; out of range
    /// is a bad request naming the count, never a silent fallback.
    #[serde(default)]
    pub stream: u32,
    pub settings: ChannelSettings,
}

/// The channel types this server build offers, driving the "add channel" UI (PLAN §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelTypesResponse {
    pub types: Vec<ChannelDescriptor>,
}

/// Shape version of a stored [`PresetSnapshot`]. Read like [`crate::WORKSPACE_SNAPSHOT_VERSION`]:
/// a blob this build did not write is refused rather than guessed at.
///
/// Version 2 is the workspace preset. Version 1 held one device set — its settings and its
/// channels — and stored rows do not migrate: a v1 preset names a radio and no workspace, so
/// there is nothing to say which of a patch's radios it was meant for.
pub const PRESET_SNAPSHOT_VERSION: u32 = 2;

/// The stored body of a preset: where every radio a workspace draws was tuned, and what hung off
/// them (PLAN §11).
///
/// A preset is workspace-wide because a workspace is: an operator who saved "the morning airband
/// bench" means every radio on it, and a per-device preset made that several saves that could be
/// restored in the wrong order or half-applied. Applying one is one gesture over the whole patch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetSnapshot {
    /// [`PRESET_SNAPSHOT_VERSION`] at the time of writing.
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<PresetDevice>,
}

/// One device node's radio settings and channels, as the preset captured them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetDevice {
    /// [`crate::PatchNode::id`] of the device node this was captured from — the primary match on
    /// apply, and the only one that is right when a patch draws two of the same radio.
    pub node: String,
    /// `driver:key` of the radio it was captured on. The fallback match, so a preset still lands
    /// after the node was redrawn, and what the client names in the list.
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
    /// How many radios the preset carries, denormalized so the list never parses a blob.
    pub devices: u32,
}

/// `POST /api/presets` — snapshot the active workspace's radios under a name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatePresetRequest {
    pub name: String,
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
    /// Which receive stream a start records — one recording per set, on a named stream.
    /// Defaults to 0, the only stream a single-stream radio has.
    #[serde(default)]
    pub stream: u32,
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

/// `POST /api/devicesets/{ds}/playback` — drive the replay transport of a set whose device is
/// a recording. Looping is not an action here: it is the `loop` device setting, applied like
/// any other (see [`crate::PlaybackStatus`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackRequest {
    pub action: PlaybackAction,
    /// Where [`PlaybackAction::Seek`] should land, in samples from the start; clamped to the
    /// end of the recording. Ignored by every other action, and absent means the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_samples: Option<u64>,
}

/// What a [`PlaybackRequest`] should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackAction {
    Play,
    Pause,
    /// Pause and return to the start, in one step.
    Stop,
    Seek,
}

/// Container a recording is downloaded in. A query field rather than a path segment (unlike
/// [`ExportFormat`]): the format is optional here, and giving a path segment a default would
/// mean two routes for one resource.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFormat {
    /// The SigMF pair packed into one `.sigmf` tar (SigMF v1.2.6 §"Rules for SigMF Archive
    /// files"). Lossless: extract it and the pair is exactly what was recorded, replayable
    /// through `virtual:file:`.
    #[default]
    Sigmf,
    /// Interleaved I/Q as a two-channel 32-bit-float `.wav`, for HDSDR, SDR#, Audacity and
    /// `ffmpeg`. The samples survive exactly; of the metadata only the center frequency and
    /// the start time do.
    Wav,
}

/// `GET /api/recordings/{id}/download`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct RecordingDownloadQuery {
    #[serde(default)]
    pub format: RecordingFormat,
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
    /// [`crate::PatchNode::id`] of the channel node this frame came from, resolved against the
    /// active workspace when the row was written. This is the row's durable identity: `channel`
    /// above is an engine id, allocated per run and reused (CANVAS §3), so it names this frame's
    /// origin only for as long as that run lasted.
    ///
    /// Absent on rows written before the log recorded it, and on rows written while the channel
    /// was not bound to any node — a channel created outside a workspace, or one the binding had
    /// not caught up with yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
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
    /// Restrict to the channels named by patch node — [`crate::PatchNode::id`]s, comma separated
    /// (`channel:a1b2,channel:c3d4`).
    ///
    /// This is the filter a canvas node draws with its wires (CANVAS §1): a decoder-log or export
    /// node shows the decoders wired into it, and "wired into it" is a *set of channels*, which
    /// neither `kind` nor `device_set` can name. The node id is the durable half of that — engine
    /// channel ids are allocated per run and reused (CANVAS §3), so a scope built from them would
    /// hand a node another node's history after a restart.
    ///
    /// Read against the *active* workspace, which is the one whose canvas drew the scope. A node
    /// id is unique only within a workspace — templates author theirs as slugs and
    /// [`crate::WorkspaceSnapshot::merge_patch`] deduplicates only inside the workspace it merges
    /// into — so ids alone would let two workspaces built from the same template read each
    /// other's history. Rows written before the server recorded the workspace answer to no id.
    ///
    /// Composes with [`Self::sources`] as an OR, and the pair is one filter: absent means every
    /// channel, and *both* empty means none, so a node with nothing wired in matches nothing
    /// rather than everything.
    ///
    /// A node id containing a comma cannot be named here. Nothing generates one — ids are
    /// `kind:uuid` from the client and slugs from the templates — and the fallback below still
    /// reaches such a node's rows for the run they were written in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<String>,
    /// Fallback for rows that carry no node: `device_set:channel` pairs, comma separated
    /// (`0:1,0:2`), matched only against rows whose `node` is null *and* that this server run
    /// wrote.
    ///
    /// Two kinds of row have none. Rows written before the log recorded one, and rows written in
    /// the window between a channel starting to decode and the workspace binding catching up with
    /// it. Both are attributable only by the coordinates they *do* carry, and only for the run
    /// that wrote them — engine channel ids are allocated per run and reused (CANVAS §3), so `0:1`
    /// names a different decoder in every run that had one. Hence the run bound, and hence this
    /// being the fallback rather than the filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<String>,
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

/// How many channels one wire scope may name, on either side. The graph itself is bounded at
/// [`crate::patch::MAX_EDGES`] wires, so no node the canvas can draw exceeds this; the cap is what
/// stops a hand-written URL from turning into a thousand-term `WHERE`.
pub const MAX_LOG_SOURCES: usize = crate::patch::MAX_EDGES;

/// The channels a decoder-log or export node is wired to, by both names a stored row can answer
/// to. A row is in scope when its `node` is one of [`Self::nodes`], or it has no node and its
/// coordinates are one of [`Self::channels`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogScope {
    /// Patch node ids — durable across runs, and how all but the oldest rows are matched.
    pub nodes: Vec<String>,
    /// `(device set, channel)` pairs, for the rows that carry no node.
    pub channels: Vec<(u32, u32)>,
}

impl LogScope {
    /// Whether this scope can match anything at all. An empty scope is a node with nothing wired
    /// into it, which matches no row rather than every row.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.channels.is_empty()
    }
}

impl DecoderLogQuery {
    /// The wire scope this query carries, or `None` when it names neither half — an unscoped
    /// query over the whole log, which is what a caller that is not looking at the canvas sends.
    ///
    /// `Err` carries the fragment that would not read. A malformed list is a rejected request and
    /// never an ignored one: dropping it would *widen* the query, and a node would quietly show
    /// frames it is not wired to — or, on the clear endpoint, delete them.
    ///
    /// # Errors
    /// Either list names more than [`MAX_LOG_SOURCES`] channels, a node id is empty or longer
    /// than [`crate::patch::MAX_NODE_ID_LEN`], or a source is not `device_set:channel` with both
    /// sides a `u32`.
    pub fn scope(&self) -> Result<Option<LogScope>, &str> {
        if self.nodes.is_none() && self.sources.is_none() {
            return Ok(None);
        }
        Ok(Some(LogScope {
            nodes: parse_list(self.nodes.as_deref(), |id| {
                if id.is_empty() || id.len() > crate::patch::MAX_NODE_ID_LEN {
                    Err(id)
                } else {
                    Ok(id.to_owned())
                }
            })?,
            channels: parse_list(self.sources.as_deref(), |source| {
                let (set, channel) = source.split_once(':').ok_or(source)?;
                Ok((
                    set.parse().map_err(|_| source)?,
                    channel.parse().map_err(|_| source)?,
                ))
            })?,
        }))
    }
}

/// A comma-separated list, read through `item`. Absent and empty both mean no entries — the
/// difference between them is [`DecoderLogQuery::scope`]'s to make, over both lists at once.
fn parse_list<T>(
    list: Option<&str>,
    item: impl Fn(&str) -> Result<T, &str>,
) -> Result<Vec<T>, &str> {
    let Some(list) = list.filter(|list| !list.is_empty()) else {
        return Ok(Vec::new());
    };
    let fragments: Vec<&str> = list.split(',').collect();
    if fragments.len() > MAX_LOG_SOURCES {
        return Err(list);
    }
    fragments.into_iter().map(item).collect()
}

/// `DELETE /api/decoderlog` — how many rows the filtered clear removed.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCount {
    pub deleted: u64,
}

/// One built-in workspace template (PLAN §10: the template gallery). Read-only and
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
    /// template that names no patch leaves the workspace alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<crate::patch::PatchGraph>,
    /// Which direction the radio has to have. Every built-in template receives; the field is
    /// here because "what kind of radio does this need" is the question the picker asks, and a
    /// transmit template must not be offered on a receiver the day one exists.
    #[serde(default = "receive")]
    pub direction: crate::device::Direction,
    /// Whether `sample_rate` is the only rate that works, rather than a starting point. ADS-B
    /// fills its whole 2 MHz channel, so a resampled one decodes nothing (PLAN §18) — a radio
    /// whose rate menu misses 2 Msps cannot run it at all, while an FM template is happy at
    /// anything wide enough.
    #[serde(default)]
    pub exact_rate: bool,
    /// Devices this template can actually run on, as `driver:key` handles — the server's answer,
    /// computed against each probed radio's [`crate::DeviceProfile`], so the gallery offers a
    /// device only when the template fits it.
    ///
    /// A radio whose driver reports no profile is *included*: unknown is not the same as
    /// unsuitable, and hiding a device because its backend cannot answer cheaply would make the
    /// picker lie. Empty on the static table; filled in per request.
    ///
    /// Always serialized, unlike the other quiet fields: "no attached radio can run this" is a
    /// real answer the gallery has to render, and eliding it would make it arrive as the absence
    /// that means "nobody asked".
    #[serde(default)]
    pub supported_devices: Vec<String>,
}

/// Templates receive unless they say otherwise — the direction a stored or peer-sent entry that
/// predates the field describes.
const fn receive() -> crate::device::Direction {
    crate::device::Direction::Rx
}

/// How far a rate may sit from a template's nominal one and still count, when the template does
/// not demand an exact match. Wide enough to accept a 2.048 Msps dongle for a 2.4 Msps template,
/// which is the same radio doing the same job.
const RATE_TOLERANCE: f64 = 0.25;

impl TemplateInfo {
    /// Why this radio cannot run the template, or `None` if it can.
    ///
    /// One implementation, in `wire`, because both sides need the answer and two copies would
    /// drift: the server evaluates it against every probed radio to fill
    /// [`supported_devices`](Self::supported_devices), and the client renders the reason.
    ///
    /// Conservative on purpose — a profile that advertises nothing is not refused. The engine
    /// still validates on apply; this is what keeps the operator from being offered a template
    /// that will fail.
    #[must_use]
    pub fn unmet_by(&self, profile: &crate::device::DeviceProfile) -> Option<String> {
        if !profile.duplex.supports(self.direction) {
            return Some(format!("this radio does not {}", self.direction));
        }
        if !profile.reaches(self.min_freq_hz) || !profile.reaches(self.max_freq_hz) {
            return Some(format!(
                "needs {:.3}–{:.3} MHz, outside this radio's tuning range",
                self.min_freq_hz / 1e6,
                self.max_freq_hz / 1e6
            ));
        }
        let tolerance = if self.exact_rate { 0.0 } else { RATE_TOLERANCE };
        if !profile.runs_at(self.sample_rate, tolerance) {
            return Some(if self.exact_rate {
                format!(
                    "needs exactly {:.3} Msps, which this radio does not offer",
                    self.sample_rate / 1e6
                )
            } else {
                format!(
                    "needs about {:.3} Msps, which this radio does not offer",
                    self.sample_rate / 1e6
                )
            });
        }
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Capabilities, DeviceProfile, Duplex, Range, StreamScope};

    fn scoped(nodes: Option<&str>, sources: Option<&str>) -> DecoderLogQuery {
        DecoderLogQuery {
            nodes: nodes.map(str::to_owned),
            sources: sources.map(str::to_owned),
            ..DecoderLogQuery::default()
        }
    }

    /// The three readings that must stay apart: naming neither list is every channel, naming an
    /// empty one is no channel, and a malformed one is a refusal. Collapsing any pair widens a
    /// query that was asked to narrow — which for the clear endpoint is the difference between
    /// emptying one node's rows and emptying the log.
    #[test]
    fn a_wire_scope_reads_both_lists_and_refuses_anything_else() {
        assert_eq!(scoped(None, None).scope(), Ok(None));
        assert_eq!(
            scoped(Some(""), Some("")).scope(),
            Ok(Some(LogScope::default()))
        );
        assert!(
            scoped(Some(""), Some(""))
                .scope()
                .unwrap()
                .unwrap()
                .is_empty()
        );

        assert_eq!(
            scoped(Some("channel:a1,ch0"), Some("0:1,2:13")).scope(),
            Ok(Some(LogScope {
                nodes: vec!["channel:a1".to_owned(), "ch0".to_owned()],
                channels: vec![(0, 1), (2, 13)],
            }))
        );
        // Either half alone is a scope; the other simply matches nothing.
        assert_eq!(
            scoped(Some("ch0"), None).scope(),
            Ok(Some(LogScope {
                nodes: vec!["ch0".to_owned()],
                channels: Vec::new(),
            }))
        );

        for bad in ["0", "0:", ":1", "0:1,", "a:1", "0:-1", "0:1:2"] {
            assert!(scoped(None, Some(bad)).scope().is_err(), "{bad}");
        }
        assert!(scoped(Some("a,,b"), None).scope().is_err());
        assert!(
            scoped(Some(&"n".repeat(crate::patch::MAX_NODE_ID_LEN + 1)), None)
                .scope()
                .is_err()
        );

        let outsize = |item: fn(usize) -> String| {
            (0..=MAX_LOG_SOURCES)
                .map(item)
                .collect::<Vec<_>>()
                .join(",")
        };
        assert!(
            scoped(None, Some(&outsize(|n| format!("0:{n}"))))
                .scope()
                .is_err()
        );
        assert!(
            scoped(Some(&outsize(|n| format!("ch{n}"))), None)
                .scope()
                .is_err()
        );
    }

    fn profile(freq: Vec<Range>, rates: Vec<f64>, duplex: Duplex) -> DeviceProfile {
        Capabilities {
            freq_ranges: freq,
            sample_rates: rates,
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
        }
        .profile()
    }

    fn range(min: f64, max: f64) -> Range {
        Range {
            min,
            max,
            step: None,
        }
    }

    fn template(min_freq_hz: f64, max_freq_hz: f64, sample_rate: f64) -> TemplateInfo {
        TemplateInfo {
            id: "t".to_string(),
            name: "T".to_string(),
            description: String::new(),
            explainer: String::new(),
            center_hz: min_freq_hz,
            sample_rate,
            channels: Vec::new(),
            min_freq_hz,
            max_freq_hz,
            patch: None,
            direction: crate::device::Direction::Rx,
            exact_rate: false,
            supported_devices: Vec::new(),
        }
    }

    /// An RTL-SDR reaches 1090 MHz and an HF-only radio does not; the reason names the span, so
    /// the gallery can say why rather than just greying the card out.
    #[test]
    fn a_template_out_of_a_radios_tuning_range_is_refused_with_the_span() {
        let adsb = template(1_090e6, 1_090e6, 2e6);
        let dongle = profile(vec![range(24e6, 1.766e9)], vec![2e6, 2.4e6], Duplex::RxOnly);
        assert_eq!(adsb.unmet_by(&dongle), None);

        let hf = profile(vec![range(0.0, 30e6)], vec![2e6], Duplex::RxOnly);
        let reason = adsb.unmet_by(&hf).expect("out of range");
        assert!(reason.contains("1090.000"), "{reason}");
    }

    /// ADS-B fills its whole channel, so 2.048 Msps is not "close enough" — while an FM template
    /// at a nominal 2.4 Msps is happy on the same dongle.
    #[test]
    fn an_exact_rate_template_refuses_a_neighbouring_rate() {
        let dongle = profile(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);

        let mut adsb = template(1_090e6, 1_090e6, 2e6);
        adsb.exact_rate = true;
        let reason = adsb.unmet_by(&dongle).expect("2.048 is not 2.000");
        assert!(reason.contains("exactly"), "{reason}");

        let fm = template(98e6, 98e6, 2.4e6);
        assert_eq!(fm.unmet_by(&dongle), None, "a nominal rate tolerates 2.048");
    }

    #[test]
    fn a_transmit_template_is_refused_on_a_receiver() {
        let mut beacon = template(144e6, 144e6, 1e6);
        beacon.direction = crate::device::Direction::Tx;

        let receiver = profile(vec![range(24e6, 1.766e9)], vec![1e6], Duplex::RxOnly);
        let reason = beacon.unmet_by(&receiver).expect("a receiver cannot send");
        assert!(reason.contains("transmitting"), "{reason}");

        let transceiver = profile(vec![range(1e6, 6e9)], vec![1e6], Duplex::Half);
        assert_eq!(beacon.unmet_by(&transceiver), None);
    }

    /// A radio that advertises nothing is not refused: the virtual devices report no ranges, and
    /// a filter that hid them would hide the only radio CI has.
    #[test]
    fn a_radio_that_advertises_nothing_is_not_refused() {
        let unknown = profile(Vec::new(), Vec::new(), Duplex::RxOnly);
        assert_eq!(template(1_090e6, 1_090e6, 2e6).unmet_by(&unknown), None);
    }

    /// A v1 preset is one radio's settings with no workspace to put them in. It must fail the
    /// version check rather than deserialize into an empty v2 preset that applies cleanly and
    /// changes nothing.
    #[test]
    fn a_v1_preset_is_not_a_workspace_preset() {
        let v1 = serde_json::json!({
            "version": 1,
            "device_id": "virtual:siggen",
            "settings": {},
            "channels": [],
        });
        let parsed: PresetSnapshot = serde_json::from_value(v1).expect("the shape still parses");
        assert_ne!(parsed.version, PRESET_SNAPSHOT_VERSION);
        assert!(parsed.devices.is_empty());
    }

    /// The field is new; a peer that predates it describes a receive template at a nominal rate.
    #[test]
    fn an_older_template_payload_reads_as_receive() {
        let parsed: TemplateInfo = serde_json::from_str(
            r#"{"id":"t","name":"T","description":"","explainer":"","center_hz":98e6,
                "sample_rate":2.4e6,"channels":[],"min_freq_hz":98e6,"max_freq_hz":98e6}"#,
        )
        .expect("a template from before the direction field");
        assert_eq!(parsed.direction, crate::device::Direction::Rx);
        assert!(!parsed.exact_rate);
        assert!(parsed.supported_devices.is_empty());
    }
}
