//! Antenna calculator: resonant element lengths for the designs an operator actually builds.
//!
//! Every length is free-space wavelength scaled by the element's end-effect factor — the
//! `468/f` rule generalised, with the factor exposed instead of baked in, because it is the one
//! number that changes with wire diameter, insulation and height. Feedpoint impedances are
//! free-space estimates: a real antenna at a real height is trimmed, not computed.

mod geometry;

use geometry::BoomElement;
use sdrmm_wire::{
    ANTENNA_TOOL_ID, AntennaDesign, AntennaGeometry, AntennaPart, AntennaReport, AntennaRequest,
    GroundPlaneParams, InvertedVParams, MAX_ANTENNA_FREQ_HZ, MAX_APEX_ANGLE_DEG,
    MAX_FEEDLINE_VELOCITY_FACTOR, MAX_RADIAL_SLOPE_DEG, MAX_RADIALS, MAX_VELOCITY_FACTOR,
    MAX_YAGI_DIRECTORS, MAX_YAGI_SPACING_WL, MIN_ANTENNA_FREQ_HZ, MIN_APEX_ANGLE_DEG,
    MIN_FEEDLINE_VELOCITY_FACTOR, MIN_VELOCITY_FACTOR, MIN_YAGI_SPACING_WL, ToolCategory,
    ToolDescriptor, ToolRequest, ToolResponse, YagiParams,
};

use crate::{Tool, ToolError};

const SPEED_OF_LIGHT_M_S: f64 = 299_792_458.0;

/// Radials are cut long: they are not the resonant element, and a slightly long radial pulls
/// the feedpoint impedance the right way.
const RADIAL_OVERSHOOT: f64 = 1.05;

/// A five-eighths vertical takes no radial count from the operator: four is what the design
/// assumes, and the loading coil is the part that gets adjusted.
const FIVE_EIGHTHS_RADIALS: u8 = 4;

pub struct AntennaTool;

impl Tool for AntennaTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: ANTENNA_TOOL_ID.to_owned(),
            name: "Antenna calculator".to_owned(),
            summary: "Element lengths, boom positions and feedpoint estimates for common \
                      designs."
                .to_owned(),
            category: ToolCategory::Calculator,
            needs_hardware: false,
        }
    }

    fn run(&self, request: ToolRequest) -> Result<ToolResponse, ToolError> {
        match request {
            ToolRequest::Antenna(request) => report(&request).map(ToolResponse::Antenna),
            other => Err(ToolError::WrongTool {
                tool: ANTENNA_TOOL_ID,
                got: other.tool_id().to_owned(),
            }),
        }
    }
}

/// Work out one design.
///
/// # Errors
/// [`ToolError::Invalid`] when a number is outside the range the wire types declare.
pub fn report(request: &AntennaRequest) -> Result<AntennaReport, ToolError> {
    validate(request)?;
    let lengths = Lengths {
        wavelength_m: SPEED_OF_LIGHT_M_S / request.frequency_hz,
        velocity_factor: request.velocity_factor,
        feedline_velocity_factor: request.feedline_velocity_factor,
    };
    let design = match request.design {
        AntennaDesign::Dipole => dipole(&lengths),
        AntennaDesign::InvertedV(params) => inverted_v(&lengths, params),
        AntennaDesign::GroundPlane(params) => ground_plane(&lengths, params),
        AntennaDesign::FiveEighthsVertical => five_eighths_vertical(&lengths),
        AntennaDesign::FoldedDipole => folded_dipole(&lengths),
        AntennaDesign::JPole => j_pole(&lengths),
        AntennaDesign::Yagi(params) => yagi(&lengths, params),
        AntennaDesign::QuadLoop => quad_loop(&lengths),
        AntennaDesign::EndFedHalfWave => end_fed_half_wave(&lengths),
    };
    Ok(AntennaReport {
        design: request.design,
        frequency_hz: request.frequency_hz,
        wavelength_m: lengths.wavelength_m,
        velocity_factor: request.velocity_factor,
        parts: design.parts,
        geometry: design.geometry,
        feedpoint_ohms: design.feedpoint_ohms,
        balanced: design.balanced,
        notes: design.notes,
    })
}

struct Lengths {
    wavelength_m: f64,
    velocity_factor: f64,
    feedline_velocity_factor: f64,
}

impl Lengths {
    /// A conductor that has to resonate, so the end-effect factor applies.
    fn resonant(&self, wavelengths: f64) -> f64 {
        wavelengths * self.wavelength_m * self.velocity_factor
    }

    /// A spacing or a boom run: geometry in free space, uncorrected.
    fn free(&self, wavelengths: f64) -> f64 {
        wavelengths * self.wavelength_m
    }

    /// A length of coax, where the cable's own velocity factor is what counts.
    fn line(&self, wavelengths: f64) -> f64 {
        wavelengths * self.wavelength_m * self.feedline_velocity_factor
    }
}

struct Design {
    parts: Vec<AntennaPart>,
    geometry: AntennaGeometry,
    feedpoint_ohms: Option<f64>,
    balanced: bool,
    notes: Vec<String>,
}

fn dipole(lengths: &Lengths) -> Design {
    let leg = lengths.resonant(0.25);
    Design {
        parts: vec![
            part("Leg", 2, leg),
            noted_part(
                "Tip-to-tip span",
                1,
                lengths.resonant(0.5),
                "both legs plus the feedpoint gap",
            ),
        ],
        geometry: geometry::dipole(leg),
        feedpoint_ohms: Some(73.0),
        balanced: true,
        notes: vec![
            TRIM_NOTE.to_owned(),
            "Balanced feedpoint: put a 1:1 current balun where the coax meets the legs, or the \
             braid radiates too."
                .to_owned(),
            "Height matters more than length. Below a quarter wavelength above ground the \
             feedpoint impedance drops well under 73 Ω."
                .to_owned(),
        ],
    }
}

fn inverted_v(lengths: &Lengths, params: InvertedVParams) -> Design {
    let droop = ((MAX_APEX_ANGLE_DEG - params.apex_angle_deg) / 90.0).clamp(0.0, 1.0);
    let leg = lengths.resonant(0.25) * (1.0 - 0.05 * droop);
    let half_apex = params.apex_angle_deg.to_radians() / 2.0;
    Design {
        parts: vec![
            noted_part("Leg", 2, leg, "measured along the wire, not the ground"),
            noted_part(
                "Horizontal span",
                1,
                2.0 * leg * half_apex.sin(),
                "end to end on the ground, at the stated apex angle",
            ),
            noted_part(
                "Drop below the apex",
                1,
                leg * half_apex.cos(),
                "how far each end hangs below the feedpoint",
            ),
        ],
        geometry: geometry::inverted_v(leg, half_apex),
        feedpoint_ohms: Some(73.0 - 23.0 * droop),
        balanced: true,
        notes: vec![
            format!(
                "Legs are {:.1}% shorter than a flat dipole for the {:.0}° apex.",
                5.0 * droop,
                params.apex_angle_deg
            ),
            TRIM_NOTE.to_owned(),
            "Keep the ends well clear of people and metal: that is where the voltage is."
                .to_owned(),
        ],
    }
}

fn ground_plane(lengths: &Lengths, params: GroundPlaneParams) -> Design {
    let slope = (params.radial_slope_deg / 45.0).clamp(0.0, 1.0);
    let mut notes = vec![
        format!(
            "Radials are cut {:.0}% long; they set the impedance, not the resonance.",
            (RADIAL_OVERSHOOT - 1.0) * 100.0
        ),
        format!(
            "Sloping the radials {:.0}° below horizontal lifts the feedpoint towards 50 Ω.",
            params.radial_slope_deg
        ),
        TRIM_NOTE.to_owned(),
    ];
    if params.radials < 4 {
        notes.push(
            "Fewer than four radials makes the pattern lopsided and the feedpoint hard to \
             predict."
                .to_owned(),
        );
    }
    let radiator = lengths.resonant(0.25);
    let radial = radiator * RADIAL_OVERSHOOT;
    Design {
        parts: vec![
            part("Radiator", 1, radiator),
            noted_part(
                "Radial",
                params.radials,
                radial,
                "bonded to the coax braid at the base",
            ),
        ],
        geometry: geometry::ground_plane(radiator, radial, params.radials, params.radial_slope_deg),
        feedpoint_ohms: Some(36.0 + 14.0 * slope),
        balanced: false,
        notes,
    }
}

fn five_eighths_vertical(lengths: &Lengths) -> Design {
    let radiator = lengths.resonant(0.625);
    let radial = lengths.resonant(0.25) * RADIAL_OVERSHOOT;
    Design {
        parts: vec![
            part("Radiator", 1, radiator),
            noted_part(
                "Radial",
                FIVE_EIGHTHS_RADIALS,
                radial,
                "bonded to the coax braid at the base",
            ),
            noted_part(
                "Base loading coil, wire length",
                1,
                lengths.resonant(0.125),
                "roughly an eighth wavelength of electrical length; wind it, then adjust the \
                 turns for the lowest SWR",
            ),
        ],
        geometry: geometry::five_eighths_vertical(radiator, radial, FIVE_EIGHTHS_RADIALS),
        feedpoint_ohms: None,
        balanced: false,
        notes: vec![
            "A five-eighths vertical is not resonant on its own — the base coil is what makes \
             it match, so the impedance depends on the coil, not on the rod."
                .to_owned(),
            "About 3 dB over a quarter-wave vertical at low angles, which is the whole point of \
             the extra height."
                .to_owned(),
        ],
    }
}

fn folded_dipole(lengths: &Lengths) -> Design {
    let spacing = lengths.free(0.01);
    let conductor = lengths.resonant(0.5);
    Design {
        parts: vec![
            noted_part(
                "Conductor",
                2,
                conductor,
                "the fed side and the shorted side of the loop",
            ),
            noted_part(
                "End spacing",
                2,
                spacing,
                "not critical; 1% of a wavelength works",
            ),
            noted_part(
                "Total wire",
                1,
                2.0 * conductor + 2.0 * spacing,
                "one continuous loop",
            ),
        ],
        geometry: geometry::folded_dipole(conductor, spacing),
        feedpoint_ohms: Some(292.0),
        balanced: true,
        notes: vec![
            "Four times the impedance of a plain dipole: feed it with 300 Ω twin lead, or with \
             coax through a 4:1 balun."
                .to_owned(),
            "Wider bandwidth than a plain dipole, which is why it is the usual driven element \
             for a Yagi."
                .to_owned(),
            TRIM_NOTE.to_owned(),
        ],
    }
}

fn j_pole(lengths: &Lengths) -> Design {
    let radiator = lengths.resonant(0.75);
    let stub = lengths.resonant(0.25);
    let feed_height = lengths.free(0.05);
    let spacing = lengths.free(0.02);
    Design {
        parts: vec![
            noted_part(
                "Radiator (long element)",
                1,
                radiator,
                "half-wave radiator on top of the stub's long side",
            ),
            noted_part(
                "Matching stub (short element)",
                1,
                stub,
                "shorted to the radiator at the bottom",
            ),
            noted_part(
                "Feedpoint above the short",
                1,
                feed_height,
                "start here and slide both tap points together for the lowest SWR",
            ),
            noted_part(
                "Element spacing",
                1,
                spacing,
                "between the two vertical elements",
            ),
        ],
        geometry: geometry::j_pole(radiator, stub, spacing, feed_height),
        feedpoint_ohms: Some(50.0),
        balanced: false,
        notes: vec![
            "The stub is a quarter-wave transformer: it is the match, so the tap position is \
             the adjustment, not the element length."
                .to_owned(),
            "The coax braid is part of the antenna unless a choke goes just below the feed \
             point."
                .to_owned(),
        ],
    }
}

fn yagi(lengths: &Lengths, params: YagiParams) -> Design {
    let driven = lengths.resonant(0.5);
    let spacing = lengths.free(params.spacing_wavelengths);
    let mut elements = vec![
        BoomElement {
            name: "Reflector".to_owned(),
            length_m: driven * 1.05,
            position_m: 0.0,
            driven: false,
        },
        BoomElement {
            name: "Driven element".to_owned(),
            length_m: driven,
            position_m: spacing,
            driven: true,
        },
    ];
    for index in 0..params.directors {
        let shortening = 0.95 - 0.01 * f64::from(index);
        elements.push(BoomElement {
            name: format!("Director {}", index + 1),
            length_m: driven * shortening.max(0.88),
            position_m: spacing * f64::from(index + 2),
            driven: false,
        });
    }
    let boom = spacing * f64::from(params.directors + 1);
    let mut parts = vec![
        noted_part(
            "Reflector",
            1,
            elements[0].length_m,
            "behind the driven element",
        ),
        placed_part(
            "Driven element",
            driven,
            spacing,
            "split at the centre for the feedpoint",
        ),
    ];
    for element in &elements[2..] {
        parts.push(placed_part(
            &element.name,
            element.length_m,
            element.position_m,
            "in front of the driven element",
        ));
    }
    parts.push(noted_part(
        "Boom",
        1,
        boom,
        "reflector to the last director, before any overhang",
    ));
    Design {
        parts,
        geometry: geometry::yagi(&elements, boom),
        feedpoint_ohms: Some(match params.directors {
            0 => 40.0,
            1 => 30.0,
            _ => 25.0,
        }),
        balanced: true,
        notes: vec![
            "Element lengths are a starting point for thin elements. Thick tubing resonates \
             shorter, and elements bonded to a metal boom need 1–3% more length."
                .to_owned(),
            "The driven element runs well below 50 Ω: match it with a gamma, a hairpin, or a \
             folded driven element."
                .to_owned(),
            format!(
                "{} elements at {:.2} λ spacing. Wider spacing buys gain and costs bandwidth \
                 and front-to-back.",
                u16::from(params.directors) + 2,
                params.spacing_wavelengths
            ),
        ],
    }
}

fn quad_loop(lengths: &Lengths) -> Design {
    let circumference = lengths.free(1.02);
    let side = circumference / 4.0;
    let matching_line = lengths.line(0.25);
    Design {
        parts: vec![
            part("Side", 4, side),
            noted_part(
                "Total wire",
                1,
                circumference,
                "one full wavelength plus 2% for the closed loop",
            ),
            noted_part(
                "Quarter-wave 75 Ω matching line",
                1,
                matching_line,
                "transforms about 110 Ω down to near 50 Ω",
            ),
        ],
        geometry: geometry::quad_loop(side, matching_line),
        feedpoint_ohms: Some(110.0),
        balanced: true,
        notes: vec![
            "A closed loop has no ends, so the wire end-effect factor does not apply — the 2% \
             is the loop's own correction."
                .to_owned(),
            "Fed at the middle of a side it is horizontally polarised; fed at a corner, \
             vertically."
                .to_owned(),
            "The matching line's length follows the coax velocity factor, not the wire's."
                .to_owned(),
        ],
    }
}

fn end_fed_half_wave(lengths: &Lengths) -> Design {
    let radiator = lengths.resonant(0.5);
    let counterpoise = lengths.free(0.05);
    Design {
        parts: vec![
            part("Radiator", 1, radiator),
            noted_part(
                "Counterpoise",
                1,
                counterpoise,
                "on the transformer's ground side; the transformer needs something to work \
                 against",
            ),
        ],
        geometry: geometry::end_fed_half_wave(radiator, counterpoise),
        feedpoint_ohms: Some(2_450.0),
        balanced: false,
        notes: vec![
            "Fed at a voltage maximum, so it wants a 49:1 transformer — and the transformer, \
             not the wire, is what usually limits the power."
                .to_owned(),
            "Without a choke after the transformer the feedline becomes the counterpoise, and \
             the shack becomes part of the antenna."
                .to_owned(),
            TRIM_NOTE.to_owned(),
        ],
    }
}

const TRIM_NOTE: &str = "Cut about 2% long and trim for the lowest SWR: the correction factor is an estimate, the \
     antenna is the measurement.";

fn part(name: &str, count: u8, length_m: f64) -> AntennaPart {
    AntennaPart {
        name: name.to_owned(),
        count,
        length_m,
        position_m: None,
        detail: None,
    }
}

fn noted_part(name: &str, count: u8, length_m: f64, detail: &str) -> AntennaPart {
    AntennaPart {
        detail: Some(detail.to_owned()),
        ..part(name, count, length_m)
    }
}

fn placed_part(name: &str, length_m: f64, position_m: f64, detail: &str) -> AntennaPart {
    AntennaPart {
        position_m: Some(position_m),
        ..noted_part(name, 1, length_m, detail)
    }
}

fn validate(request: &AntennaRequest) -> Result<(), ToolError> {
    range(
        "frequency_hz",
        request.frequency_hz,
        MIN_ANTENNA_FREQ_HZ,
        MAX_ANTENNA_FREQ_HZ,
    )?;
    range(
        "velocity_factor",
        request.velocity_factor,
        MIN_VELOCITY_FACTOR,
        MAX_VELOCITY_FACTOR,
    )?;
    range(
        "feedline_velocity_factor",
        request.feedline_velocity_factor,
        MIN_FEEDLINE_VELOCITY_FACTOR,
        MAX_FEEDLINE_VELOCITY_FACTOR,
    )?;
    match request.design {
        AntennaDesign::InvertedV(params) => range(
            "apex_angle_deg",
            params.apex_angle_deg,
            MIN_APEX_ANGLE_DEG,
            MAX_APEX_ANGLE_DEG,
        ),
        AntennaDesign::GroundPlane(params) => {
            range(
                "radial_slope_deg",
                params.radial_slope_deg,
                0.0,
                MAX_RADIAL_SLOPE_DEG,
            )?;
            count("radials", params.radials, 1, MAX_RADIALS)
        }
        AntennaDesign::Yagi(params) => {
            range(
                "spacing_wavelengths",
                params.spacing_wavelengths,
                MIN_YAGI_SPACING_WL,
                MAX_YAGI_SPACING_WL,
            )?;
            count("directors", params.directors, 0, MAX_YAGI_DIRECTORS)
        }
        _ => Ok(()),
    }
}

fn range(field: &str, value: f64, min: f64, max: f64) -> Result<(), ToolError> {
    if value.is_finite() && (min..=max).contains(&value) {
        return Ok(());
    }
    Err(ToolError::Invalid(format!(
        "{field} must be between {min} and {max}, got {value}"
    )))
}

fn count(field: &str, value: u8, min: u8, max: u8) -> Result<(), ToolError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(ToolError::Invalid(format!(
        "{field} must be between {min} and {max}, got {value}"
    )))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AntennaPoint, AntennaSegment};

    use super::*;

    fn at(frequency_hz: f64, design: AntennaDesign) -> AntennaReport {
        report(&AntennaRequest {
            frequency_hz,
            design,
            ..AntennaRequest::default()
        })
        .expect("a valid request")
    }

    fn part_named<'a>(report: &'a AntennaReport, name: &str) -> &'a AntennaPart {
        report
            .parts
            .iter()
            .find(|part| part.name == name)
            .unwrap_or_else(|| panic!("{name} is one of the parts"))
    }

    /// The classic 468/f(MHz) rule in feet, which is a half wave at a 0.951 factor. The
    /// default 0.95 must land on it to within the width of the rule itself.
    #[test]
    fn a_dipole_matches_the_468_over_f_rule() {
        let report = at(14_200_000.0, AntennaDesign::Dipole);
        let expected_m = 468.0 / 14.2 * 0.304_8;
        let span = part_named(&report, "Tip-to-tip span").length_m;
        assert!(
            (span - expected_m).abs() / expected_m < 0.005,
            "span {span} m against the rule's {expected_m} m"
        );
        assert!((part_named(&report, "Leg").length_m - span / 2.0).abs() < 1e-9);
        assert_eq!(part_named(&report, "Leg").count, 2);
        assert!(report.balanced);
        assert_eq!(report.feedpoint_ohms, Some(73.0));
    }

    #[test]
    fn the_wavelength_is_free_space_and_the_elements_are_not() {
        let report = at(145_500_000.0, AntennaDesign::Dipole);
        assert!((report.wavelength_m - SPEED_OF_LIGHT_M_S / 145_500_000.0).abs() < 1e-12);
        let leg = part_named(&report, "Leg").length_m;
        assert!((leg - report.wavelength_m * 0.25 * 0.95).abs() < 1e-12);
    }

    #[test]
    fn a_lower_velocity_factor_makes_every_element_shorter() {
        let slow = report(&AntennaRequest {
            frequency_hz: 145_500_000.0,
            velocity_factor: 0.90,
            design: AntennaDesign::Dipole,
            ..AntennaRequest::default()
        })
        .expect("a valid request");
        let default = at(145_500_000.0, AntennaDesign::Dipole);
        assert!(part_named(&slow, "Leg").length_m < part_named(&default, "Leg").length_m);
        assert!((slow.wavelength_m - default.wavelength_m).abs() < 1e-12);
    }

    /// A flat inverted V is a dipole; closing the apex shortens the legs and drops the
    /// feedpoint impedance towards 50 Ω.
    #[test]
    fn an_inverted_v_converges_on_a_dipole_as_it_opens() {
        let flat = at(
            14_200_000.0,
            AntennaDesign::InvertedV(InvertedVParams {
                apex_angle_deg: 180.0,
            }),
        );
        let dipole = at(14_200_000.0, AntennaDesign::Dipole);
        assert!(
            (part_named(&flat, "Leg").length_m - part_named(&dipole, "Leg").length_m).abs() < 1e-9
        );
        assert_eq!(flat.feedpoint_ohms, Some(73.0));

        let drooped = at(
            14_200_000.0,
            AntennaDesign::InvertedV(InvertedVParams {
                apex_angle_deg: 90.0,
            }),
        );
        assert!(part_named(&drooped, "Leg").length_m < part_named(&flat, "Leg").length_m);
        assert_eq!(drooped.feedpoint_ohms, Some(50.0));
        assert!(
            part_named(&drooped, "Horizontal span").length_m
                < 2.0 * part_named(&drooped, "Leg").length_m
        );
    }

    #[test]
    fn a_ground_plane_cuts_its_radials_long_and_counts_them() {
        let report = at(
            145_500_000.0,
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 6,
                radial_slope_deg: 45.0,
            }),
        );
        let radiator = part_named(&report, "Radiator").length_m;
        let radial = part_named(&report, "Radial");
        assert_eq!(radial.count, 6);
        assert!((radial.length_m - radiator * RADIAL_OVERSHOOT).abs() < 1e-12);
        assert_eq!(report.feedpoint_ohms, Some(50.0));
        assert!(!report.balanced);

        let flat = at(
            145_500_000.0,
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 4,
                radial_slope_deg: 0.0,
            }),
        );
        assert_eq!(flat.feedpoint_ohms, Some(36.0));
    }

    #[test]
    fn a_thin_ground_plane_says_so() {
        let report = at(
            145_500_000.0,
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 2,
                radial_slope_deg: 45.0,
            }),
        );
        assert!(
            report
                .notes
                .iter()
                .any(|note| note.contains("Fewer than four radials")),
            "{:?}",
            report.notes
        );
    }

    #[test]
    fn a_yagi_orders_its_elements_along_the_boom() {
        let report = at(
            432_000_000.0,
            AntennaDesign::Yagi(YagiParams {
                directors: 3,
                spacing_wavelengths: 0.2,
            }),
        );
        let elements: Vec<&AntennaPart> = report
            .parts
            .iter()
            .filter(|part| part.position_m.is_some() || part.name == "Reflector")
            .collect();
        assert_eq!(elements.len(), 5);

        let reflector = part_named(&report, "Reflector").length_m;
        let driven = part_named(&report, "Driven element").length_m;
        assert!(reflector > driven);

        let mut previous = 0.0;
        for index in 1..=3 {
            let director = part_named(&report, &format!("Director {index}"));
            assert!(director.length_m < driven);
            let position = director.position_m.expect("a director sits on the boom");
            assert!(position > previous);
            previous = position;
        }
        assert!((part_named(&report, "Boom").length_m - previous).abs() < 1e-9);
        assert!(
            part_named(&report, "Director 2").length_m < part_named(&report, "Director 1").length_m
        );
    }

    #[test]
    fn a_two_element_yagi_has_no_directors() {
        let report = at(
            432_000_000.0,
            AntennaDesign::Yagi(YagiParams {
                directors: 0,
                spacing_wavelengths: 0.15,
            }),
        );
        assert!(
            !report
                .parts
                .iter()
                .any(|part| part.name.starts_with("Director"))
        );
        assert_eq!(report.feedpoint_ohms, Some(40.0));
        let spacing = report.wavelength_m * 0.15;
        assert!((part_named(&report, "Boom").length_m - spacing).abs() < 1e-9);
    }

    /// The loop is closed, so the wire end-effect factor must not touch it — only the coax
    /// matching section follows a velocity factor, and it follows the feedline's.
    #[test]
    fn a_quad_loop_ignores_the_wire_factor_and_the_matching_line_does_not() {
        let request = AntennaRequest {
            frequency_hz: 28_400_000.0,
            velocity_factor: 0.90,
            feedline_velocity_factor: 0.80,
            design: AntennaDesign::QuadLoop,
        };
        let report = report(&request).expect("a valid request");
        let total = part_named(&report, "Total wire").length_m;
        assert!((total - report.wavelength_m * 1.02).abs() < 1e-9);
        assert!((part_named(&report, "Side").length_m - total / 4.0).abs() < 1e-12);
        assert_eq!(part_named(&report, "Side").count, 4);

        let line = part_named(&report, "Quarter-wave 75 Ω matching line").length_m;
        assert!((line - report.wavelength_m * 0.25 * 0.80).abs() < 1e-9);
    }

    #[test]
    fn a_j_pole_is_three_quarters_of_a_wave_over_a_quarter_wave_stub() {
        let report = at(145_500_000.0, AntennaDesign::JPole);
        let radiator = part_named(&report, "Radiator (long element)").length_m;
        let stub = part_named(&report, "Matching stub (short element)").length_m;
        assert!((radiator / stub - 3.0).abs() < 1e-9);
        assert!(!report.balanced);
    }

    #[test]
    fn a_folded_dipole_is_four_times_a_dipole() {
        let folded = at(145_500_000.0, AntennaDesign::FoldedDipole);
        let dipole = at(145_500_000.0, AntennaDesign::Dipole);
        assert_eq!(folded.feedpoint_ohms, Some(292.0));
        assert!(
            (part_named(&folded, "Conductor").length_m
                - part_named(&dipole, "Tip-to-tip span").length_m)
                .abs()
                < 1e-12
        );
        assert!(
            part_named(&folded, "Total wire").length_m > 2.0 * dipole.wavelength_m * 0.5 * 0.95
        );
    }

    /// The coil, not the rod, sets the match, so quoting a feedpoint impedance would be a
    /// number the builder cannot use.
    #[test]
    fn a_five_eighths_vertical_reports_no_feedpoint_impedance() {
        let report = at(145_500_000.0, AntennaDesign::FiveEighthsVertical);
        assert_eq!(report.feedpoint_ohms, None);
        assert!(
            (part_named(&report, "Radiator").length_m - report.wavelength_m * 0.625 * 0.95).abs()
                < 1e-12
        );
    }

    #[test]
    fn an_end_fed_half_wave_carries_a_counterpoise() {
        let report = at(7_100_000.0, AntennaDesign::EndFedHalfWave);
        assert!(
            (part_named(&report, "Radiator").length_m - report.wavelength_m * 0.5 * 0.95).abs()
                < 1e-12
        );
        assert!(part_named(&report, "Counterpoise").length_m > 0.0);
        assert!(!report.balanced);
    }

    #[test]
    fn every_design_produces_parts_and_says_something_about_them() {
        for design in every_design() {
            let report = at(145_500_000.0, design);
            assert_eq!(report.design, design);
            assert!(!report.parts.is_empty(), "{}", design.type_id());
            assert!(!report.notes.is_empty(), "{}", design.type_id());
            for part in &report.parts {
                assert!(
                    part.length_m > 0.0 && part.length_m.is_finite(),
                    "{} has a {} of {} m",
                    design.type_id(),
                    part.name,
                    part.length_m
                );
                assert!(part.count >= 1, "{}: {}", design.type_id(), part.name);
            }
        }
    }

    fn every_design() -> Vec<AntennaDesign> {
        vec![
            AntennaDesign::Dipole,
            AntennaDesign::InvertedV(InvertedVParams::default()),
            AntennaDesign::GroundPlane(GroundPlaneParams::default()),
            AntennaDesign::FiveEighthsVertical,
            AntennaDesign::FoldedDipole,
            AntennaDesign::JPole,
            AntennaDesign::Yagi(YagiParams::default()),
            AntennaDesign::QuadLoop,
            AntennaDesign::EndFedHalfWave,
        ]
    }

    #[test]
    fn every_design_draws_itself_at_a_real_size() {
        for design in every_design() {
            let report = at(145_500_000.0, design);
            let geometry = &report.geometry;
            assert!(!geometry.segments.is_empty(), "{}", design.type_id());
            for segment in &geometry.segments {
                for point in [segment.from, segment.to] {
                    assert!(
                        point.x_m.is_finite() && point.y_m.is_finite() && point.z_m.is_finite(),
                        "{}: {} runs off to {point:?}",
                        design.type_id(),
                        segment.label
                    );
                }
                assert!(
                    segment.length_m() > 0.0,
                    "{}: {} has no length",
                    design.type_id(),
                    segment.label
                );
                assert!(
                    segment.length_m() < report.wavelength_m * 2.0,
                    "{}: {} is longer than the antenna",
                    design.type_id(),
                    segment.label
                );
            }
            let feed = geometry.feed;
            assert!(feed.x_m.is_finite() && feed.y_m.is_finite() && feed.z_m.is_finite());
        }
    }

    /// The drawing and the cutting list are the same numbers: wherever a segment carries a
    /// part's name, it is exactly as long as that part.
    #[test]
    fn a_segment_named_after_a_part_is_exactly_that_long() {
        for design in every_design() {
            let report = at(50_000_000.0, design);
            let mut matched = 0;
            for segment in &report.geometry.segments {
                let Some(part) = report.parts.iter().find(|part| part.name == segment.label) else {
                    continue;
                };
                matched += 1;
                assert!(
                    (segment.length_m() - part.length_m).abs() < 1e-9,
                    "{}: {} is drawn {} m but cut {} m",
                    design.type_id(),
                    segment.label,
                    segment.length_m(),
                    part.length_m
                );
            }
            assert!(
                matched > 0,
                "{} draws nothing it also lists",
                design.type_id()
            );
        }
    }

    #[test]
    fn a_dipole_is_drawn_as_two_legs_either_side_of_the_feedpoint() {
        let report = at(14_200_000.0, AntennaDesign::Dipole);
        let legs: Vec<&AntennaSegment> = report
            .geometry
            .segments
            .iter()
            .filter(|segment| segment.label == "Leg")
            .collect();
        assert_eq!(legs.len(), 2);
        let span = part_named(&report, "Tip-to-tip span").length_m;
        let tips: Vec<f64> = legs
            .iter()
            .map(|leg| leg.from.x_m.min(leg.to.x_m))
            .collect();
        assert!((legs[0].length_m() + legs[1].length_m() - span).abs() < 1e-9);
        assert!(tips.iter().any(|tip| *tip < 0.0));
        assert_eq!(report.geometry.feed, AntennaPoint::ORIGIN);
    }

    #[test]
    fn a_yagi_stacks_its_elements_along_the_boom_and_feeds_the_driven_one() {
        let report = at(
            432_000_000.0,
            AntennaDesign::Yagi(YagiParams {
                directors: 3,
                spacing_wavelengths: 0.2,
            }),
        );
        let boom = report
            .geometry
            .segments
            .iter()
            .find(|segment| segment.label == "Boom")
            .expect("a boom");
        assert!((boom.length_m() - part_named(&report, "Boom").length_m).abs() < 1e-9);

        for name in ["Reflector", "Driven element", "Director 1", "Director 3"] {
            let element = report
                .geometry
                .segments
                .iter()
                .find(|segment| segment.label == name)
                .unwrap_or_else(|| panic!("{name} is drawn"));
            assert!(
                (element.from.z_m - element.to.z_m).abs() < 1e-12,
                "{name} lies across the boom"
            );
            assert!((element.from.x_m + element.to.x_m).abs() < 1e-12, "{name}");
        }

        let driven = report
            .geometry
            .segments
            .iter()
            .find(|segment| segment.label == "Driven element")
            .expect("a driven element");
        assert!((report.geometry.feed.z_m - driven.from.z_m).abs() < 1e-12);
        assert!(report.geometry.feed.z_m > 0.0);
    }

    #[test]
    fn a_quad_loop_closes_on_itself() {
        let report = at(28_400_000.0, AntennaDesign::QuadLoop);
        let sides: Vec<&AntennaSegment> = report
            .geometry
            .segments
            .iter()
            .filter(|segment| segment.label == "Side")
            .collect();
        assert_eq!(sides.len(), 4);
        let walk = sides.iter().fold((0.0, 0.0), |(x, y), side| {
            (
                x + side.to.x_m - side.from.x_m,
                y + side.to.y_m - side.from.y_m,
            )
        });
        assert!(walk.0.abs() < 1e-9 && walk.1.abs() < 1e-9, "{walk:?}");
        let perimeter: f64 = sides.iter().map(|side| side.length_m()).sum();
        assert!((perimeter - part_named(&report, "Total wire").length_m).abs() < 1e-9);
    }

    #[test]
    fn a_ground_plane_draws_every_radial_it_lists_and_slopes_them() {
        let report = at(
            145_500_000.0,
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 6,
                radial_slope_deg: 45.0,
            }),
        );
        let radials: Vec<&AntennaSegment> = report
            .geometry
            .segments
            .iter()
            .filter(|segment| segment.label == "Radial")
            .collect();
        assert_eq!(radials.len(), 6);
        for radial in &radials {
            assert!(radial.to.y_m < 0.0, "a sloped radial drops below the base");
        }

        let flat = at(
            145_500_000.0,
            AntennaDesign::GroundPlane(GroundPlaneParams {
                radials: 4,
                radial_slope_deg: 0.0,
            }),
        );
        for radial in flat
            .geometry
            .segments
            .iter()
            .filter(|segment| segment.label == "Radial")
        {
            assert!(radial.to.y_m.abs() < 1e-12);
        }
    }

    #[test]
    fn out_of_range_numbers_are_refused_by_name() {
        let cases = [
            (
                AntennaRequest {
                    frequency_hz: 0.0,
                    ..AntennaRequest::default()
                },
                "frequency_hz",
            ),
            (
                AntennaRequest {
                    frequency_hz: f64::NAN,
                    ..AntennaRequest::default()
                },
                "frequency_hz",
            ),
            (
                AntennaRequest {
                    velocity_factor: 1.4,
                    ..AntennaRequest::default()
                },
                "velocity_factor",
            ),
            (
                AntennaRequest {
                    feedline_velocity_factor: 0.1,
                    ..AntennaRequest::default()
                },
                "feedline_velocity_factor",
            ),
            (
                AntennaRequest {
                    design: AntennaDesign::Yagi(YagiParams {
                        directors: MAX_YAGI_DIRECTORS + 1,
                        spacing_wavelengths: 0.2,
                    }),
                    ..AntennaRequest::default()
                },
                "directors",
            ),
            (
                AntennaRequest {
                    design: AntennaDesign::Yagi(YagiParams {
                        directors: 2,
                        spacing_wavelengths: 0.9,
                    }),
                    ..AntennaRequest::default()
                },
                "spacing_wavelengths",
            ),
            (
                AntennaRequest {
                    design: AntennaDesign::GroundPlane(GroundPlaneParams {
                        radials: 0,
                        radial_slope_deg: 45.0,
                    }),
                    ..AntennaRequest::default()
                },
                "radials",
            ),
            (
                AntennaRequest {
                    design: AntennaDesign::InvertedV(InvertedVParams {
                        apex_angle_deg: 200.0,
                    }),
                    ..AntennaRequest::default()
                },
                "apex_angle_deg",
            ),
        ];
        for (request, field) in cases {
            let err = report(&request).expect_err("out of range");
            assert!(err.is_bad_request(), "{err}");
            assert!(err.to_string().contains(field), "{err}");
        }
    }

    #[test]
    fn the_tool_answers_its_own_requests_and_refuses_nothing_else() {
        let response = AntennaTool
            .run(ToolRequest::Antenna(AntennaRequest::default()))
            .expect("its own request");
        assert_eq!(response.tool_id(), ANTENNA_TOOL_ID);
        assert_eq!(AntennaTool.descriptor().id, ANTENNA_TOOL_ID);
        assert!(!AntennaTool.descriptor().needs_hardware);
    }
}
