use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::doctor::DoctorReport;

pub const MAX_LOG_LINES: usize = 500;
pub const MAX_LOG_MESSAGE_LEN: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogLine {
    pub at: String,
    pub level: LogLevel,
    pub target: String,
    pub message: String,
}

/// What the server can say about itself when something went wrong, already redacted: the
/// environment report `--doctor` prints, plus the tail of this run's log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DiagnosticsReport {
    pub generated_at: String,
    pub doctor: DoctorReport,
    pub log: Vec<LogLine>,
    /// Lines the ring dropped before anyone read it, so a truncated tail never reads as a quiet
    /// one.
    pub dropped: u64,
}
