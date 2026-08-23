use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::codeplug::{Codeplug, CodeplugCounts};

pub const MAX_CPS_NAME_LEN: usize = 64;
pub const MAX_CPS_NOTE_LEN: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsUser {
    pub id: i64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmr_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsUserRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dmr_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsDevice {
    pub id: i64,
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsDeviceRequest {
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsCodeplugInfo {
    pub id: i64,
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    pub counts: CodeplugCounts,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsCodeplugDetail {
    #[serde(flatten)]
    pub info: CpsCodeplugInfo,
    pub codeplug: Codeplug,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsCodeplugRequest {
    pub name: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    pub codeplug: Codeplug,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsLibraryResponse {
    pub users: Vec<CpsUser>,
    pub devices: Vec<CpsDevice>,
    pub codeplugs: Vec<CpsCodeplugInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsConvertRequest {
    pub target_model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    #[serde(default)]
    pub store: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsConvertResponse {
    pub report: super::report::ConversionReport,
    pub codeplug: Codeplug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    #[default]
    Replace,
    Append,
    Union,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsMergeRequest {
    pub source_id: i64,
    #[serde(default)]
    pub mode: MergeMode,
    #[serde(default)]
    pub parts: Vec<MergePart>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MergePart {
    Contacts,
    GroupLists,
    Channels,
    Zones,
    ScanLists,
    RadioIds,
    Settings,
}

impl MergePart {
    pub const ALL: [Self; 7] = [
        Self::Contacts,
        Self::GroupLists,
        Self::Channels,
        Self::Zones,
        Self::ScanLists,
        Self::RadioIds,
        Self::Settings,
    ];
}
