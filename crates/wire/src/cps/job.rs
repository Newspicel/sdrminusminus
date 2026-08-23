use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::report::ConversionReport;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CpsJobKind {
    Read,
    Write,
    Identify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CpsJobState {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl CpsJobState {
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioIdent {
    pub reported_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bands: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsJob {
    pub id: u64,
    pub kind: CpsJobKind,
    pub model_id: String,
    pub port: String,
    pub state: CpsJobState,
    pub step: String,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codeplug_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio: Option<RadioIdent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ConversionReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl CpsJob {
    #[must_use]
    pub fn percent(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.done_bytes as f32 / self.total_bytes as f32 * 100.0).clamp(0.0, 100.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsJobsResponse {
    pub jobs: Vec<CpsJob>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsReadRequest {
    pub model_id: String,
    pub port: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsWriteRequest {
    pub model_id: String,
    pub port: String,
    pub codeplug_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
    pub confirm: bool,
    #[serde(default)]
    pub restore_image: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsIdentifyRequest {
    pub model_id: String,
    pub port: String,
}
