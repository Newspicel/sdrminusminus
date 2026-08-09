//! Channel model. At M0 no demodulators exist yet (they arrive at M2, PLAN §16); this
//! carries the type/settings envelope so the engine, server, and UI share one shape.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Static description of a channel type, surfaced to drive the "add channel" UI (PLAN §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelDescriptor {
    /// Stable type id, e.g. `"nfm"`, `"am"`, `"adsb"`.
    pub type_id: String,
    pub name: String,
    /// Nominal RF bandwidth the channel needs, in Hz.
    pub bandwidth_hz: f64,
}

/// Per-channel settings envelope. Concrete typed settings land with each demod (PLAN §8);
/// until then this carries the type id plus a free-form params blob validated server-side.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelSettings {
    pub type_id: String,
    /// Offset from the device center frequency, in Hz.
    #[serde(default)]
    pub offset_hz: f64,
    /// Type-specific parameters. Replaced by generated typed structs as demods land.
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A live channel instance inside a device set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelInfo {
    pub id: u32,
    pub settings: ChannelSettings,
}
