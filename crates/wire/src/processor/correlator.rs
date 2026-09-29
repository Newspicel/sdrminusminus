use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::array::MAX_ARRAY_LANES;
use crate::limits::CORRELATOR_LIMITS as LIMITS;
use crate::processor::{band_holds, power_of_two_in, reserved_at};

pub const MAX_BASELINES: usize = (MAX_ARRAY_LANES * (MAX_ARRAY_LANES - 1) / 2) as usize;
pub const MAX_VISIBILITY_CELLS: u32 = 65_535;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct CorrelatorParams {
    pub bins: u32,
    pub integrate_s: f32,
    pub offset_hz: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth_hz: Option<f64>,
    pub overlap: bool,
    pub channels: u32,
}

impl Default for CorrelatorParams {
    fn default() -> Self {
        Self {
            bins: 1024,
            integrate_s: 1.0,
            offset_hz: 0.0,
            bandwidth_hz: None,
            overlap: false,
            channels: 256,
        }
    }
}

impl CorrelatorParams {
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (power_of_two_in(self.bins, LIMITS.bins), "Bins out of range"),
            (
                LIMITS.integrate_s.contains(self.integrate_s),
                "Integrate out of range",
            ),
            (
                LIMITS.band.offset_hz.contains(self.offset_hz),
                "Offset out of range",
            ),
            (
                band_holds(LIMITS.band, self.bandwidth_hz),
                "Band out of range",
            ),
            (
                power_of_two_in(self.channels, LIMITS.channels) && self.channels <= self.bins,
                "Channels out of range",
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
pub struct Baseline {
    pub a: u32,
    pub b: u32,
    pub delay_ns: f32,
    pub coherence: f32,
    #[serde(default)]
    pub phase_deg: f32,
    #[serde(default)]
    pub length_m: f32,
    #[serde(default)]
    pub azimuth_deg: f32,
    #[serde(default)]
    pub snr_db: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CorrelatorReading {
    pub at: String,
    pub integrated_s: f32,
    pub baselines: Vec<Baseline>,
    #[serde(default)]
    pub frames: u64,
}

impl CorrelatorReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            baselines: Vec::with_capacity(MAX_BASELINES),
            ..Self::default()
        }
    }
}
