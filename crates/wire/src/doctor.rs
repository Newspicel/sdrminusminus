use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DoctorCheck {
    pub id: String,
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DoctorReport {
    pub version: String,
    pub platform: String,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    #[must_use]
    pub fn worst(&self) -> CheckStatus {
        self.checks
            .iter()
            .map(|c| c.status)
            .fold(CheckStatus::Ok, |acc, s| match (acc, s) {
                (CheckStatus::Fail, _) | (_, CheckStatus::Fail) => CheckStatus::Fail,
                (CheckStatus::Warn, _) | (_, CheckStatus::Warn) => CheckStatus::Warn,
                _ => CheckStatus::Ok,
            })
    }
}
