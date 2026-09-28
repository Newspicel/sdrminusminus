use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::array::MAX_ARRAY_LANES;
use crate::processor::df::{MAX_DF_BANDWIDTH_HZ, MAX_DF_OFFSET_HZ, MIN_DF_BANDWIDTH_HZ};
use crate::processor::{finite_within, reserved_at};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Hand {
    Right,
    Left,
    #[default]
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct PolarimeterParams {
    pub h_lane: u32,
    pub v_lane: u32,
    pub offset_hz: f64,
    pub bandwidth_hz: f64,
    pub matched: bool,
    pub report_ms: u32,
    pub average_ms: u32,
    pub crossfade_ms: u32,
    pub flip_hand: bool,
}

impl Default for PolarimeterParams {
    fn default() -> Self {
        Self {
            h_lane: 0,
            v_lane: 1,
            offset_hz: 0.0,
            bandwidth_hz: 20_000.0,
            matched: true,
            report_ms: 250,
            average_ms: 500,
            crossfade_ms: 20,
            flip_hand: false,
        }
    }
}

impl PolarimeterParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (self.h_lane < MAX_ARRAY_LANES, "H out of range"),
            (self.v_lane < MAX_ARRAY_LANES, "V out of range"),
            (self.h_lane != self.v_lane, "Lanes must differ"),
            (
                finite_within(self.offset_hz, MAX_DF_OFFSET_HZ),
                "Offset out of range",
            ),
            (
                (MIN_DF_BANDWIDTH_HZ..=MAX_DF_BANDWIDTH_HZ).contains(&self.bandwidth_hz),
                "Bandwidth out of range",
            ),
            (
                (50..=10_000).contains(&self.report_ms),
                "Report out of range",
            ),
            (
                (50..=10_000).contains(&self.average_ms),
                "Average out of range",
            ),
            (
                (0..=500).contains(&self.crossfade_ms),
                "Crossfade out of range",
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
pub struct PolarimeterReading {
    pub at: String,
    pub i_db: f32,
    pub q: f32,
    pub u: f32,
    pub v: f32,
    pub degree: f32,
    pub angle_deg: f32,
    pub ellipticity_deg: f32,
    #[serde(default)]
    pub hand: Hand,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr_db: Option<f32>,
    #[serde(default)]
    pub out_center_hz: f64,
    #[serde(default)]
    pub out_rate: f64,
}

impl PolarimeterReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            ..Self::default()
        }
    }
}
