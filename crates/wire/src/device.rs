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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraValue>,
}
