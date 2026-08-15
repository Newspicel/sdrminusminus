use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Longest stored `host:port` destination. Long enough for an IPv6 literal or DNS name while
/// bounding workspace and request payloads.
pub const MAX_NETWORK_ADDRESS_LEN: usize = 255;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkTransport {
    #[default]
    Udp,
    Tcp,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkSampleFormat {
    /// SigMF `cf32_le`: interleaved IEEE-754 `f32`, little-endian I then Q.
    #[default]
    Cf32Le,
    /// SigMF `ci16_le`, normalized to ±32767.
    Ci16Le,
    /// SigMF `cu8`, compatible with RTL-SDR sample bytes and centered at 127.5.
    Cu8,
}

impl NetworkSampleFormat {
    #[must_use]
    pub const fn bytes_per_sample(self) -> usize {
        match self {
            Self::Cf32Le => 8,
            Self::Ci16Le => 4,
            Self::Cu8 => 2,
        }
    }
}

fn default_network_address() -> String {
    "127.0.0.1:7355".to_owned()
}

/// An unframed, interleaved IQ stream sent to a network analysis tool.
///
/// UDP preserves datagram boundaries but carries no sequence header. TCP is one continuous byte
/// stream. In both cases the receiver must be configured with the radio's sample rate and center
/// frequency separately.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct NetworkExportSettings {
    pub transport: NetworkTransport,
    pub format: NetworkSampleFormat,
    pub address: String,
}

impl Default for NetworkExportSettings {
    fn default() -> Self {
        Self {
            transport: NetworkTransport::Udp,
            format: NetworkSampleFormat::Cf32Le,
            address: default_network_address(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetworkExportNode {
    #[serde(flatten)]
    pub settings: NetworkExportSettings,
}

/// `POST /api/devicesets/{ds}/network-export`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetworkExportRequest {
    pub action: NetworkExportAction,
    /// Patch-node identity. A set permits one active exporter and only its owner may stop it.
    pub node: String,
    #[serde(default)]
    pub stream: u32,
    #[serde(default)]
    pub settings: NetworkExportSettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkExportAction {
    Start,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetworkExportStatus {
    pub node: String,
    pub stream: u32,
    pub settings: NetworkExportSettings,
    pub sample_rate: u64,
    pub center_hz: i64,
    pub samples: u64,
    pub bytes: u64,
    pub packets: u64,
    /// Capture-ring samples lost while this export was active.
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
