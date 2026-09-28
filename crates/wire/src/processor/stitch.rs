use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::array::MAX_ARRAY_LANES;
use crate::processor::reserved_at;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StitchBlend {
    #[default]
    Snr,
    Equal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct StitchParams {
    pub blend: StitchBlend,
    pub noise_equalise: bool,
    pub flatten: bool,
    pub spur_reject: bool,
    pub match_phase: bool,
}

impl Default for StitchParams {
    fn default() -> Self {
        Self {
            blend: StitchBlend::Snr,
            noise_equalise: true,
            flatten: true,
            spur_reject: true,
            match_phase: true,
        }
    }
}

impl StitchParams {
    #[must_use]
    pub const fn problem(&self) -> Option<&'static str> {
        None
    }

    #[must_use]
    pub const fn valid(&self) -> bool {
        self.problem().is_none()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StitchLane {
    pub lane: u32,
    pub center_hz: f64,
    pub noise_eq_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coherence: Option<f32>,
    #[serde(default)]
    pub phase_deg: f32,
    #[serde(default)]
    pub spur_bins: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StitchReading {
    pub at: String,
    pub center_hz: f64,
    pub span_hz: f64,
    pub lanes: Vec<StitchLane>,
    #[serde(default)]
    pub dropped_blocks: u64,
    #[serde(default)]
    pub no_overlap: bool,
}

impl StitchReading {
    #[must_use]
    pub fn reserved() -> Self {
        Self {
            at: reserved_at(),
            lanes: Vec::with_capacity(MAX_ARRAY_LANES as usize),
            ..Self::default()
        }
    }
}
