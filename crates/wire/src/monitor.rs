use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{EventAudio, IdentSignal};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SpectrumMonitorNode {
    #[serde(default = "enabled")]
    pub record_audio: bool,
    #[serde(default = "default_min_confidence")]
    #[schema(minimum = 0, maximum = 1)]
    pub min_confidence: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_protocols: Vec<String>,
    #[serde(default = "enabled")]
    pub report_unidentified: bool,
}

const fn default_min_confidence() -> f32 {
    0.7
}

const fn enabled() -> bool {
    true
}

impl Default for SpectrumMonitorNode {
    fn default() -> Self {
        Self {
            record_audio: true,
            min_confidence: default_min_confidence(),
            disabled_protocols: Vec::new(),
            report_unidentified: true,
        }
    }
}

impl SpectrumMonitorNode {
    pub fn valid(&self) -> bool {
        (0.0..=1.0).contains(&self.min_confidence)
    }

    #[must_use]
    pub fn decodes(&self, kind: &str) -> bool {
        !self
            .disabled_protocols
            .iter()
            .any(|disabled| disabled == kind)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_settings_receive_the_confidence_default() {
        let settings: SpectrumMonitorNode =
            serde_json::from_str(r#"{"record_audio":false}"#).unwrap();
        assert!(!settings.record_audio);
        assert_eq!(settings.min_confidence, 0.7);
        assert!(settings.disabled_protocols.is_empty());
        assert!(settings.report_unidentified);
    }

    #[test]
    fn every_protocol_decodes_until_disabled() {
        let mut settings = SpectrumMonitorNode::default();
        assert!(settings.decodes("dmr"));
        settings.disabled_protocols.push("dmr".to_owned());
        assert!(!settings.decodes("dmr"));
        assert!(settings.decodes("nfm"));
    }

    #[test]
    fn confidence_must_be_finite_and_within_zero_and_one() {
        for min_confidence in [-0.1, 1.1, f32::NAN, f32::INFINITY] {
            assert!(
                !SpectrumMonitorNode {
                    min_confidence,
                    ..Default::default()
                }
                .valid()
            );
        }
        for min_confidence in [0.0, 0.7, 1.0] {
            assert!(
                SpectrumMonitorNode {
                    min_confidence,
                    ..Default::default()
                }
                .valid()
            );
        }
    }
}
