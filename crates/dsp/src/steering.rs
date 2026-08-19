use std::f64::consts::TAU;

use num_complex::Complex;

pub const LIGHT_SPEED_M_S: f64 = 299_792_458.0;

/// Element positions in metres on the ground plane: `x` east, `y` north.
///
/// Bearings everywhere in the project are compass bearings — zero due north, increasing
/// clockwise — so the projection of a position onto an arrival direction is
/// `x·sin θ + y·cos θ` and nothing has to remember a second convention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Element {
    pub x_m: f64,
    pub y_m: f64,
}

impl Element {
    #[must_use]
    pub const fn new(x_m: f64, y_m: f64) -> Self {
        Self { x_m, y_m }
    }

    #[must_use]
    pub fn projected(&self, bearing_deg: f64) -> f64 {
        let bearing = bearing_deg.to_radians();
        self.x_m * bearing.sin() + self.y_m * bearing.cos()
    }
}

/// A uniform circular array with element zero due north, matching how the antenna tool lays a
/// radial fan out.
#[must_use]
pub fn uca(radius_m: f64, count: usize) -> Vec<Element> {
    (0..count)
        .map(|index| {
            let angle = TAU * index as f64 / count.max(1) as f64;
            Element::new(radius_m * angle.sin(), radius_m * angle.cos())
        })
        .collect()
}

/// A uniform linear array laid out east–west and centred on the origin.
#[must_use]
pub fn ula(spacing_m: f64, count: usize) -> Vec<Element> {
    let centre = (count.max(1) as f64 - 1.0) / 2.0;
    (0..count)
        .map(|index| Element::new((index as f64 - centre) * spacing_m, 0.0))
        .collect()
}

/// Every steering vector the direction finder will test, built once for a tuning.
pub struct SteeringGrid {
    elements: usize,
    step_deg: f64,
    points: usize,
    vectors: Vec<Complex<f32>>,
}

impl SteeringGrid {
    #[must_use]
    pub fn new(elements: &[Element], freq_hz: f64, step_deg: f64) -> Self {
        let step_deg = if step_deg > 0.0 { step_deg } else { 1.0 };
        let points = (360.0 / step_deg).round().max(1.0) as usize;
        let wavelength_m = if freq_hz > 0.0 {
            LIGHT_SPEED_M_S / freq_hz
        } else {
            f64::INFINITY
        };
        let mut vectors = Vec::with_capacity(points * elements.len());
        for point in 0..points {
            let bearing = point as f64 * step_deg;
            for element in elements {
                let phase = TAU * element.projected(bearing) / wavelength_m;
                vectors.push(Complex::from_polar(1.0, phase as f32));
            }
        }
        Self {
            elements: elements.len(),
            step_deg,
            points,
            vectors,
        }
    }

    #[must_use]
    pub const fn points(&self) -> usize {
        self.points
    }

    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    #[must_use]
    pub const fn step_deg(&self) -> f64 {
        self.step_deg
    }

    #[must_use]
    pub fn bearing_deg(&self, point: usize) -> f64 {
        point as f64 * self.step_deg
    }

    #[must_use]
    pub fn vector(&self, point: usize) -> &[Complex<f32>] {
        let start = point * self.elements;
        &self.vectors[start..start + self.elements]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_circular_array_starts_due_north_and_runs_clockwise() {
        let elements = uca(1.0, 4);
        assert!((elements[0].x_m).abs() < 1e-9 && (elements[0].y_m - 1.0).abs() < 1e-9);
        assert!((elements[1].x_m - 1.0).abs() < 1e-9 && elements[1].y_m.abs() < 1e-9);
        assert!((elements[2].y_m + 1.0).abs() < 1e-9);
        assert!((elements[3].x_m + 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_linear_array_is_centred_on_the_origin() {
        let elements = ula(0.5, 4);
        let sum: f64 = elements.iter().map(|e| e.x_m).sum();
        assert!(sum.abs() < 1e-9);
        assert!((elements[1].x_m - elements[0].x_m - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_element_pointing_at_the_source_carries_the_largest_projection() {
        let elements = uca(1.0, 4);
        for (index, bearing) in [(0usize, 0.0), (1, 90.0), (2, 180.0), (3, 270.0)] {
            let best = (0..4)
                .max_by(|&a, &b| {
                    elements[a]
                        .projected(bearing)
                        .total_cmp(&elements[b].projected(bearing))
                })
                .expect("four elements");
            assert_eq!(best, index, "bearing {bearing}");
        }
    }

    #[test]
    fn a_grid_covers_the_circle_once_at_its_step() {
        let grid = SteeringGrid::new(&uca(0.3, 4), 300e6, 1.0);
        assert_eq!(grid.points(), 360);
        assert_eq!(grid.elements(), 4);
        assert!((grid.bearing_deg(137) - 137.0).abs() < 1e-9);
        for point in 0..grid.points() {
            for value in grid.vector(point) {
                assert!((value.norm() - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn a_zero_extent_array_steers_nowhere() {
        let grid = SteeringGrid::new(&uca(0.0, 4), 300e6, 5.0);
        assert_eq!(grid.points(), 72);
        for point in 0..grid.points() {
            for value in grid.vector(point) {
                assert!((value - Complex::new(1.0, 0.0)).norm() < 1e-6);
            }
        }
    }
}
