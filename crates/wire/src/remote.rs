use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::{DeviceFault, DeviceSetStatus};

pub const DEFAULT_REMOTE_APP: &str = "https://app.sdrmm.com";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteState {
    Unpaired,
    Pairing,
    Connecting,
    Online,
    Retrying,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RemoteStatus {
    pub state: RemoteState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub app_origin: String,
    pub via_relay: bool,
}

pub const MAX_HEALTH_RADIOS: usize = 16;
pub const MAX_HEALTH_LABEL_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteHealth {
    pub version: String,
    pub platform: String,
    pub started_at: u64,
    pub clients: u32,
    pub radios: Vec<RadioHealth>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadioHealth {
    pub label: String,
    pub driver: String,
    pub status: DeviceSetStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fault: Option<DeviceFault>,
    pub channels: u32,
    pub overruns: u64,
    pub recording: bool,
}
