//! Environment diagnostics (PLAN §15: `sdrmm --doctor` prints what's found — Soapy modules,
//! USB permissions, udev hints). The report is a wire type, not console text, so the CLI and
//! the web UI render one source of truth.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// How a check came out. `Warn` is "works, but something is degraded or absent"; `Fail` is
/// "this will not work as configured".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

/// One diagnostic line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DoctorCheck {
    /// Short stable identifier, e.g. `"backend.rtlsdr"`.
    pub id: String,
    /// Human label, e.g. `"RTL-SDR (native)"`.
    pub name: String,
    pub status: CheckStatus,
    /// What was actually found.
    pub detail: String,
    /// What to do about it, when there is something to do.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `GET /api/doctor` / `sdrmm --doctor`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DoctorReport {
    /// Server version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// `os/arch` of the running build.
    pub platform: String,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Worst status across the checks — the exit-code decision for the CLI.
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
