//! WebSocket JSON messages (PLAN §5). Tagged enums → TS discriminated unions the client
//! can exhaustively `switch` on. High-rate data (spectrum, audio) travels as binary frames
//! ([`crate::frame`]); this module is the low-rate control/event channel.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
    /// A subscribed stream stopped.
    StreamStopped { stream_id: u16 },
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
}
