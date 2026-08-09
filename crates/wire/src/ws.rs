//! WebSocket JSON messages (PLAN §5). Tagged enums → TS discriminated unions the client
//! can exhaustively `switch` on. High-rate data (spectrum, audio) travels as binary frames
//! ([`crate::frame`]); this module is the low-rate control/event channel.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::decode::DecodedRecord;

/// Granularity of a `StateChanged` invalidation. The client maps each scope to the
/// TanStack Query keys it must invalidate (PLAN §10: the *only* cache-invalidation path).
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
}

/// Which binary stream a control event refers to. Spectrum stream ids are device-set ids
/// (< 0x8000) and audio ids are connection-allocated from `0x8000..=0xFFFF`, but only the
/// pair `(kind, stream_id)` identifies a stream — events must carry the kind or a spectrum
/// stop is indistinguishable from an audio stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Spectrum,
    Audio,
}

/// Server → client push (PLAN §5). Adjacently tagged so unit variants stay compact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    /// First frame after connect: current state revision so the client can detect gaps.
    Hello { revision: u64 },
    /// Something changed; invalidate the matching query keys and refetch.
    StateChanged { scope: StateScope },
    /// A subscribed binary stream is now active with this stream id (see [`crate::frame`]).
    StreamStarted { stream_id: u16, device_set: u32 },
    /// A subscribed audio stream is now active. Stream ids are allocated per-connection
    /// from the audio range (see [`StreamKind`]); clients demux binary frames by
    /// `(kind, stream_id)`.
    AudioStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    /// A subscribed stream stopped; `kind` says which one, since spectrum and audio ids
    /// come from different spaces.
    StreamStopped { stream_id: u16, kind: StreamKind },
    /// A decoder produced a frame (PLAN §5: typed JSON decoder output). Pushed to every
    /// connected client; the same record is persisted to the decoder log (PLAN §11).
    ///
    /// Boxed so one rare variant does not set the size of every `ServerEvent`: the control
    /// broadcast carries hundreds of buffered `StateChanged`s, which would each pay for a
    /// record they never hold. `Box` is transparent to serde and to the schema.
    Decoded(Box<DecodedRecord>),
    /// Decoder frames were dropped before reaching clients or the log because a consumer
    /// fell behind. Loss is surfaced, never silent (PLAN §5).
    DecodedLost { count: u64 },
    /// Live frequency-scanner progress (M5). Its own event rather than a `StateChanged`:
    /// a scan retunes the device every dwell, and one full-state refetch per step would
    /// cost more than the scan does. The authoritative copy is `DeviceSet.scanner`, which
    /// this mirrors; a `StateChanged { DeviceSet }` still fires when a scan starts or stops.
    ScannerUpdate {
        device_set: u32,
        status: Box<crate::scan::ScannerStatus>,
    },
    /// Non-fatal server-side error surfaced to the client.
    Error { message: String },
}

/// Client → server commands over the same socket (PLAN §5). Stream subscriptions are
/// per-connection, so a phone can request a lighter stream than a desktop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ClientCommand {
    /// Start receiving spectrum frames for a device set at the requested rate/resolution.
    SubscribeSpectrum {
        device_set: u32,
        /// Requested frame rate; server clamps to its supported range.
        fps: u16,
        /// Requested display bins (≤ 4096, PLAN §9); server clamps.
        bins: u16,
    },
    /// Stop the spectrum stream for a device set.
    UnsubscribeSpectrum { device_set: u32 },
    /// Start receiving Opus audio frames for a channel; answered with `AudioStreamStarted`.
    SubscribeAudio { device_set: u32, channel: u32 },
    /// Stop the audio stream for a channel.
    UnsubscribeAudio { device_set: u32, channel: u32 },
}
