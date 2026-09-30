use num_complex::Complex;

use super::{Direction, MAX_ELEMENTS, ManifoldError, widen};
use crate::special::norm_deg;

const STEP_TOLERANCE: f64 = 1e-9;
const COVER_TOLERANCE: f64 = 1e-9;

#[derive(Clone, Debug, PartialEq)]
pub struct ManifoldTable {
    elements: usize,
    freqs_hz: Vec<f64>,
    azimuth_step_deg: f64,
    elevations_deg: Vec<f64>,
    data: Vec<Complex<f32>>,
}

#[derive(Clone, Copy)]
struct Bracket {
    low: usize,
    high: usize,
    weight: f64,
}

impl Bracket {
    const fn single(index: usize) -> Self {
        Self {
            low: index,
            high: index,
            weight: 0.0,
        }
    }

    fn corners(self) -> [(usize, f64); 2] {
        [(self.low, 1.0 - self.weight), (self.high, self.weight)]
    }
}

impl ManifoldTable {
    pub fn new(
        elements: usize,
        freqs_hz: Vec<f64>,
        azimuth_step_deg: f64,
        elevations_deg: Vec<f64>,
        data: Vec<Complex<f32>>,
    ) -> Result<Self, ManifoldError> {
        let azimuths = azimuth_count(azimuth_step_deg).ok_or(ManifoldError::Table)?;
        let valid = (2..=MAX_ELEMENTS).contains(&elements)
            && ascending(&freqs_hz, |f| f > 0.0)
            && ascending(&elevations_deg, |e| (0.0..=90.0).contains(&e))
            && elevations_deg.first() == Some(&0.0)
            && data.len() == freqs_hz.len() * elevations_deg.len() * azimuths * elements
            && data.iter().all(|value| value.is_finite());
        if !valid {
            return Err(ManifoldError::Table);
        }
        Ok(Self {
            elements,
            freqs_hz,
            azimuth_step_deg,
            elevations_deg,
            data,
        })
    }

    pub fn select(&self, elements: &[usize]) -> Result<Self, ManifoldError> {
        if elements.iter().any(|&element| element >= self.elements) {
            return Err(ManifoldError::Table);
        }
        let data = self
            .data
            .chunks_exact(self.elements)
            .flat_map(|row| elements.iter().map(|&element| row[element]))
            .collect();
        Self::new(
            elements.len(),
            self.freqs_hz.clone(),
            self.azimuth_step_deg,
            self.elevations_deg.clone(),
            data,
        )
    }

    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    #[must_use]
    pub fn freqs_hz(&self) -> &[f64] {
        &self.freqs_hz
    }

    #[must_use]
    pub const fn azimuth_step_deg(&self) -> f64 {
        self.azimuth_step_deg
    }

    #[must_use]
    pub fn elevations_deg(&self) -> &[f64] {
        &self.elevations_deg
    }

    #[must_use]
    pub fn data(&self) -> &[Complex<f32>] {
        &self.data
    }

    #[must_use]
    pub fn covers(&self, freq_hz: f64) -> bool {
        match (self.freqs_hz.first(), self.freqs_hz.last()) {
            (Some(&low), Some(&high)) => {
                let slack = COVER_TOLERANCE * freq_hz.abs();
                freq_hz.is_finite() && freq_hz >= low - slack && freq_hz <= high + slack
            }
            _ => false,
        }
    }

    pub fn response(
        &self,
        freq_hz: f64,
        direction: Direction,
        out: &mut [Complex<f32>],
    ) -> Result<(), ManifoldError> {
        if !self.covers(freq_hz) {
            return Err(ManifoldError::OutOfTable(freq_hz));
        }
        let n = self.elements;
        let out = out.get_mut(..n).ok_or(ManifoldError::Table)?;
        let freq = bracket(&self.freqs_hz, freq_hz);
        let elevation = bracket(&self.elevations_deg, direction.elevation_deg);
        let azimuth = self.azimuth_bracket(direction.azimuth_deg);
        let mut sum = [Complex::new(0.0f64, 0.0); MAX_ELEMENTS];
        let mut reference: Option<&[Complex<f32>]> = None;
        for (f, wf) in freq.corners() {
            for (e, we) in elevation.corners() {
                for (a, wa) in azimuth.corners() {
                    let weight = wf * we * wa;
                    if weight == 0.0 {
                        continue;
                    }
                    let corner = self.vector(f, e, a);
                    let first = *reference.get_or_insert(corner);
                    let align = alignment(first, corner) * weight;
                    for (total, value) in sum.iter_mut().zip(corner) {
                        *total += widen(*value) * align;
                    }
                }
            }
        }
        let power: f64 = sum[..n].iter().map(Complex::norm_sqr).sum();
        let scale = if power > 0.0 {
            (n as f64 / power).sqrt()
        } else {
            0.0
        };
        for (value, total) in out.iter_mut().zip(&sum) {
            let scaled = total * scale;
            *value = Complex::new(scaled.re as f32, scaled.im as f32);
        }
        Ok(())
    }

    fn azimuths(&self) -> usize {
        azimuth_count(self.azimuth_step_deg).unwrap_or(1)
    }

    fn azimuth_bracket(&self, azimuth_deg: f64) -> Bracket {
        let count = self.azimuths();
        let position = norm_deg(azimuth_deg) / self.azimuth_step_deg;
        let low = (position.floor() as usize).min(count - 1);
        Bracket {
            low,
            high: (low + 1) % count,
            weight: (position - low as f64).clamp(0.0, 1.0),
        }
    }

    fn vector(&self, freq: usize, elevation: usize, azimuth: usize) -> &[Complex<f32>] {
        let row = (freq * self.elevations_deg.len() + elevation) * self.azimuths() + azimuth;
        &self.data[row * self.elements..(row + 1) * self.elements]
    }
}

fn azimuth_count(step_deg: f64) -> Option<usize> {
    if !(step_deg.is_finite() && step_deg > 0.0 && step_deg <= 360.0) {
        return None;
    }
    let count = 360.0 / step_deg;
    ((count - count.round()).abs() <= STEP_TOLERANCE * count).then_some(count.round() as usize)
}

fn ascending(values: &[f64], valid: impl Fn(f64) -> bool) -> bool {
    !values.is_empty()
        && values.iter().all(|&v| v.is_finite() && valid(v))
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn bracket(axis: &[f64], value: f64) -> Bracket {
    let last = axis.len() - 1;
    if last == 0 || value <= axis[0] {
        return Bracket::single(0);
    }
    if value >= axis[last] {
        return Bracket::single(last);
    }
    let high = axis.partition_point(|&v| v <= value).min(last);
    let low = high - 1;
    Bracket {
        low,
        high,
        weight: (value - axis[low]) / (axis[high] - axis[low]),
    }
}

fn alignment(reference: &[Complex<f32>], corner: &[Complex<f32>]) -> Complex<f64> {
    let inner: Complex<f64> = reference
        .iter()
        .zip(corner)
        .map(|(r, c)| widen(r.conj() * c))
        .sum();
    Complex::from_polar(1.0, -inner.arg())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::tests::ideal_table;
    use crate::manifold::{Geometry, Winding, steer};

    #[test]
    fn table_interpolation_reproduces_the_ideal_manifold() {
        let geometry = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
        let table = ideal_table(&geometry, &[420e6, 440e6], 5.0);
        let mid = 430e6;
        let mut from_table = [Complex::new(0.0f32, 0.0); 5];
        let mut ideal = [Complex::new(0.0f32, 0.0); 5];
        for azimuth in [12.5, 181.0, 357.5] {
            let direction = Direction::horizon(azimuth);
            table.response(mid, direction, &mut from_table).unwrap();
            steer(geometry.positions(), mid, direction, &mut ideal);
            let matched: Complex<f32> = ideal
                .iter()
                .zip(&from_table)
                .map(|(a, b)| a.conj() * b)
                .sum();
            assert!(
                matched.norm() / 5.0 > 0.999,
                "{azimuth}: {}",
                matched.norm()
            );
            let power: f32 = from_table.iter().map(Complex::norm_sqr).sum();
            assert!((power - 5.0).abs() < 1e-4);
        }
    }

    #[test]
    fn table_outside_its_frequency_range_is_refused() {
        let geometry = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
        let table = ideal_table(&geometry, &[420e6, 440e6], 5.0);
        let mut out = [Complex::new(0.0f32, 0.0); 5];
        assert_eq!(
            table.response(441e6, Direction::horizon(0.0), &mut out),
            Err(ManifoldError::OutOfTable(441e6))
        );
        assert!(!table.covers(419e6));
        assert!(table.covers(420e6));
    }

    #[test]
    fn a_single_elevation_row_serves_every_elevation() {
        let geometry = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
        let table = ideal_table(&geometry, &[433.92e6], 5.0);
        let mut level = [Complex::new(0.0f32, 0.0); 5];
        let mut raised = [Complex::new(0.0f32, 0.0); 5];
        table
            .response(433.92e6, Direction::new(40.0, 0.0), &mut level)
            .unwrap();
        table
            .response(433.92e6, Direction::new(40.0, 30.0), &mut raised)
            .unwrap();
        assert_eq!(level, raised);
    }

    #[test]
    fn a_selection_keeps_the_rows_of_its_elements() {
        let geometry = Geometry::uca(0.35, 5, 0.0, Winding::Clockwise).unwrap();
        let table = ideal_table(&geometry, &[420e6, 440e6], 5.0);
        let picked = [1, 3, 4];
        let selected = table.select(&picked).unwrap();
        assert_eq!(selected.elements(), 3);
        assert_eq!(selected.freqs_hz(), table.freqs_hz());
        let mut all = [Complex::new(0.0f32, 0.0); 5];
        let mut rows = [Complex::new(0.0f32, 0.0); 3];
        for (freq, azimuth) in [(420e6, 60.0), (440e6, 215.0)] {
            let direction = Direction::horizon(azimuth);
            table.response(freq, direction, &mut all).unwrap();
            selected.response(freq, direction, &mut rows).unwrap();
            for (&element, row) in picked.iter().zip(&rows) {
                assert!((all[element] - row).norm() < 1e-5, "{azimuth}: {element}");
            }
        }
        assert_eq!(table.select(&[0, 5]).unwrap_err(), ManifoldError::Table);
        assert_eq!(table.select(&[2]).unwrap_err(), ManifoldError::Table);
    }

    #[test]
    fn inconsistent_tables_are_refused() {
        let data = vec![Complex::new(1.0f32, 0.0); 2 * 36 * 3];
        assert!(ManifoldTable::new(3, vec![1e8, 2e8], 10.0, vec![0.0], data.clone()).is_ok());
        let cases = [
            ManifoldTable::new(3, vec![2e8, 1e8], 10.0, vec![0.0], data.clone()),
            ManifoldTable::new(3, vec![1e8, 2e8], 7.0, vec![0.0], data.clone()),
            ManifoldTable::new(3, vec![1e8, 2e8], 10.0, vec![5.0], data.clone()),
            ManifoldTable::new(3, vec![1e8, 2e8], 10.0, vec![0.0, 10.0], data.clone()),
            ManifoldTable::new(1, vec![1e8, 2e8], 10.0, vec![0.0], data),
        ];
        for case in cases {
            assert_eq!(case.unwrap_err(), ManifoldError::Table);
        }
    }
}
