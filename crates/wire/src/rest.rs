//! REST request/response bodies (PLAN §5). Defined once here; TS is generated, never
//! hand-written (CLAUDE.md non-negotiable #1).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{channel::ChannelSettings, device::DeviceInfo};

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
    pub settings: ChannelSettings,
}

/// Identifier returned when a resource is created.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatedId {
    pub id: u32,
}

/// Uniform error body for REST failures.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
