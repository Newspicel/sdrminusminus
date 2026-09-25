use sdrmm_wire::{
    coherent::{ArrayGeometry, CalState},
    device::Coherence,
};

pub const COMPASS_MARKS: [(f64, &str); 8] = [
    (0.0, "N"),
    (45.0, "NE"),
    (90.0, "E"),
    (135.0, "SE"),
    (180.0, "S"),
    (225.0, "SW"),
    (270.0, "W"),
    (315.0, "NW"),
];

#[must_use]
pub fn polar_point(bearing_deg: f64, radius: f64, centre: f64) -> (f64, f64) {
    let angle = (bearing_deg - 90.0).to_radians();
    (centre + radius * angle.cos(), centre + radius * angle.sin())
}

#[must_use]
pub fn spectrum_points(spectrum: &[u8], centre: f64, inner: f64, outer: f64) -> Vec<(f64, f64)> {
    let points = spectrum.len() as f64;
    spectrum
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let radius = inner + (outer - inner) * f64::from(*level) / 255.0;
            polar_point(index as f64 * 360.0 / points, radius, centre)
        })
        .collect()
}

#[must_use]
pub fn bearing_label(bearing_deg: f32) -> String {
    format!("{bearing_deg:05.1}\u{b0}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalVerdict {
    Injecting,
    PhaseUnknown,
    Solving,
    Solved,
}

impl CalVerdict {
    #[must_use]
    pub fn of(cal: Option<&CalState>) -> Self {
        match cal {
            Some(cal) if cal.reference_on => Self::Injecting,
            Some(cal) if !cal.phase_unknown && cal.solved => Self::Solved,
            Some(cal) if !cal.phase_unknown => Self::Solving,
            _ => Self::PhaseUnknown,
        }
    }

    #[must_use]
    pub fn text(self) -> &'static str {
        match self {
            Self::Injecting => "noise source in: paused",
            Self::PhaseUnknown => "phase unknown: calibrate",
            Self::Solving => "calibrating",
            Self::Solved => "calibrated",
        }
    }

    #[must_use]
    pub fn trusts_bearing(self) -> bool {
        matches!(self, Self::Solving | Self::Solved)
    }
}

#[must_use]
pub fn tier_label(cal: Option<&CalState>) -> &'static str {
    match cal.map(|cal| cal.tier) {
        Some(Coherence::PhaseCoherent) => "shared LO",
        Some(Coherence::TimeSync) => "shared clock",
        _ => "not coherent",
    }
}

#[must_use]
pub fn lane_quality_percent(quality: f32) -> u32 {
    (quality.clamp(0.0, 1.0) * 100.0).round() as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Line,
}

#[must_use]
pub fn shape_of(geometry: &ArrayGeometry) -> Option<Shape> {
    match geometry {
        ArrayGeometry::Uca { .. } => Some(Shape::Circle),
        ArrayGeometry::Ula { .. } => Some(Shape::Line),
        ArrayGeometry::Explicit { .. } => None,
    }
}

#[must_use]
pub fn geometry_of(shape: Shape, current: &ArrayGeometry) -> ArrayGeometry {
    let count = current.count();
    match shape {
        Shape::Line => ArrayGeometry::Ula {
            spacing_m: 0.5,
            count,
        },
        Shape::Circle => ArrayGeometry::Uca {
            radius_m: 0.35,
            count,
        },
    }
}

#[must_use]
pub fn with_count(geometry: &ArrayGeometry, count: u32) -> ArrayGeometry {
    match geometry {
        ArrayGeometry::Uca { radius_m, .. } => ArrayGeometry::Uca {
            radius_m: *radius_m,
            count,
        },
        ArrayGeometry::Ula { spacing_m, .. } => ArrayGeometry::Ula {
            spacing_m: *spacing_m,
            count,
        },
        explicit @ ArrayGeometry::Explicit { .. } => explicit.clone(),
    }
}

#[must_use]
pub fn with_extent(geometry: &ArrayGeometry, metres: f64) -> ArrayGeometry {
    match geometry {
        ArrayGeometry::Uca { count, .. } => ArrayGeometry::Uca {
            radius_m: metres,
            count: *count,
        },
        ArrayGeometry::Ula { count, .. } => ArrayGeometry::Ula {
            spacing_m: metres,
            count: *count,
        },
        explicit @ ArrayGeometry::Explicit { .. } => explicit.clone(),
    }
}

#[must_use]
pub fn extent_of(geometry: &ArrayGeometry) -> Option<f64> {
    match geometry {
        ArrayGeometry::Uca { radius_m, .. } => Some(*radius_m),
        ArrayGeometry::Ula { spacing_m, .. } => Some(*spacing_m),
        ArrayGeometry::Explicit { .. } => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beam {
    Follow,
    Fixed,
}

#[must_use]
pub fn beam_mode(beam_bearing_deg: Option<f64>) -> Beam {
    if beam_bearing_deg.is_some() {
        Beam::Fixed
    } else {
        Beam::Follow
    }
}

#[must_use]
pub fn beam_azimuth(mode: Beam, bearing_deg: Option<f64>) -> Option<f64> {
    match mode {
        Beam::Follow => None,
        Beam::Fixed => Some(bearing_deg.unwrap_or(0.0).round().rem_euclid(360.0)),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::coherent::{ArrayElement, DfParams};

    use super::*;

    fn cal(phase_unknown: bool, solved: bool, reference_on: bool, tier: Coherence) -> CalState {
        CalState {
            tier,
            lanes: Vec::new(),
            phase_unknown,
            solved,
            reference_on,
        }
    }

    fn solved() -> CalState {
        cal(false, true, false, Coherence::PhaseCoherent)
    }

    #[test]
    fn keeps_the_element_count_when_the_shape_changes() {
        let line = geometry_of(
            Shape::Line,
            &ArrayGeometry::Uca {
                radius_m: 0.35,
                count: 6,
            },
        );
        assert_eq!(
            line,
            ArrayGeometry::Ula {
                spacing_m: 0.5,
                count: 6
            }
        );
        assert_eq!(geometry_of(Shape::Circle, &line).count(), 6);
    }

    #[test]
    fn leaves_explicit_positions_alone_when_the_count_is_edited() {
        let explicit = ArrayGeometry::Explicit {
            positions: vec![ArrayElement { x_m: 0.0, y_m: 0.0 }],
        };
        assert_eq!(with_count(&explicit, 8), explicit);
        assert_eq!(with_extent(&explicit, 2.0), explicit);
        assert_eq!(
            with_count(
                &ArrayGeometry::Uca {
                    radius_m: 1.0,
                    count: 2
                },
                8
            ),
            ArrayGeometry::Uca {
                radius_m: 1.0,
                count: 8
            }
        );
        assert_eq!(shape_of(&explicit), None);
        assert_eq!(extent_of(&explicit), None);
    }

    #[test]
    fn puts_north_at_the_top_and_runs_clockwise() {
        let close =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
        assert!(close(polar_point(0.0, 50.0, 100.0), (100.0, 50.0)));
        assert!(close(polar_point(90.0, 50.0, 100.0), (150.0, 100.0)));
        assert!((polar_point(180.0, 50.0, 100.0).1 - 150.0).abs() < 1e-6);
    }

    #[test]
    fn a_pseudospectrum_is_one_point_per_sample_between_the_rings() {
        let points = spectrum_points(&[0, 128, 255, 128], 100.0, 20.0, 80.0);
        assert_eq!(points.len(), 4);
        assert!((points[0].1 - 80.0).abs() < 1e-6);
        assert!((points[2].1 - 180.0).abs() < 1e-6);
        assert!(spectrum_points(&[], 100.0, 20.0, 80.0).is_empty());
    }

    #[test]
    fn refuses_to_call_an_unknown_phase_anything_else() {
        assert_eq!(CalVerdict::of(None), CalVerdict::PhaseUnknown);
        let unknown = cal(true, true, false, Coherence::PhaseCoherent);
        assert_eq!(CalVerdict::of(Some(&unknown)), CalVerdict::PhaseUnknown);
        let solving = cal(false, false, false, Coherence::PhaseCoherent);
        assert_eq!(CalVerdict::of(Some(&solving)), CalVerdict::Solving);
        let injecting = cal(false, true, true, Coherence::PhaseCoherent);
        assert_eq!(CalVerdict::of(Some(&injecting)), CalVerdict::Injecting);
        let both = cal(true, true, true, Coherence::PhaseCoherent);
        assert_eq!(CalVerdict::of(Some(&both)), CalVerdict::Injecting);
        assert_eq!(CalVerdict::of(Some(&solved())), CalVerdict::Solved);
        assert!(CalVerdict::PhaseUnknown.text().contains("calibrate"));
        assert!(CalVerdict::Injecting.text().contains("paused"));
        assert!(!CalVerdict::Injecting.trusts_bearing());
        assert!(CalVerdict::Solving.trusts_bearing());
    }

    #[test]
    fn says_what_the_hardware_actually_shares() {
        assert_eq!(tier_label(Some(&solved())), "shared LO");
        let clock = cal(false, true, false, Coherence::TimeSync);
        assert_eq!(tier_label(Some(&clock)), "shared clock");
        let none = cal(false, true, false, Coherence::None);
        assert_eq!(tier_label(Some(&none)), "not coherent");
        assert_eq!(tier_label(None), "not coherent");
    }

    #[test]
    fn pads_a_bearing_so_the_readout_never_jumps_width() {
        assert_eq!(bearing_label(7.24), "007.2\u{b0}");
        assert_eq!(bearing_label(137.5), "137.5\u{b0}");
    }

    #[test]
    fn clamps_a_lane_quality_to_a_bar_width() {
        assert_eq!(lane_quality_percent(-1.0), 0);
        assert_eq!(lane_quality_percent(0.5), 50);
        assert_eq!(lane_quality_percent(3.0), 100);
    }

    #[test]
    fn the_default_array_is_one_the_server_accepts() {
        let params = DfParams::default();
        assert!(params.geometry.count() >= 2);
        assert!(params.sources < params.geometry.count());
        assert!(params.report_ms >= 100);
    }

    #[test]
    fn follows_the_bearing_until_the_operator_pins_it() {
        assert_eq!(beam_mode(None), Beam::Follow);
        assert_eq!(beam_mode(Some(0.0)), Beam::Fixed);
        assert_eq!(beam_azimuth(Beam::Follow, Some(137.0)), None);
    }

    #[test]
    fn pins_the_beam_where_the_array_is_already_pointing() {
        assert_eq!(beam_azimuth(Beam::Fixed, Some(137.4)), Some(137.0));
        assert_eq!(beam_azimuth(Beam::Fixed, Some(359.7)), Some(0.0));
        assert_eq!(beam_azimuth(Beam::Fixed, None), Some(0.0));
    }
}
