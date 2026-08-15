//! Tool types. A tool is a self-contained instrument or calculator that stands beside the
//! receiver rather than inside it: it owns no device set, no channel and no DSP graph, and the
//! signal path neither knows nor cares that one exists.
//!
//! Adding a tool is one module here, one module in `sdrmm-tools`, and optionally one React
//! panel. The request and response enums below are the only places the rest of the system
//! learns a tool's name.

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

/// What kind of thing a tool is, so the launcher can group them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    /// Arithmetic over numbers the operator types.
    Calculator,
    /// Drives an instrument — a VNA, a signal generator, a power meter.
    Instrument,
    /// Looks something up.
    Reference,
}

/// One tool the server can run, as advertised by `GET /api/tools`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolDescriptor {
    /// Stable id, and the tag of this tool's [`ToolRequest`] variant.
    pub id: String,
    pub name: String,
    /// One line, shown under the name in the launcher.
    pub summary: String,
    pub category: ToolCategory,
    /// Whether running it needs hardware attached. Feature-gated tools are absent from the
    /// list entirely; this marks the ones that are compiled in but may still find nothing.
    pub needs_hardware: bool,
}

/// `GET /api/tools`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToolsResponse {
    pub tools: Vec<ToolDescriptor>,
}

/// `POST /api/tools/run` — one call to one tool. The tag is the tool id, so the body names its
/// own destination and no path parameter can disagree with it.
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

/// What a tool answered, tagged with the same id the request carried.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "tool", content = "result", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ToolResponse {
    Antenna(AntennaReport),
    // Boxed so one tool's large answer does not set the size of every other tool's: a sweep
    // carries its points and the instrument's whole reported state. Serialises as the bare
    // result, so the wire shape is the same either way.
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
