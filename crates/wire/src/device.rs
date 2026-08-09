//! Device capability model and settings. `Capabilities` is the backbone of the
//! backend-driven UI (PLAN §6): the client auto-renders controls from it.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A discovered receiver, produced by a driver's probe (PLAN §6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceInfo {
    /// Driver id that produced this entry: `"virtual"`, `"soapy"`, `"rtlsdr"`, …
    pub driver: String,
    /// Stable per-device key within a driver (serial, index, or file path).
    pub key: String,
    /// Human label for the device picker.
    pub label: String,
    /// Serial number when the driver exposes one (used to collapse probe duplicates, PLAN §6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

impl DeviceInfo {
    /// The `driver:key` handle used by `POST /api/devicesets` (PLAN §5).
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.driver, self.key)
    }
}

/// An inclusive numeric range with an optional step, in the setting's native unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

/// A named gain stage with its range in dB (e.g. RTL-SDR tuner gain, HackRF LNA/VGA).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainStage {
    pub name: String,
    pub range: Range,
}

/// A typed device-specific setting the client renders generically when it has no
/// first-class UI (PLAN §6: "typed extra settings").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtraSetting {
    Bool {
        name: String,
        default: bool,
    },
    Range {
        name: String,
        range: Range,
        unit: String,
    },
    Enum {
        name: String,
        options: Vec<String>,
        default: String,
    },
}

/// Everything the client needs to render device controls without hand-written DTOs (PLAN §6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Capabilities {
    /// Tunable center-frequency ranges in Hz (multiple = discontiguous tuner ranges).
    pub freq_ranges: Vec<Range>,
    /// Supported sample rates in samples/s. Empty means "continuous", use `sample_rate_range`.
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_range: Option<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub bandwidths: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraSetting>,
    /// True once TX is implemented; declared from day one, unused in the RX phases (PLAN §1).
    #[serde(default)]
    pub tx_capable: bool,
}

/// A gain-stage value in dB, keyed by the stage name from [`GainStage`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainValue {
    pub stage: String,
    pub value_db: f64,
}

/// A value for one [`ExtraSetting`], keyed by its name. `value` is bool/number/string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExtraValue {
    pub name: String,
    pub value: serde_json::Value,
}

/// A mutation applied to a device. Absent fields are left unchanged (PLAN §5 PATCH device).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
    /// Hardware baseband filter bandwidth in Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraValue>,
}

impl DeviceSettings {
    /// Overlay the present fields of `delta` onto `self` (PLAN §5 PATCH: absent fields are
    /// unchanged). Gains and extras merge per name, so a delta carrying one stage patches only
    /// that stage — required for the capability UI, which sends one control's value at a time.
    /// The single merge implementation: engine and device backends both use this so applied
    /// settings and reported state can never disagree on merge semantics.
    pub fn merge_from(&mut self, delta: &DeviceSettings) {
        if delta.center_hz.is_some() {
            self.center_hz = delta.center_hz;
        }
        if delta.sample_rate.is_some() {
            self.sample_rate = delta.sample_rate;
        }
        if delta.ppm.is_some() {
            self.ppm = delta.ppm;
        }
        if delta.antenna.is_some() {
            self.antenna.clone_from(&delta.antenna);
        }
        if delta.bandwidth.is_some() {
            self.bandwidth = delta.bandwidth;
        }
        for gain in &delta.gains {
            match self.gains.iter_mut().find(|g| g.stage == gain.stage) {
                Some(existing) => existing.value_db = gain.value_db,
                None => self.gains.push(gain.clone()),
            }
        }
        for extra in &delta.extra {
            match self.extra.iter_mut().find(|e| e.name == extra.name) {
                Some(existing) => existing.value.clone_from(&extra.value),
                None => self.extra.push(extra.clone()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gain(stage: &str, value_db: f64) -> GainValue {
        GainValue {
            stage: stage.to_string(),
            value_db,
        }
    }

    fn extra(name: &str, value: serde_json::Value) -> ExtraValue {
        ExtraValue {
            name: name.to_string(),
            value,
        }
    }

    #[test]
    fn merge_patches_one_gain_stage_and_appends_new_ones() {
        let mut settings = DeviceSettings {
            gains: vec![gain("LNA", 16.0), gain("VGA", 20.0)],
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            gains: vec![gain("VGA", 30.0), gain("AMP", 14.0)],
            ..DeviceSettings::default()
        });
        assert_eq!(
            settings.gains,
            vec![gain("LNA", 16.0), gain("VGA", 30.0), gain("AMP", 14.0)]
        );
    }

    #[test]
    fn merge_patches_extra_by_name() {
        let mut settings = DeviceSettings {
            extra: vec![extra("bias_t", false.into()), extra("agc", true.into())],
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            extra: vec![
                extra("bias_t", true.into()),
                extra("offset_tuning", true.into()),
            ],
            ..DeviceSettings::default()
        });
        assert_eq!(
            settings.extra,
            vec![
                extra("bias_t", true.into()),
                extra("agc", true.into()),
                extra("offset_tuning", true.into()),
            ]
        );
    }

    #[test]
    fn merge_overlays_bandwidth_and_leaves_absent_fields() {
        let mut settings = DeviceSettings {
            center_hz: Some(100_000_000.0),
            bandwidth: Some(2_500_000.0),
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            bandwidth: Some(1_750_000.0),
            ..DeviceSettings::default()
        });
        assert_eq!(settings.center_hz, Some(100_000_000.0));
        assert_eq!(settings.bandwidth, Some(1_750_000.0));

        settings.merge_from(&DeviceSettings::default());
        assert_eq!(settings.bandwidth, Some(1_750_000.0));
    }
}
