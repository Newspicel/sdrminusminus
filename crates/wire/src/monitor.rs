use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{EventAudio, IdentSignal};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SpectrumMonitorNode {
    #[serde(default = "enabled")]
    pub record_audio: bool,
}

const fn enabled() -> bool {
    true
}

impl Default for SpectrumMonitorNode {
    fn default() -> Self {
        Self { record_audio: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventOrigin {
    pub node: String,
    pub transmission: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransmissionState {
    Started,
    Completed,
    Continued,
    Interrupted,
    Problem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Transmission {
    pub id: u64,
    pub state: TransmissionState,
    pub signal: IdentSignal,
    pub start_sample: u64,
    pub end_sample: u64,
    pub sample_rate_hz: f64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<String>,
    #[serde(default)]
    pub decoder_confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<EventAudio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Transmission {
    pub fn summary(&self) -> String {
        let state = match self.state {
            TransmissionState::Started => "started",
            TransmissionState::Completed => "completed",
            TransmissionState::Continued => "continued",
            TransmissionState::Interrupted => "interrupted",
            TransmissionState::Problem => "problem",
        };
        let decoder = self
            .decoder
            .as_deref()
            .unwrap_or(self.signal.modulation.label());
        let mut summary = format!("{state} · {decoder}");
        if let Some(error) = &self.error {
            summary.push_str(" · ");
            summary.push_str(error);
        }
        summary
    }
}
