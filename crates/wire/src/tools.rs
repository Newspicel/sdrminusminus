pub mod antenna;
pub mod nanovna;

pub use antenna::{
    ANTENNA_TOOL_ID, AntennaDesign, AntennaGeometry, AntennaPart, AntennaPoint, AntennaReport,
    AntennaRequest, AntennaSegment, AntennaSegmentRole, GroundPlaneParams, InvertedVParams,
    MAX_ANTENNA_FREQ_HZ, MAX_APEX_ANGLE_DEG, MAX_FEEDLINE_VELOCITY_FACTOR, MAX_RADIAL_SLOPE_DEG,
    MAX_RADIALS, MAX_VELOCITY_FACTOR, MAX_YAGI_DIRECTORS, MAX_YAGI_SPACING_WL, MIN_ANTENNA_FREQ_HZ,
    MIN_APEX_ANGLE_DEG, MIN_FEEDLINE_VELOCITY_FACTOR, MIN_VELOCITY_FACTOR, MIN_YAGI_SPACING_WL,
    YagiParams,
};
pub use nanovna::{
    MAX_NANOVNA_AVERAGES, MAX_NANOVNA_CAL_SLOT, MAX_NANOVNA_FREQ_HZ, MAX_NANOVNA_POINTS,
    MAX_NANOVNA_PORT_LEN, MIN_NANOVNA_FREQ_HZ, MIN_NANOVNA_POINTS, NANOVNA_TOOL_ID, NanoVnaCalStep,
    NanoVnaCalibrateRequest, NanoVnaCalibration, NanoVnaComplex, NanoVnaDevice,
    NanoVnaDeviceReport, NanoVnaMatch, NanoVnaPoint, NanoVnaPortRequest, NanoVnaRequest,
    NanoVnaResult, NanoVnaStandard, NanoVnaSweep, NanoVnaSweepRequest, NanoVnaSweepState,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Calculator,
    Instrument,
    Reference,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolDescriptor {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub category: ToolCategory,
    pub needs_hardware: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolsResponse {
    pub tools: Vec<ToolDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "tool", content = "request", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolRequest {
    Antenna(AntennaRequest),
    #[serde(rename = "nanovna")]
    NanoVna(NanoVnaRequest),
}

impl ToolRequest {
    #[must_use]
    pub fn tool_id(&self) -> &'static str {
        match self {
            Self::Antenna(_) => ANTENNA_TOOL_ID,
            Self::NanoVna(_) => NANOVNA_TOOL_ID,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "tool", content = "result", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolResponse {
    Antenna(AntennaReport),
    #[serde(rename = "nanovna")]
    NanoVna(Box<NanoVnaResult>),
}

impl ToolResponse {
    #[must_use]
    pub fn tool_id(&self) -> &'static str {
        match self {
            Self::Antenna(_) => ANTENNA_TOOL_ID,
            Self::NanoVna(_) => NANOVNA_TOOL_ID,
        }
    }
}
