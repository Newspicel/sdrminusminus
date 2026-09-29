use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::array::MAX_ARRAY_LANES;
use crate::geo::LatLon;
use crate::limits::DF_LIMITS as LIMITS;
use crate::processor::reserved_at;

pub const DF_POINTS: usize = 360;
pub const MAX_DF_PEAKS: u8 = 4;
pub const MAX_DF_SOURCES: u32 = 15;
pub const MIN_DF_REPORT_MS: u32 = 100;
pub const MAX_DF_REPORT_MS: u32 = 10_000;
pub const MIN_DF_BANDWIDTH_HZ: f64 = 100.0;
pub const MAX_DF_BANDWIDTH_HZ: f64 = 20_000_000.0;
pub const MAX_DF_OFFSET_HZ: f64 = 100_000_000.0;
pub const MAX_STATION_ID_LEN: usize = 64;
pub const MAX_DF_SMOOTHING: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DfAlgorithm {
    Bartlett,
    Capon,
    #[default]
    Music,
    RootMusic,
    Esprit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceRule {
    #[default]
    Dominance,
    Mdl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UlaSide {
    #[default]
    Both,
    Front,
    Back,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct DfParams {
    pub algorithm: DfAlgorithm,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<u32>,
    pub source_rule: SourceRule,
    pub max_peaks: u8,
    pub smoothing: u8,
    pub forward_backward: bool,
    pub elevation: bool,
    pub ula_side: UlaSide,
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    pub report_ms: u32,
    pub carry_over: f32,
    pub squelch_db: f32,
    pub loading: f32,
    pub azimuth_step_deg: f64,
    pub yaw_gate_dps: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub station_id: Option<String>,
}

impl Default for DfParams {
    fn default() -> Self {
        Self {
            algorithm: DfAlgorithm::Music,
            sources: None,
            source_rule: SourceRule::Dominance,
            max_peaks: 2,
            smoothing: 0,
            forward_backward: false,
            elevation: false,
            ula_side: UlaSide::Both,
            offset_hz: 0.0,
            bandwidth_hz: 20_000.0,
            report_ms: 500,
            carry_over: 0.5,
            squelch_db: 6.0,
            loading: 0.001,
            azimuth_step_deg: 1.0,
            yaw_gate_dps: 20.0,
            station_id: None,
        }
    }
}

impl DfParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (
                self.sources.is_none_or(|n| LIMITS.sources.contains(n)),
                "Sources out of range",
            ),
            (LIMITS.peaks.contains(self.max_peaks), "Peaks out of range"),
            (
                LIMITS.smoothing.contains(self.smoothing),
                "Smoothing out of range",
            ),
            (
                LIMITS.band.offset_hz.contains(self.offset_hz),
                "Offset out of range",
            ),
            (
                LIMITS.band.bandwidth_hz.contains(self.bandwidth_hz),
                "Bandwidth out of range",
            ),
            (
                LIMITS.report_ms.contains(self.report_ms),
                "Report out of range",
            ),
            (
                LIMITS.carry_over.contains(self.carry_over),
                "Carry over out of range",
            ),
            (
                LIMITS.squelch_db.contains(self.squelch_db),
                "Squelch out of range",
            ),
            (
                LIMITS.loading.contains(self.loading),
                "Loading out of range",
            ),
            (
                LIMITS.azimuth_step_deg.contains(self.azimuth_step_deg),
                "Step out of range",
            ),
            (
                LIMITS.yaw_gate_dps.contains(self.yaw_gate_dps),
                "Yaw gate out of range",
            ),
            (
                self.station_id
                    .as_deref()
                    .is_none_or(|id| !id.is_empty() && id.len() <= LIMITS.station_len),
                "Station out of range",
            ),
        ];
        checks.iter().find(|(ok, _)| !ok).map(|(_, text)| *text)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfPeak {
    pub relative_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_deg: Option<f32>,
    pub power_db: f32,
    pub confidence: f32,
    pub sigma_deg: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror_true_deg: Option<f32>,
    #[serde(default)]
    pub fit: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfReading {
    pub at: String,
    pub peaks: Vec<DfPeak>,
    pub pseudospectrum: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<LatLon>,
    pub sources: u32,
    pub sources_auto: bool,
    pub squelched: bool,
    pub aliasing: bool,
    #[serde(default)]
    pub algorithm: DfAlgorithm,
    #[serde(default)]
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_sigma_deg: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub likelihood: Vec<u8>,
    #[serde(default)]
    pub likelihood_true: bool,
    #[serde(default)]
    pub span_db: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub eigenvalues_db: Vec<f32>,
    #[serde(default)]
    pub eig_ratio_db: f32,
    #[serde(default)]
    pub lambda12_db: f32,
    #[serde(default)]
    pub snr_db: f32,
    #[serde(default)]
    pub snapshots: f32,
    #[serde(default)]
    pub fit: f32,
    #[serde(default)]
    pub spacing_ratio: f32,
    #[serde(default)]
    pub aperture_wavelengths: f32,
    #[serde(default)]
    pub mode_aliasing: bool,
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub rotating: bool,
    #[serde(default)]
    pub singular: bool,
    #[serde(default)]
    pub table_out_of_range: bool,
    #[serde(default)]
    pub gated_blocks: u32,
}

impl DfReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            peaks: Vec::with_capacity(usize::from(MAX_DF_PEAKS)),
            pseudospectrum: Vec::with_capacity(DF_POINTS),
            likelihood: Vec::with_capacity(DF_POINTS),
            eigenvalues_db: Vec::with_capacity(MAX_ARRAY_LANES as usize),
            ..Self::default()
        }
    }
}
