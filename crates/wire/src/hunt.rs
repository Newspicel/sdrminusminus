use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HuntSettings {
    /// The decoder being hunted. Its frequency and bandwidth are what the readings measure, so
    /// retuning it retunes the hunt.
    pub channel: u32,
    /// How often a reading is published. A hunt is walked with, so the feedback has to keep up
    /// with the steps rather than with a status panel.
    #[serde(default = "default_interval_ms")]
    pub interval_ms: u32,
}

const fn default_interval_ms() -> u32 {
    50
}

impl HuntSettings {
    #[must_use]
    pub const fn for_channel(channel: u32) -> Self {
        Self {
            channel,
            interval_ms: default_interval_ms(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntStatus {
    pub settings: HuntSettings,
    /// The frequency the readings are taken on: the decoder's, as of the last reading.
    #[serde(default)]
    pub freq_hz: f64,
    #[serde(default)]
    pub bw_hz: f64,
    /// The strongest reading in the last interval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level_db: Option<f32>,
    /// The reading with the jitter taken out, which is what a walking operator can act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smooth_db: Option<f32>,
    /// The quietest and loudest this hunt has seen, so a meter can scale itself to the ground
    /// actually covered instead of to a guess about how loud the transmitter is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_db: Option<f32>,
    /// Where the current reading sits between `floor_db` and `best_db`, from 0 to 1.
    #[serde(default)]
    pub strength: f32,
    /// Whether the last few readings are climbing: the answer to "warmer or colder".
    #[serde(default)]
    pub closing: bool,
    pub readings: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HuntAction {
    Start,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HuntRequest {
    pub action: HuntAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<HuntSettings>,
}
