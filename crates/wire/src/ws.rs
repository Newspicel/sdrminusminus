use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{decode::DecodedRecord, position::PositionFix};

/// Granularity of a `StateChanged` invalidation. The client maps each scope to the
/// TanStack Query keys it must invalidate (: the *only* cache-invalidation path).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum StateScope {
    /// Full snapshot changed; refetch `GET /api/state`.
    All,
    /// The discovered-devices list changed.
    Devices,
    /// A single device set changed (settings, status, or its channels).
    DeviceSet(u32),
    /// The stored presets changed; refetch `GET /api/presets`.
    Presets,
    /// The stored bookmarks changed; refetch `GET /api/bookmarks`.
    Bookmarks,
    /// The recordings index changed; refetch `GET /api/recordings`.
    Recordings,
    /// The number of connected clients changed; refetch `GET /api/clients`. Server-owned
    /// state (connections are not an engine concept), so it travels through `emit_scope`
    /// like presets and bookmarks do.
    Clients,
    /// The stored decoder log changed *structurally* (cleared, pruned). Individual decodes
    /// arrive as [`ServerEvent::Decoded`] and are appended client-side — invalidating per
    /// decode would refetch the whole log hundreds of times a second under ADS-B traffic.
    DecoderLog,
    Calls,
    Workspaces,
}

/// Which binary stream a control event refers to. Every id is allocated per connection, from a
/// range per class, but only the pair `(kind, stream_id)` identifies a stream — events must
/// carry the kind or a spectrum stop is indistinguishable from an audio one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Spectrum,
    Audio,
    Video,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    /// First frame after connect: current state revision so the client can detect gaps.
    Hello {
        revision: u64,
    },
    /// Something changed; invalidate the matching query keys and refetch.
    StateChanged {
        scope: StateScope,
    },
    /// A subscribed spectrum stream is now active with this stream id (see [`crate::frame`]).
    ///
    /// The id is allocated per connection, exactly like an audio one: a multi-stream radio can
    /// have several lanes watched at once, so the device-set id is no longer enough to tell two
    /// spectra apart. `stream` names the lane this id carries — which is how the client knows
    /// which of its scopes the frames belong to, since the frame header carries only the id.
    StreamStarted {
        stream_id: u16,
        device_set: u32,
        #[serde(default)]
        stream: u32,
    },
    /// A subscribed audio stream is now active. Stream ids are allocated per-connection
    /// from the audio range (see [`StreamKind`]); clients demux binary frames by
    /// `(kind, stream_id)`.
    AudioStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    /// A subscribed video stream is now active, carrying the channel's pictures as
    /// [`crate::VideoFrame`]s. Ids come from the same per-connection media range audio uses, so
    /// the client demuxes on `(kind, stream_id)` exactly as it does there.
    VideoStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    /// A subscribed stream stopped; `kind` says which one, since spectrum and audio ids
    /// come from different spaces.
    StreamStopped {
        stream_id: u16,
        kind: StreamKind,
    },
    Decoded(Box<DecodedRecord>),
    /// What the decoders heard shortly before this client connected, oldest first, sent once
    /// after [`ServerEvent::Hello`].
    ///
    /// Decodes are pushed and never replayed, so a reloaded browser used to start with an empty
    /// map and refill it only as contacts happened to transmit again — a gap in the server, since
    /// the engine had been decoding the whole time. The server keeps a bounded, in-memory buffer
    /// of the last few records per station and hands it over on connect.
    ///
    /// Raw records rather than merged tracks, on purpose: the client already merges a position
    /// frame onto an earlier identity frame, and replaying what it would have received reaches
    /// the same state through the same code instead of a second implementation of that rule.
    /// Records are aggregated by station id, so an event with no identity is never in here.
    DecodedBacklog {
        records: Vec<DecodedRecord>,
    },
    DecodedLost {
        count: u64,
    },
    /// Live frequency-scanner progress. Its own event rather than a `StateChanged`:
    /// a scan retunes the device every dwell, and one full-state refetch per step would
    /// cost more than the scan does. The authoritative copy is `DeviceSet.scanner`, which
    /// this mirrors; a `StateChanged { DeviceSet }` still fires when a scan starts or stops.
    ScannerUpdate {
        device_set: u32,
        status: Box<crate::scan::ScannerStatus>,
    },
    /// Latest state of one GPS source node. Exactly one of `fix` and `error` is present; an error
    /// means the source has gone unavailable and consumers stop using its previous fix.
    PositionChanged {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Non-fatal server-side error surfaced to the client.
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ClientCommand {
    /// Start receiving spectrum frames for a device set at the requested rate/resolution.
    SubscribeSpectrum {
        device_set: u32,
        /// Requested frame rate; server clamps to its supported range.
        fps: u16,
        /// Requested display bins (≤ 4096); server clamps.
        bins: u16,
        /// Which receive stream's spectrum. Defaults to 0 so a client that predates
        /// multi-stream devices keeps its subscription; which stream a binary frame carries is
        /// implicit in this subscription, never a frame header field.
        #[serde(default)]
        stream: u32,
    },
    /// Stop one receive stream's spectrum. `stream` defaults to 0, so a client that predates
    /// multi-stream radios ends the subscription it started; without it, unsubscribing one scope
    /// would silence every other lane of the same radio on this connection.
    UnsubscribeSpectrum {
        device_set: u32,
        #[serde(default)]
        stream: u32,
    },
    /// Start receiving Opus audio frames for a channel; answered with `AudioStreamStarted`.
    SubscribeAudio { device_set: u32, channel: u32 },
    /// Stop the audio stream for a channel.
    UnsubscribeAudio { device_set: u32, channel: u32 },
    /// Start receiving pictures from a channel that produces them (`ChannelDescriptor.has_video`);
    /// answered with `VideoStreamStarted`. A channel with no video refuses rather than opening a
    /// stream that would never carry a frame.
    SubscribeVideo { device_set: u32, channel: u32 },
    /// Stop the video stream for a channel.
    UnsubscribeVideo { device_set: u32, channel: u32 },
    /// A fix from the desktop WebView's geolocation provider. The server accepts this only for
    /// a device-position node in the active workspace.
    PublishPosition {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}
