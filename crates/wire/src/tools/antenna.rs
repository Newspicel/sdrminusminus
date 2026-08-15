//! Antenna calculator types: what an operator asks for (a design at a frequency) and what the
//! tool answers (the pieces to cut, with the lengths already corrected).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const ANTENNA_TOOL_ID: &str = "antenna";

/// Below this a half-wave element is kilometres long and the request is a typo, not a design.
pub const MIN_ANTENNA_FREQ_HZ: f64 = 10_000.0;
pub const MAX_ANTENNA_FREQ_HZ: f64 = 300_000_000_000.0;

/// The element's end-effect factor: how much shorter a real conductor resonates than free
/// space says. 0.95 is the classic wire figure (the 468/f rule); thick tubing runs lower.
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

/// `POST /api/tools/run` with `"tool": "antenna"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaRequest {
    pub frequency_hz: f64,
    /// End-effect factor applied to every resonant element.
    #[serde(default = "default_velocity_factor")]
    pub velocity_factor: f64,
    /// Velocity factor of the coax, used only by designs that include a matching section.
    /// Solid polyethylene is 0.66, foam 0.80, PTFE 0.70.
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

/// The antenna to cut. Adjacently tagged like [`crate::ChannelParams`]: the designs that take
/// no choices are bare tags, the rest carry their settings.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "settings", rename_all = "snake_case")]
pub enum AntennaDesign {
    /// Half-wave, centre fed, horizontal.
    Dipole,
    /// A dipole with its legs sloped down from a single support.
    InvertedV(InvertedVParams),
    /// Quarter-wave vertical over radials.
    GroundPlane(GroundPlaneParams),
    /// Five-eighths-wave vertical with a base loading coil.
    FiveEighthsVertical,
    /// Half-wave folded dipole, the ~300 Ω driven element of a Yagi.
    FoldedDipole,
    /// End-fed half-wave with a quarter-wave matching stub.
    JPole,
    /// Reflector, driven element and directors on a boom.
    Yagi(YagiParams),
    /// Full-wave square loop (quad), fed at a corner or the middle of a side.
    QuadLoop,
    /// Half-wave wire fed at its high-impedance end through a transformer.
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
    /// Angle between the two legs. 180° is a flat dipole; the legs shorten as it closes.
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
    /// How far the radials droop below horizontal. Sloping them raises the feedpoint
    /// impedance from about 36 Ω towards 50 Ω.
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
    /// Directors in front of the driven element. Zero is a two-element reflector Yagi.
    #[serde(default = "default_directors")]
    pub directors: u8,
    /// Boom spacing between neighbouring elements, in wavelengths.
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

/// One thing to cut, bend or buy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaPart {
    pub name: String,
    /// How many of this part the design needs.
    pub count: u8,
    pub length_m: f64,
    /// Where it sits along the boom, measured from the reflector. Only the designs that have
    /// a boom set it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A point on the antenna, in metres from the origin. `x` runs along the elements, `y` is up,
/// `z` is the boom's depth. The origin is the feedpoint, or the base of anything that stands on
/// one.
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

/// What a drawn piece does, so a view can colour it without reading its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AntennaSegmentRole {
    /// Carries the feedline current: the radiator, the legs, the loop.
    Driven,
    /// Radiates because the driven element makes it: reflectors and directors.
    Parasitic,
    /// The other half of an unbalanced antenna: radials and counterpoises.
    Radial,
    /// Part of the match rather than the radiator: a stub, a coil, a transformer line.
    Matching,
    /// Cable running away from the feedpoint. It has a length worth cutting, but it is not part
    /// of the antenna's own size.
    Feedline,
    /// Holds the rest up and radiates nothing: booms and masts.
    Structure,
}

/// One straight piece of the antenna, as drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaSegment {
    /// The part this piece is. Where the report lists a part under the same name, the segment is
    /// exactly that long.
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

/// The antenna as a shape: enough to draw it to scale from any angle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaGeometry {
    pub segments: Vec<AntennaSegment>,
    /// Where the feedline attaches.
    pub feed: AntennaPoint,
}

/// What the calculator worked out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AntennaReport {
    /// The design that produced this, echoed so a cached report can name itself.
    pub design: AntennaDesign,
    pub frequency_hz: f64,
    /// Free-space wavelength, before any correction factor.
    pub wavelength_m: f64,
    pub velocity_factor: f64,
    pub parts: Vec<AntennaPart>,
    /// The same design as a shape, to scale, for a drawing of it.
    pub geometry: AntennaGeometry,
    /// Estimated feedpoint impedance in free space. `None` where the design's own matching
    /// network sets it and a raw figure would mislead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedpoint_ohms: Option<f64>,
    /// Whether the feedpoint is balanced, and so wants a balun ahead of coax.
    pub balanced: bool,
    /// Everything the numbers alone do not say: what to trim, what to match with, what the
    /// estimate assumes.
    pub notes: Vec<String>,
}
