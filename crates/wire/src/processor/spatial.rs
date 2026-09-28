use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::processor::df::{MAX_DF_BANDWIDTH_HZ, MAX_DF_OFFSET_HZ};
use crate::processor::{MIN_BAND_HZ, finite_within, power_of_two_in, reserved_at};

pub const MAX_SPATIAL_PEAKS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpatialMethod {
    #[default]
    Bartlett,
    Capon,
    Music,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct SpatialSpectrumParams {
    pub bins: u32,
    pub columns: u32,
    pub average_ms: u32,
    pub report_ms: u32,
    pub azimuth_step_deg: f64,
    pub method: SpatialMethod,
    pub span_db: f32,
    pub offset_hz: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth_hz: Option<f64>,
}

impl Default for SpatialSpectrumParams {
    fn default() -> Self {
        Self {
            bins: 1024,
            columns: 256,
            average_ms: 300,
            report_ms: 100,
            azimuth_step_deg: 2.0,
            method: SpatialMethod::Bartlett,
            span_db: 30.0,
            offset_hz: 0.0,
            bandwidth_hz: None,
        }
    }
}

fn divides_circle(step_deg: f64) -> bool {
    let count = 360.0 / step_deg;
    (1.0..=10.0).contains(&step_deg) && (count - count.round()).abs() < 1e-9
}

impl SpatialSpectrumParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (power_of_two_in(self.bins, 256, 4096), "Bins out of range"),
            (
                power_of_two_in(self.columns, 64, 1024) && self.columns <= self.bins,
                "Columns out of range",
            ),
            (
                (50..=10_000).contains(&self.average_ms),
                "Average out of range",
            ),
            (
                (50..=2_000).contains(&self.report_ms),
                "Report out of range",
            ),
            (divides_circle(self.azimuth_step_deg), "Step out of range"),
            ((10.0..=80.0).contains(&self.span_db), "Span out of range"),
            (
                finite_within(self.offset_hz, MAX_DF_OFFSET_HZ),
                "Offset out of range",
            ),
            (
                self.bandwidth_hz
                    .is_none_or(|hz| (MIN_BAND_HZ..=MAX_DF_BANDWIDTH_HZ).contains(&hz)),
                "Bandwidth out of range",
            ),
        ];
        checks.iter().find(|(ok, _)| !ok).map(|(_, text)| *text)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SpatialPeak {
    pub freq_hz: f64,
    pub bearing_deg: f32,
    pub db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub true_deg: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SpatialReading {
    pub at: String,
    pub peaks: Vec<SpatialPeak>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azimuth_deg: Option<f64>,
    #[serde(default)]
    pub frames: u64,
    #[serde(default)]
    pub dropped_frames: u64,
}

impl SpatialReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            peaks: Vec::with_capacity(MAX_SPATIAL_PEAKS),
            ..Self::default()
        }
    }
}
