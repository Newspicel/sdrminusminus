use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::array::MAX_ARRAY_LANES;
use crate::limits::BEAMFORMER_LIMITS as LIMITS;
use crate::processor::df::DF_POINTS;
use crate::processor::{DEFAULT_BAND_HZ, band_holds, reserved_at};

pub const MAX_BEAM_NULLS: usize = 3;
pub const MAX_BEAM_TAPS: u32 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BeamMode {
    #[default]
    Mrc,
    Das,
    Mvdr,
    Lcmv,
    Gsc,
    Canceller,
    Cma,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SteerSource {
    #[default]
    Wired,
    Fixed {
        azimuth_deg: f64,
        #[serde(default)]
        elevation_deg: f64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Adaptation {
    #[default]
    Nlms,
    Rls,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoiseModel {
    #[default]
    Measured,
    White,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct BeamformerParams {
    pub mode: BeamMode,
    pub steer: SteerSource,
    pub nulls_deg: Vec<f64>,
    pub auto_nulls: bool,
    pub main_lane: u32,
    pub reference_lanes: Vec<u32>,
    pub taps: u32,
    pub adaptation: Adaptation,
    pub step: f32,
    pub forget: f32,
    pub crossfade_ms: u32,
    pub update_ms: u32,
    pub carry_over: f32,
    pub loading: f32,
    pub noise: NoiseModel,
    pub offset_hz: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth_hz: Option<f64>,
    pub steer_timeout_ms: u32,
}

impl Default for BeamformerParams {
    fn default() -> Self {
        Self {
            mode: BeamMode::Mrc,
            steer: SteerSource::Wired,
            nulls_deg: Vec::new(),
            auto_nulls: false,
            main_lane: 0,
            reference_lanes: Vec::new(),
            taps: 1,
            adaptation: Adaptation::Nlms,
            step: 0.05,
            forget: 0.999,
            crossfade_ms: 20,
            update_ms: 200,
            carry_over: 0.7,
            loading: 0.1,
            noise: NoiseModel::Measured,
            offset_hz: 0.0,
            bandwidth_hz: Some(DEFAULT_BAND_HZ),
            steer_timeout_ms: 5_000,
        }
    }
}

impl BeamformerParams {
    fn steer_ok(&self) -> bool {
        match self.steer {
            SteerSource::Wired => true,
            SteerSource::Fixed {
                azimuth_deg,
                elevation_deg,
            } => azimuth_deg.is_finite() && LIMITS.elevation_deg.contains(elevation_deg),
        }
    }

    fn references_differ(&self) -> bool {
        self.reference_lanes
            .iter()
            .enumerate()
            .all(|(i, lane)| *lane != self.main_lane && !self.reference_lanes[..i].contains(lane))
    }

    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        let checks = [
            (self.steer_ok(), "Steer out of range"),
            (
                self.nulls_deg.len() <= LIMITS.nulls
                    && self.nulls_deg.iter().all(|deg| deg.is_finite()),
                "Nulls out of range",
            ),
            (self.main_lane < MAX_ARRAY_LANES, "Main out of range"),
            (
                self.reference_lanes
                    .iter()
                    .all(|lane| *lane < MAX_ARRAY_LANES),
                "References out of range",
            ),
            (self.references_differ(), "Lanes must differ"),
            (LIMITS.taps.contains(self.taps), "Taps out of range"),
            (LIMITS.step.contains(self.step), "Step out of range"),
            (LIMITS.forget.contains(self.forget), "Forget out of range"),
            (
                LIMITS.crossfade_ms.contains(self.crossfade_ms),
                "Crossfade out of range",
            ),
            (
                LIMITS.update_ms.contains(self.update_ms),
                "Update out of range",
            ),
            (
                LIMITS.carry_over.contains(self.carry_over),
                "Carry over out of range",
            ),
            (
                LIMITS.loading.contains(self.loading),
                "Loading out of range",
            ),
            (
                LIMITS.band.offset_hz.contains(self.offset_hz),
                "Offset out of range",
            ),
            (
                band_holds(LIMITS.band, self.bandwidth_hz),
                "Bandwidth out of range",
            ),
            (
                LIMITS.steer_timeout_ms.contains(self.steer_timeout_ms),
                "Steer timeout out of range",
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
pub struct LaneWeight {
    pub amplitude_db: f32,
    pub phase_deg: f32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BeamformerReading {
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sinr_gain_db: Option<f32>,
    pub output_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_deg: Option<f32>,
    pub nulls_deg: Vec<f32>,
    pub weights: Vec<LaneWeight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled_db: Option<f32>,
    #[serde(default)]
    pub mode: BeamMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pattern: Vec<u8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub null_depths_db: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_age_ms: Option<u32>,
    #[serde(default)]
    pub loading_used: f32,
    #[serde(default)]
    pub resets: u32,
    #[serde(default)]
    pub out_center_hz: f64,
    #[serde(default)]
    pub out_rate: f64,
    #[serde(default)]
    pub no_steer: bool,
    #[serde(default)]
    pub steer_stale: bool,
    #[serde(default)]
    pub singular: bool,
    #[serde(default)]
    pub diverged: bool,
    #[serde(default)]
    pub band_full: bool,
}

impl BeamformerReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            nulls_deg: Vec::with_capacity(MAX_BEAM_NULLS),
            weights: Vec::with_capacity(MAX_ARRAY_LANES as usize),
            pattern: Vec::with_capacity(DF_POINTS),
            null_depths_db: Vec::with_capacity(MAX_BEAM_NULLS),
            ..Self::default()
        }
    }
}
