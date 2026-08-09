//! The authoritative state model (PLAN §2: the server is the single source of truth).
//! `GET /api/state` returns [`StateSnapshot`]; clients converge via WS `StateChanged`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::ChannelInfo,
    device::{Capabilities, DeviceInfo, DeviceSettings},
};

/// Runtime status of a device set's capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceSetStatus {
    Idle,
    Running,
    Error,
}

/// One opened device and everything hosted on it (PLAN §2: "one device set per opened device").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSet {
    pub id: u32,
    pub device: DeviceInfo,
    pub capabilities: Capabilities,
    pub settings: DeviceSettings,
    pub status: DeviceSetStatus,
    pub channels: Vec<ChannelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Full state snapshot for initial load (PLAN §5 `GET /api/state`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StateSnapshot {
    pub device_sets: Vec<DeviceSet>,
    /// Monotonic revision; bumps on every mutation so clients can detect missed events.
    pub revision: u64,
}
