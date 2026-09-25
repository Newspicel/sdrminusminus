use sdrmm_wire::tools::{
    AntennaDesign, AntennaGeometry, AntennaPoint, AntennaReport, AntennaRequest,
    AntennaSegmentRole, GroundPlaneParams, InvertedVParams, YagiParams,
};
use sdrmm_wire::tools::{ToolRequest, ToolResponse};

use crate::ui::tools::kit::Paint;

pub const DESIGNS: [(AntennaDesign, &str); 9] = [
    (AntennaDesign::Dipole, "Dipole (half wave)"),
    (
        AntennaDesign::InvertedV(InvertedVParams {
            apex_angle_deg: 120.0,
        }),
        "Inverted V",
    ),
    (
        AntennaDesign::GroundPlane(GroundPlaneParams {
            radials: 4,
            radial_slope_deg: 45.0,
        }),
        "Ground plane (quarter wave)",
    ),
    (AntennaDesign::FiveEighthsVertical, "5/8 wave vertical"),
    (AntennaDesign::FoldedDipole, "Folded dipole"),
    (AntennaDesign::JPole, "J-pole"),
    (
        AntennaDesign::Yagi(YagiParams {
            directors: 2,
            spacing_wavelengths: 0.2,
        }),
        "Yagi",
    ),
    (AntennaDesign::QuadLoop, "Quad loop (full wave)"),
    (AntennaDesign::EndFedHalfWave, "End-fed half wave"),
];

#[must_use]
pub fn default_design(type_id: &str) -> AntennaDesign {
    DESIGNS
        .iter()
        .find(|(design, _)| design.type_id() == type_id)
        .map_or(AntennaDesign::Dipole, |(design, _)| *design)
}

#[must_use]
pub fn uses_feedline(design: &AntennaDesign) -> bool {
    matches!(design, AntennaDesign::QuadLoop)
}

#[must_use]
pub fn antenna_request(request: AntennaRequest) -> ToolRequest {
    ToolRequest::Antenna(request)
}

#[must_use]
pub fn antenna_report(response: Option<&ToolResponse>) -> Option<&AntennaReport> {
    match response? {
        ToolResponse::Antenna(report) => Some(report),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Plan,
    Orbit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Metres,
    Feet,
}

const INCHES_PER_M: f64 = 39.370_078_7;
const METERS_PER_FOOT: f64 = 0.3048;

#[must_use]
pub fn format_length(meters: f64, unit: Unit) -> String {
    if !meters.is_finite() {
        return "-".to_owned();
    }
    match unit {
        Unit::Feet => {
            let inches = (meters * INCHES_PER_M * 10.0).round() / 10.0;
            let feet = (inches / 12.0).floor();
            if feet > 0.0 {
                format!("{feet:.0} ft {:.1} in", inches - feet * 12.0)
            } else {
                format!("{inches:.1} in")
            }
        }
        Unit::Metres if meters < 1.0 => format!("{:.1} cm", meters * 100.0),
        Unit::Metres => format!("{meters:.3} m"),
    }
}

#[must_use]
pub fn format_impedance(ohms: Option<f64>) -> String {
    ohms.map_or_else(
        || "set by its own matching network".to_owned(),
        |ohms| format!("\u{2248} {} \u{3a9}", ohms.round()),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub min: f64,
    pub max: f64,
    pub size: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub x: Extent,
    pub y: Extent,
    pub z: Extent,
}

impl Bounds {
    #[must_use]
    pub fn along(&self, axis: Axis) -> Extent {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
            Axis::Z => self.z,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Angles {
    pub yaw: f64,
    pub pitch: f64,
}

pub const ISOMETRIC: Angles = Angles {
    yaw: -32.0,
    pitch: 22.0,
};

pub const MAX_PITCH: f64 = 88.0;
const DEGREES_PER_PIXEL: f64 = 0.4;
const GRID_DIVISIONS: usize = 4;
const GRID_DROP: f64 = 0.06;

#[must_use]
pub fn bounds_of(geometry: &AntennaGeometry) -> Bounds {
    let mut points = vec![geometry.feed];
    points.extend(
        geometry
            .segments
            .iter()
            .flat_map(|segment| [segment.from, segment.to]),
    );
    bounds_of_points(&points)
}

#[must_use]
pub fn structure_bounds(geometry: &AntennaGeometry) -> Bounds {
    let points: Vec<AntennaPoint> = geometry
        .segments
        .iter()
        .filter(|segment| segment.role != AntennaSegmentRole::Feedline)
        .flat_map(|segment| [segment.from, segment.to])
        .collect();
    bounds_of_points(&points)
}

fn bounds_of_points(points: &[AntennaPoint]) -> Bounds {
    Bounds {
        x: extent_of(points.iter().map(|point| point.x_m)),
        y: extent_of(points.iter().map(|point| point.y_m)),
        z: extent_of(points.iter().map(|point| point.z_m)),
    }
}

fn extent_of(values: impl Iterator<Item = f64>) -> Extent {
    let (min, max) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
        (low.min(value), high.max(value))
    });
    if min > max {
        return Extent {
            min: 0.0,
            max: 0.0,
            size: 0.0,
        };
    }
    Extent {
        min,
        max,
        size: max - min,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanView {
    pub label: &'static str,
    pub angles: Angles,
    pub horizontal: Axis,
    pub vertical: Axis,
}

const FRONT: PlanView = PlanView {
    label: "Front view",
    angles: Angles {
        yaw: 0.0,
        pitch: 0.0,
    },
    horizontal: Axis::X,
    vertical: Axis::Y,
};

const TOP: PlanView = PlanView {
    label: "Top view",
    angles: Angles {
        yaw: 0.0,
        pitch: -90.0,
    },
    horizontal: Axis::X,
    vertical: Axis::Z,
};

const SIDE: PlanView = PlanView {
    label: "Side view",
    angles: Angles {
        yaw: 90.0,
        pitch: 0.0,
    },
    horizontal: Axis::Z,
    vertical: Axis::Y,
};

#[must_use]
pub fn plan_view(bounds: &Bounds) -> PlanView {
    if bounds.z.size <= bounds.x.size && bounds.z.size <= bounds.y.size {
        return FRONT;
    }
    if bounds.y.size <= bounds.x.size {
        TOP
    } else {
        SIDE
    }
}

#[must_use]
pub fn project(point: &AntennaPoint, angles: Angles) -> Point2 {
    let yaw = angles.yaw.to_radians();
    let pitch = angles.pitch.to_radians();
    let across = point.x_m * yaw.cos() + point.z_m * yaw.sin();
    let depth = point.z_m * yaw.cos() - point.x_m * yaw.sin();
    let up = point.y_m * pitch.cos() - depth * pitch.sin();
    Point2 { x: across, y: -up }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub width: f64,
    pub height: f64,
    pub padding: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

#[must_use]
pub fn fit_to(points: &[Point2], viewport: Viewport) -> Fit {
    let centred = Fit {
        scale: 1.0,
        offset_x: viewport.width / 2.0,
        offset_y: viewport.height / 2.0,
    };
    if points.is_empty() {
        return centred;
    }
    let xs = extent_of(points.iter().map(|point| point.x));
    let ys = extent_of(points.iter().map(|point| point.y));
    let room = (viewport.width - 2.0 * viewport.padding).max(1.0);
    let height = (viewport.height - 2.0 * viewport.padding).max(1.0);
    let across = if xs.size > 0.0 {
        room / xs.size
    } else {
        f64::INFINITY
    };
    let down = if ys.size > 0.0 {
        height / ys.size
    } else {
        f64::INFINITY
    };
    let scale = across.min(down);
    if !scale.is_finite() || scale <= 0.0 {
        return centred;
    }
    Fit {
        scale,
        offset_x: viewport.width / 2.0 - (xs.max + xs.min) / 2.0 * scale,
        offset_y: viewport.height / 2.0 - (ys.max + ys.min) / 2.0 * scale,
    }
}

#[must_use]
pub fn place(point: Point2, fit: Fit) -> Point2 {
    Point2 {
        x: point.x * fit.scale + fit.offset_x,
        y: point.y * fit.scale + fit.offset_y,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleBar {
    pub meters: f64,
    pub pixels: f64,
    pub label: String,
}

#[must_use]
pub fn scale_bar(pixels_per_meter: f64, max_pixels: f64, unit: Unit) -> ScaleBar {
    let room = max_pixels / pixels_per_meter;
    if !room.is_finite() || room <= 0.0 {
        return ScaleBar {
            meters: 0.0,
            pixels: 0.0,
            label: "-".to_owned(),
        };
    }
    let meters = match unit {
        Unit::Feet => round_down(room / METERS_PER_FOOT) * METERS_PER_FOOT,
        Unit::Metres => round_down(room),
    };
    ScaleBar {
        meters,
        pixels: meters * pixels_per_meter,
        label: format_length(meters, unit),
    }
}

fn round_down(value: f64) -> f64 {
    let decade = 10f64.powf(value.log10().floor());
    let step = [5.0, 2.0, 1.0]
        .into_iter()
        .find(|step| value >= step * decade)
        .unwrap_or(1.0);
    step * decade
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoleStyle {
    pub ink: Paint,
    pub width: f64,
    pub dash: Option<[f64; 2]>,
}

#[must_use]
pub fn role_style(role: AntennaSegmentRole) -> RoleStyle {
    let (ink, width, dash) = match role {
        AntennaSegmentRole::Driven => (Paint::Accent, 3.0, None),
        AntennaSegmentRole::Parasitic => (Paint::Ink, 2.5, None),
        AntennaSegmentRole::Radial => (Paint::InkDim, 1.5, None),
        AntennaSegmentRole::Matching => (Paint::Ok, 2.0, Some([5.0, 3.0])),
        AntennaSegmentRole::Structure => (Paint::LineStrong, 4.0, None),
        AntennaSegmentRole::Feedline => (Paint::InkFaint, 2.0, Some([2.0, 4.0])),
    };
    RoleStyle { ink, width, dash }
}

#[must_use]
pub fn role_label(role: AntennaSegmentRole) -> &'static str {
    match role {
        AntennaSegmentRole::Driven => "Driven",
        AntennaSegmentRole::Parasitic => "Parasitic",
        AntennaSegmentRole::Radial => "Radial",
        AntennaSegmentRole::Matching => "Matching",
        AntennaSegmentRole::Structure => "Structure",
        AntennaSegmentRole::Feedline => "Feedline",
    }
}

const ROLE_ORDER: [AntennaSegmentRole; 6] = [
    AntennaSegmentRole::Driven,
    AntennaSegmentRole::Parasitic,
    AntennaSegmentRole::Radial,
    AntennaSegmentRole::Matching,
    AntennaSegmentRole::Structure,
    AntennaSegmentRole::Feedline,
];

#[must_use]
pub fn roles_in(geometry: &AntennaGeometry) -> Vec<AntennaSegmentRole> {
    ROLE_ORDER
        .into_iter()
        .filter(|role| {
            geometry
                .segments
                .iter()
                .any(|segment| segment.role == *role)
        })
        .collect()
}

#[must_use]
pub fn ground_grid(bounds: &Bounds) -> Vec<(AntennaPoint, AntennaPoint)> {
    let side = bounds.x.size.max(bounds.z.size) * 1.2;
    if side <= 0.0 {
        return Vec::new();
    }
    let centre_x = (bounds.x.min + bounds.x.max) / 2.0;
    let centre_z = (bounds.z.min + bounds.z.max) / 2.0;
    let y = bounds.y.min - side * GRID_DROP;
    let step = side / GRID_DIVISIONS as f64;
    let half = side / 2.0;
    (0..=GRID_DIVISIONS)
        .flat_map(|index| {
            let offset = -half + index as f64 * step;
            [
                (
                    AntennaPoint::new(centre_x + offset, y, centre_z - half),
                    AntennaPoint::new(centre_x + offset, y, centre_z + half),
                ),
                (
                    AntennaPoint::new(centre_x - half, y, centre_z + offset),
                    AntennaPoint::new(centre_x + half, y, centre_z + offset),
                ),
            ]
        })
        .collect()
}

#[must_use]
pub fn turn(angles: Angles, dx: f64, dy: f64) -> Angles {
    Angles {
        yaw: angles.yaw + dx * DEGREES_PER_PIXEL,
        pitch: (angles.pitch + dy * DEGREES_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::tools::AntennaSegment;

    use super::*;

    fn close(left: f64, right: f64, digits: i32) -> bool {
        (left - right).abs() < 10f64.powi(-digits) / 2.0
    }

    fn segment(
        label: &str,
        role: AntennaSegmentRole,
        from: [f64; 3],
        to: [f64; 3],
    ) -> AntennaSegment {
        AntennaSegment {
            label: label.to_owned(),
            role,
            from: AntennaPoint::new(from[0], from[1], from[2]),
            to: AntennaPoint::new(to[0], to[1], to[2]),
        }
    }

    fn dipole() -> AntennaGeometry {
        AntennaGeometry {
            feed: AntennaPoint::ORIGIN,
            segments: vec![
                segment(
                    "Leg",
                    AntennaSegmentRole::Driven,
                    [-1.0, 0.0, 0.0],
                    [0.0, 0.0, 0.0],
                ),
                segment(
                    "Leg",
                    AntennaSegmentRole::Driven,
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                ),
            ],
        }
    }

    fn yagi() -> AntennaGeometry {
        AntennaGeometry {
            feed: AntennaPoint::new(0.0, 0.0, 0.4),
            segments: vec![
                segment(
                    "Boom",
                    AntennaSegmentRole::Structure,
                    [0.0, 0.0, 0.0],
                    [0.0, 0.0, 1.2],
                ),
                segment(
                    "Reflector",
                    AntennaSegmentRole::Parasitic,
                    [-0.53, 0.0, 0.0],
                    [0.53, 0.0, 0.0],
                ),
                segment(
                    "Driven element",
                    AntennaSegmentRole::Driven,
                    [-0.5, 0.0, 0.4],
                    [0.5, 0.0, 0.4],
                ),
            ],
        }
    }

    fn vertical() -> AntennaGeometry {
        AntennaGeometry {
            feed: AntennaPoint::ORIGIN,
            segments: vec![
                segment(
                    "Radiator",
                    AntennaSegmentRole::Driven,
                    [0.0, 0.0, 0.0],
                    [0.0, 1.2, 0.0],
                ),
                segment(
                    "Radial",
                    AntennaSegmentRole::Radial,
                    [0.0, 0.0, 0.0],
                    [0.5, -0.5, 0.0],
                ),
                segment(
                    "Radial",
                    AntennaSegmentRole::Radial,
                    [0.0, 0.0, 0.0],
                    [-0.5, -0.5, 0.0],
                ),
            ],
        }
    }

    #[test]
    fn every_design_has_a_default_and_a_label() {
        for (design, label) in DESIGNS {
            assert_eq!(default_design(design.type_id()).type_id(), design.type_id());
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn designs_that_take_choices_start_at_usable_numbers() {
        assert_eq!(
            default_design("inverted_v"),
            AntennaDesign::InvertedV(InvertedVParams {
                apex_angle_deg: 120.0
            })
        );
        assert_eq!(
            default_design("yagi"),
            AntennaDesign::Yagi(YagiParams {
                directors: 2,
                spacing_wavelengths: 0.2
            })
        );
        assert_eq!(
            default_design("ground_plane"),
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 4,
                radial_slope_deg: 45.0
            })
        );
    }

    #[test]
    fn a_design_with_no_choices_is_sent_as_a_bare_tag() {
        let json = serde_json::to_value(default_design("dipole")).unwrap();
        assert_eq!(json, serde_json::json!({ "type": "dipole" }));
    }

    #[test]
    fn only_the_quad_asks_for_the_coax_factor() {
        assert!(uses_feedline(&default_design("quad_loop")));
        for id in ["dipole", "yagi", "j_pole", "end_fed_half_wave"] {
            assert!(!uses_feedline(&default_design(id)));
        }
    }

    #[test]
    fn the_request_is_tagged_with_the_tool_that_answers_it() {
        let request = antenna_request(AntennaRequest {
            frequency_hz: 145_500_000.0,
            velocity_factor: 0.95,
            feedline_velocity_factor: 0.66,
            design: default_design("dipole"),
        });
        let json = serde_json::to_value(request).unwrap();
        assert_eq!(json["tool"], "antenna");
        assert_eq!(json["request"]["frequency_hz"], 145_500_000.0);
    }

    #[test]
    fn the_report_is_unwrapped_from_the_tool_envelope() {
        assert!(antenna_report(None).is_none());
        let report = AntennaReport {
            design: AntennaDesign::Dipole,
            frequency_hz: 145_500_000.0,
            wavelength_m: 2.06,
            velocity_factor: 0.95,
            parts: Vec::new(),
            geometry: AntennaGeometry {
                segments: Vec::new(),
                feed: AntennaPoint::ORIGIN,
            },
            feedpoint_ohms: None,
            balanced: true,
            notes: Vec::new(),
        };
        let response = ToolResponse::Antenna(report.clone());
        assert_eq!(antenna_report(Some(&response)), Some(&report));
    }

    #[test]
    fn lengths_drop_to_centimetres_below_a_metre() {
        assert_eq!(format_length(10.0456, Unit::Metres), "10.046 m");
        assert_eq!(format_length(0.482, Unit::Metres), "48.2 cm");
    }

    #[test]
    fn inches_carry_into_feet_after_rounding() {
        assert_eq!(format_length(10.046, Unit::Feet), "32 ft 11.5 in");
        assert_eq!(format_length(0.2, Unit::Feet), "7.9 in");
        assert_eq!(
            format_length(12.0 * 3.99 * 0.0254, Unit::Feet),
            "3 ft 11.9 in"
        );
        assert_eq!(
            format_length(4.0 * 0.3048 - 0.0001, Unit::Feet),
            "4 ft 0.0 in"
        );
        assert_eq!(format_length(f64::NAN, Unit::Metres), "-");
    }

    #[test]
    fn impedance_rounds_an_estimate_and_explains_a_missing_one() {
        assert_eq!(format_impedance(Some(73.0)), "\u{2248} 73 \u{3a9}");
        assert_eq!(format_impedance(Some(36.4)), "\u{2248} 36 \u{3a9}");
        assert_eq!(format_impedance(None), "set by its own matching network");
    }

    #[test]
    fn bounds_cover_every_axis_and_the_feedpoint() {
        let bounds = bounds_of(&yagi());
        assert_eq!(
            bounds.x,
            Extent {
                min: -0.53,
                max: 0.53,
                size: 1.06
            }
        );
        assert_eq!(
            bounds.y,
            Extent {
                min: 0.0,
                max: 0.0,
                size: 0.0
            }
        );
        assert_eq!(
            bounds.z,
            Extent {
                min: 0.0,
                max: 1.2,
                size: 1.2
            }
        );
    }

    #[test]
    fn the_feedline_is_left_out_of_the_antenna_size() {
        let quad = AntennaGeometry {
            feed: AntennaPoint::ORIGIN,
            segments: vec![
                segment(
                    "Side",
                    AntennaSegmentRole::Driven,
                    [-0.5, 0.0, 0.0],
                    [0.5, 0.0, 0.0],
                ),
                segment(
                    "Side",
                    AntennaSegmentRole::Driven,
                    [-0.5, 1.0, 0.0],
                    [0.5, 1.0, 0.0],
                ),
                segment(
                    "Matching line",
                    AntennaSegmentRole::Feedline,
                    [0.0, 0.0, 0.0],
                    [0.0, -0.7, 0.0],
                ),
            ],
        };
        assert_eq!(
            structure_bounds(&quad).y,
            Extent {
                min: 0.0,
                max: 1.0,
                size: 1.0
            }
        );
        assert!(close(bounds_of(&quad).y.size, 1.7, 9));
    }

    #[test]
    fn a_flat_antenna_is_drawn_face_on() {
        assert_eq!(plan_view(&bounds_of(&dipole())).label, "Front view");
        assert_eq!(plan_view(&bounds_of(&vertical())).label, "Front view");
    }

    #[test]
    fn a_boom_is_seen_from_above_pointing_away() {
        let view = plan_view(&bounds_of(&yagi()));
        assert_eq!(view.label, "Top view");
        assert_eq!(view.horizontal, Axis::X);
        assert_eq!(view.vertical, Axis::Z);
        let reflector = project(&AntennaPoint::ORIGIN, view.angles);
        let director = project(&AntennaPoint::new(0.0, 0.0, 1.2), view.angles);
        assert!(director.y < reflector.y);
    }

    #[test]
    fn a_deep_narrow_antenna_is_seen_from_the_side() {
        let bounds = Bounds {
            x: Extent {
                min: 0.0,
                max: 0.0,
                size: 0.0,
            },
            y: Extent {
                min: 0.0,
                max: 2.0,
                size: 2.0,
            },
            z: Extent {
                min: 0.0,
                max: 3.0,
                size: 3.0,
            },
        };
        assert_eq!(plan_view(&bounds).label, "Side view");
    }

    #[test]
    fn projection_keeps_up_up_and_turns_depth_into_height() {
        let flat = project(
            &AntennaPoint::new(2.0, 1.0, 0.0),
            Angles {
                yaw: 0.0,
                pitch: 0.0,
            },
        );
        assert!(close(flat.x, 2.0, 9) && close(flat.y, -1.0, 9));
        let top = project(
            &AntennaPoint::new(1.0, 5.0, 2.0),
            Angles {
                yaw: 0.0,
                pitch: 90.0,
            },
        );
        assert!(close(top.x, 1.0, 9) && close(top.y, 2.0, 9));
        let side = project(
            &AntennaPoint::new(3.0, 1.0, 2.0),
            Angles {
                yaw: 90.0,
                pitch: 0.0,
            },
        );
        assert!(close(side.x, 2.0, 9) && close(side.y, -1.0, 9));
        let spun = project(&AntennaPoint::ORIGIN, ISOMETRIC);
        assert!(close(spun.x, 0.0, 12) && close(spun.y, 0.0, 12));
    }

    const VIEWPORT: Viewport = Viewport {
        width: 200.0,
        height: 100.0,
        padding: 10.0,
    };

    #[test]
    fn a_fit_fills_the_viewport_inside_the_padding() {
        let fit = fit_to(
            &[Point2 { x: -2.0, y: -1.0 }, Point2 { x: 2.0, y: 1.0 }],
            VIEWPORT,
        );
        let left = place(Point2 { x: -2.0, y: -1.0 }, fit);
        let right = place(Point2 { x: 2.0, y: 1.0 }, fit);
        assert!(close(left.x, 20.0, 6) && close(right.x, 180.0, 6));
        assert!(close(left.y, 10.0, 6) && close(right.y, 90.0, 6));
    }

    #[test]
    fn a_drawing_with_no_height_is_centred() {
        let fit = fit_to(
            &[Point2 { x: 0.0, y: 0.0 }, Point2 { x: 4.0, y: 0.0 }],
            VIEWPORT,
        );
        assert_eq!(
            place(Point2 { x: 0.0, y: 0.0 }, fit),
            Point2 { x: 10.0, y: 50.0 }
        );
        assert_eq!(
            place(Point2 { x: 4.0, y: 0.0 }, fit),
            Point2 { x: 190.0, y: 50.0 }
        );
        assert_eq!(
            fit_to(&[], VIEWPORT),
            Fit {
                scale: 1.0,
                offset_x: 100.0,
                offset_y: 50.0
            }
        );
    }

    #[test]
    fn the_scale_bar_picks_a_round_length_that_fits() {
        assert_eq!(
            scale_bar(100.0, 250.0, Unit::Metres),
            ScaleBar {
                meters: 2.0,
                pixels: 200.0,
                label: "2.000 m".to_owned()
            }
        );
        let small = scale_bar(1000.0, 120.0, Unit::Metres);
        assert!(close(small.meters, 0.1, 12) && close(small.pixels, 100.0, 9));
        let feet = scale_bar(100.0, 200.0, Unit::Feet);
        assert!(close(feet.meters, 5.0 * 0.3048, 9));
        assert_eq!(feet.label, "5 ft 0.0 in");
        assert_eq!(scale_bar(0.0, 200.0, Unit::Metres).label, "-");
    }

    #[test]
    fn the_legend_lists_only_the_roles_in_use_in_a_fixed_order() {
        assert_eq!(
            roles_in(&yagi()),
            [
                AntennaSegmentRole::Driven,
                AntennaSegmentRole::Parasitic,
                AntennaSegmentRole::Structure
            ]
        );
        assert_eq!(roles_in(&dipole()), [AntennaSegmentRole::Driven]);
    }

    #[test]
    fn the_ground_grid_sits_under_the_antenna() {
        let grid = ground_grid(&bounds_of(&yagi()));
        assert_eq!(grid.len(), 2 * (GRID_DIVISIONS + 1));
        assert!(grid.iter().all(|(from, _)| from.y_m < 0.0));
        assert!(
            ground_grid(&bounds_of(&AntennaGeometry {
                segments: Vec::new(),
                feed: AntennaPoint::ORIGIN
            }))
            .is_empty()
        );
    }

    #[test]
    fn dragging_turns_the_view_and_stops_short_of_the_pole() {
        let turned = turn(ISOMETRIC, 10.0, 1000.0);
        assert!(close(turned.yaw, -28.0, 9));
        assert_eq!(turned.pitch, MAX_PITCH);
    }
}
