use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::channel::ChannelParams;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ItuRegion {
    R1,
    R2,
    R3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BandLayerKind {
    World,
    Regulatory,
    Amateur,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BandService {
    Amateur,
    Broadcast,
    Aeronautical,
    Maritime,
    Mobile,
    Satellite,
    Navigation,
    Science,
    Ism,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandLayerInfo {
    pub id: String,
    pub name: String,
    pub authority: String,
    pub source: String,
    pub kind: BandLayerKind,
    pub rank: u8,
    #[serde(default)]
    pub generator: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandAllocation {
    pub id: String,
    pub layer: String,
    pub start_hz: f64,
    pub stop_hz: f64,
    pub service: BandService,
    pub name: String,
    pub official_name: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested: Option<ChannelParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_step_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandBlock {
    pub start_hz: f64,
    pub stop_hz: f64,
    pub of: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covered: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandLane {
    pub id: String,
    pub name: String,
    pub overlay: bool,
    pub blocks: Vec<BandBlock>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegion {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    pub itu_region: ItuRegion,
    pub layers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegionsResponse {
    pub regions: Vec<BandRegion>,
    pub default_region: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandPlan {
    pub region: BandRegion,
    pub layers: Vec<BandLayerInfo>,
    pub allocations: Vec<BandAllocation>,
    pub lanes: Vec<BandLane>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BandRegionMatch {
    pub region: String,
    pub itu_region: ItuRegion,
    pub approximate: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, utoipa::IntoParams)]
pub struct LocateQuery {
    pub lat: f64,
    pub lon: f64,
}
