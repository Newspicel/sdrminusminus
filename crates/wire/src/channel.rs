//! Channel model. Typed per-channel settings (PLAN §8): each demod owns a params struct
//! here, tagged into [`ChannelParams`] so the client gets a discriminated union.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Static description of a channel type, surfaced to drive the "add channel" UI (PLAN §8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelDescriptor {
    /// Stable type id, e.g. `"nfm"`, `"am"`, `"ssb"`, `"wfm"`.
    pub type_id: String,
    /// Display name, e.g. `"NFM"`, `"WFM (mono)"`.
    pub name: String,
    /// Nominal RF bandwidth the channel needs, in Hz.
    pub bandwidth_hz: f64,
    /// IQ rate the demod expects from the DDC, in Hz.
    pub input_rate_hz: f64,
}

fn default_nfm_bandwidth_hz() -> f64 {
    12_500.0
}

fn default_am_bandwidth_hz() -> f64 {
    10_000.0
}

fn default_ssb_bandwidth_hz() -> f64 {
    2_700.0
}

fn default_agc() -> bool {
    true
}

fn default_deemphasis_us() -> f32 {
    50.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NfmParams {
    #[serde(default = "default_nfm_bandwidth_hz")]
    pub bandwidth_hz: f64,
}

impl Default for NfmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_nfm_bandwidth_hz(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AmParams {
    #[serde(default = "default_am_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_agc")]
    pub agc: bool,
}

impl Default for AmParams {
    fn default() -> Self {
        Self {
            bandwidth_hz: default_am_bandwidth_hz(),
            agc: default_agc(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sideband {
    #[default]
    Usb,
    Lsb,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SsbParams {
    #[serde(default)]
    pub sideband: Sideband,
    #[serde(default = "default_ssb_bandwidth_hz")]
    pub bandwidth_hz: f64,
    #[serde(default = "default_agc")]
    pub agc: bool,
}

impl Default for SsbParams {
    fn default() -> Self {
        Self {
            sideband: Sideband::default(),
            bandwidth_hz: default_ssb_bandwidth_hz(),
            agc: default_agc(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WfmParams {
    /// De-emphasis time constant in µs (50 in most of the world, 75 in the Americas).
    #[serde(default = "default_deemphasis_us")]
    pub deemphasis_us: f32,
}

impl Default for WfmParams {
    fn default() -> Self {
        Self {
            deemphasis_us: default_deemphasis_us(),
        }
    }
}

/// Type-discriminated demod parameters. Adjacently tagged so the generated TS is a
/// discriminated union on `type`, and `{"type":"nfm","settings":{}}` deserializes with
/// every field at its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum ChannelParams {
    Nfm(NfmParams),
    Am(AmParams),
    Ssb(SsbParams),
    Wfm(WfmParams),
}

impl ChannelParams {
    /// The stable type id, matching [`ChannelDescriptor::type_id`].
    #[must_use]
    pub fn type_id(&self) -> &'static str {
        match self {
            Self::Nfm(_) => "nfm",
            Self::Am(_) => "am",
            Self::Ssb(_) => "ssb",
            Self::Wfm(_) => "wfm",
        }
    }
}

/// Per-channel settings: where the channel sits and how it demodulates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelSettings {
    /// Offset from the device center frequency, in Hz.
    #[serde(default)]
    pub offset_hz: f64,
    /// Squelch threshold in dBFS, measured on the channel-filtered IQ (the mode's occupied
    /// bandwidth, not the full DDC passband); `None` = squelch open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch_db: Option<f32>,
    pub params: ChannelParams,
}

/// A live channel instance inside a device set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelInfo {
    pub id: u32,
    pub settings: ChannelSettings,
}
