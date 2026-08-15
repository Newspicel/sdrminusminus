use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const ANTENNA_TOOL_ID: &str = "antenna";

pub const MIN_ANTENNA_FREQ_HZ: f64 = 10_000.0;
pub const MAX_ANTENNA_FREQ_HZ: f64 = 300_000_000_000.0;

pub const MIN_VELOCITY_FACTOR: f64 = 0.5;
pub const MAX_VELOCITY_FACTOR: f64 = 1.0;

pub const MIN_FEEDLINE_VELOCITY_FACTOR: f64 = 0.4;
pub const MAX_FEEDLINE_VELOCITY_FACTOR: f64 = 1.0;

pub const MAX_YAGI_DIRECTORS: u8 = 20;
pub const MIN_YAGI_SPACING_WL: f64 = 0.1;
pub const MAX_YAGI_SPACING_WL: f64 = 0.4;
pub const MAX_RADIALS: u8 = 32;
pub const MIN_APEX_ANGLE_DEG: f64 = 60.0;
pub const MAX_APEX_ANGLE_DEG: f64 = 180.0;
pub const MAX_RADIAL_SLOPE_DEG: f64 = 60.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaRequest {
    pub frequency_hz: f64,
    #[serde(default = "default_velocity_factor")]
    pub velocity_factor: f64,
    #[serde(default = "default_feedline_velocity_factor")]
    pub feedline_velocity_factor: f64,
    pub design: AntennaDesign,
}

fn default_velocity_factor() -> f64 {
    0.95
}

fn default_feedline_velocity_factor() -> f64 {
    0.66
}

impl Default for AntennaRequest {
    fn default() -> Self {
        Self {
            frequency_hz: 145_500_000.0,
            velocity_factor: default_velocity_factor(),
            feedline_velocity_factor: default_feedline_velocity_factor(),
            design: AntennaDesign::Dipole,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum AntennaDesign {
    Dipole,
    InvertedV(InvertedVParams),
    GroundPlane(GroundPlaneParams),
    FiveEighthsVertical,
    FoldedDipole,
    JPole,
    Yagi(YagiParams),
    QuadLoop,
    EndFedHalfWave,
}

impl AntennaDesign {
    #[must_use]
    pub fn type_id(&self) -> &'static str {
        match self {
            Self::Dipole => "dipole",
            Self::InvertedV(_) => "inverted_v",
            Self::GroundPlane(_) => "ground_plane",
            Self::FiveEighthsVertical => "five_eighths_vertical",
            Self::FoldedDipole => "folded_dipole",
            Self::JPole => "j_pole",
            Self::Yagi(_) => "yagi",
            Self::QuadLoop => "quad_loop",
            Self::EndFedHalfWave => "end_fed_half_wave",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct InvertedVParams {
    #[serde(default = "default_apex_angle_deg")]
    pub apex_angle_deg: f64,
}

fn default_apex_angle_deg() -> f64 {
    120.0
}

impl Default for InvertedVParams {
    fn default() -> Self {
        Self {
            apex_angle_deg: default_apex_angle_deg(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GroundPlaneParams {
    #[serde(default = "default_radials")]
    pub radials: u8,
    #[serde(default = "default_radial_slope_deg")]
    pub radial_slope_deg: f64,
}

fn default_radials() -> u8 {
    4
}

fn default_radial_slope_deg() -> f64 {
    45.0
}

impl Default for GroundPlaneParams {
    fn default() -> Self {
        Self {
            radials: default_radials(),
            radial_slope_deg: default_radial_slope_deg(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct YagiParams {
    #[serde(default = "default_directors")]
    pub directors: u8,
    #[serde(default = "default_spacing_wavelengths")]
    pub spacing_wavelengths: f64,
}

fn default_directors() -> u8 {
    2
}

fn default_spacing_wavelengths() -> f64 {
    0.2
}

impl Default for YagiParams {
    fn default() -> Self {
        Self {
            directors: default_directors(),
            spacing_wavelengths: default_spacing_wavelengths(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaPart {
    pub name: String,
    pub count: u8,
    pub length_m: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaPoint {
    pub x_m: f64,
    pub y_m: f64,
    pub z_m: f64,
}

impl AntennaPoint {
    #[must_use]
    pub const fn new(x_m: f64, y_m: f64, z_m: f64) -> Self {
        Self { x_m, y_m, z_m }
    }

    pub const ORIGIN: Self = Self::new(0.0, 0.0, 0.0);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AntennaSegmentRole {
    Driven,
    Parasitic,
    Radial,
    Matching,
    Feedline,
    Structure,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaSegment {
    pub label: String,
    pub role: AntennaSegmentRole,
    pub from: AntennaPoint,
    pub to: AntennaPoint,
}

impl AntennaSegment {
    #[must_use]
    pub fn length_m(&self) -> f64 {
        let dx = self.to.x_m - self.from.x_m;
        let dy = self.to.y_m - self.from.y_m;
        let dz = self.to.z_m - self.from.z_m;
        dx.hypot(dy).hypot(dz)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaGeometry {
    pub segments: Vec<AntennaSegment>,
    pub feed: AntennaPoint,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaReport {
    pub design: AntennaDesign,
    pub frequency_hz: f64,
    pub wavelength_m: f64,
    pub velocity_factor: f64,
    pub parts: Vec<AntennaPart>,
    pub geometry: AntennaGeometry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedpoint_ohms: Option<f64>,
    pub balanced: bool,
    pub notes: Vec<String>,
}
