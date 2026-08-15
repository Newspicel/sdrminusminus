use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

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
    #[default]
    Cf32Le,
    Ci16Le,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NetworkExportRequest {
    pub action: NetworkExportAction,
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
    pub overruns: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
