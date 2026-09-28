use std::f64::consts::{PI, TAU};

use num_complex::Complex;

use super::RadarDspError;
use crate::manifold::{Direction, LIGHT_SPEED_M_S, MAX_ELEMENTS, Vec3, steer};
use crate::special::norm_deg;

pub const MIN_SIGMA_DEG: f32 = 0.1;
pub const MAX_SIGMA_DEG: f32 = 90.0;
pub const MAX_AOA_POINTS: usize = 7200;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AoaEstimate {
    pub azimuth_deg: f32,
    pub quality: f32,
    pub sigma_deg: f32,
    pub mirror_deg: Option<f32>,
}

pub struct Beamscan {
    elements: usize,
    positions: [Vec3; MAX_ELEMENTS],
    wavelength_m: f64,
    step_deg: f64,
    mirror_axis_deg: Option<f32>,
    steering: Vec<Complex<f32>>,
    response: Vec<f32>,
}

impl Beamscan {
    pub fn new(
        positions_m: &[Vec3],
        wavelength_m: f64,
        grid_step_deg: f32,
        mirror_axis_deg: Option<f32>,
    ) -> Result<Self, RadarDspError> {
        let elements = positions_m.len();
        let step_deg = f64::from(grid_step_deg);
        let points = (360.0 / step_deg).round();
        let finite = positions_m
            .iter()
            .map(|p| p.x + p.y + p.z)
            .sum::<f64>()
            .is_finite();
        let valid = (1..=MAX_ELEMENTS).contains(&elements)
            && finite
            && wavelength_m.is_finite()
            && wavelength_m > 0.0
            && step_deg > 0.0
            && (1.0..=MAX_AOA_POINTS as f64).contains(&points)
            && mirror_axis_deg.is_none_or(f32::is_finite);
        if !valid {
            return Err(RadarDspError::Setting);
        }
        let points = points as usize;
        let mut positions = [Vec3::default(); MAX_ELEMENTS];
        positions[..elements].copy_from_slice(positions_m);
        let mut steering = vec![Complex::default(); points * elements];
        let freq_hz = LIGHT_SPEED_M_S / wavelength_m;
        let step_deg = 360.0 / points as f64;
        for (point, vector) in steering.chunks_exact_mut(elements).enumerate() {
            let direction = Direction::horizon(point as f64 * step_deg);
            steer(positions_m, freq_hz, direction, vector);
        }
        Ok(Self {
            elements,
            positions,
            wavelength_m,
            step_deg,
            mirror_axis_deg,
            steering,
            response: vec![0.0; points],
        })
    }

    #[must_use]
    pub const fn elements(&self) -> usize {
        self.elements
    }

    pub fn estimate(&mut self, snapshots: &[&[Complex<f32>]], snr: f32) -> Option<AoaEstimate> {
        let k = self.elements;
        if snapshots.is_empty() || snapshots.iter().any(|x| x.len() < k) {
            return None;
        }
        let trace: f64 = snapshots
            .iter()
            .flat_map(|x| &x[..k])
            .map(|value| f64::from(value.norm_sqr()))
            .sum();
        if !(trace > 0.0 && trace.is_finite()) {
            return None;
        }
        let scale = 1.0 / (k as f64 * trace);
        for (response, vector) in self.response.iter_mut().zip(self.steering.chunks_exact(k)) {
            let power: f64 = snapshots
                .iter()
                .map(|x| {
                    let beam: Complex<f32> = vector
                        .iter()
                        .zip(&x[..k])
                        .map(|(a, value)| a.conj() * value)
                        .sum();
                    f64::from(beam.norm_sqr())
                })
                .sum();
            *response = (power * scale) as f32;
        }
        let (peak, quality) = self.refine()?;
        let azimuth = norm_deg(peak * self.step_deg);
        Some(AoaEstimate {
            azimuth_deg: azimuth as f32,
            quality: quality.clamp(0.0, 1.0),
            sigma_deg: self.sigma_deg(azimuth, snr),
            mirror_deg: self
                .mirror_axis_deg
                .map(|axis| norm_deg(2.0 * f64::from(axis) - azimuth) as f32),
        })
    }

    fn refine(&self) -> Option<(f64, f32)> {
        let points = self.response.len();
        let (peak, &top) = self
            .response
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))?;
        if !top.is_finite() {
            return None;
        }
        let minus = self.response[(peak + points - 1) % points];
        let plus = self.response[(peak + 1) % points];
        let curvature = minus - 2.0 * top + plus;
        if points < 3 || curvature >= 0.0 {
            return Some((peak as f64, top));
        }
        let delta = ((minus - plus) / (2.0 * curvature)).clamp(-0.5, 0.5);
        let value = top - 0.25 * (minus - plus) * delta;
        Some((peak as f64 + f64::from(delta), value))
    }

    fn sigma_deg(&self, azimuth_deg: f64, snr: f32) -> f32 {
        let (sin, cos) = azimuth_deg.to_radians().sin_cos();
        let positions = &self.positions[..self.elements];
        let spread: Spread = positions
            .iter()
            .map(|p| p.x * cos - p.y * sin)
            .fold(Spread::default(), Spread::push);
        let aperture = spread.rms();
        let wavenumber = TAU / self.wavelength_m;
        let denominator =
            wavenumber * aperture * (2.0 * self.elements as f64 * f64::from(snr)).sqrt();
        if !(denominator > 0.0 && denominator.is_finite()) {
            return MAX_SIGMA_DEG;
        }
        ((180.0 / PI / denominator) as f32).clamp(MIN_SIGMA_DEG, MAX_SIGMA_DEG)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Spread {
    count: f64,
    sum: f64,
    squares: f64,
}

impl Spread {
    fn push(self, value: f64) -> Self {
        Self {
            count: self.count + 1.0,
            sum: self.sum + value,
            squares: self.squares + value * value,
        }
    }

    fn rms(self) -> f64 {
        if self.count == 0.0 {
            return 0.0;
        }
        let mean = self.sum / self.count;
        (self.squares / self.count - mean * mean).max(0.0).sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifold::{Geometry, Winding};
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};
    use crate::special::wrap_deg;

    const FREQ: f64 = 100e6;

    fn lambda() -> f64 {
        LIGHT_SPEED_M_S / FREQ
    }

    fn snapshots_from(
        geometry: Geometry,
        azimuth: f64,
        noise_db: f32,
        seed: u64,
    ) -> Vec<Vec<Complex<f32>>> {
        let mut scene = ArrayScene::new(geometry, FREQ, 1e5)
            .with_source(SceneSource::new(
                Direction::horizon(azimuth),
                0.0,
                SceneSignal::Noise {
                    offset_hz: 0.0,
                    bandwidth_hz: 2e4,
                },
            ))
            .with_noise_db(noise_db)
            .with_seed(seed);
        let lanes = scene.render(400).unwrap();
        [50, 130, 210, 290, 370]
            .iter()
            .map(|&n| lanes.iter().map(|lane| lane[n]).collect())
            .collect()
    }

    fn estimate(
        scan: &mut Beamscan,
        snapshots: &[Vec<Complex<f32>>],
        snr: f32,
    ) -> Option<AoaEstimate> {
        let views: Vec<&[Complex<f32>]> = snapshots.iter().map(Vec::as_slice).collect();
        scan.estimate(&views, snr)
    }

    #[test]
    fn a_plane_wave_is_found_on_a_uca() {
        let geometry = Geometry::uca(0.4 * lambda(), 4, 0.0, Winding::Clockwise).unwrap();
        let mut scan = Beamscan::new(geometry.positions(), lambda(), 0.5, None).unwrap();
        for step in 0..24 {
            let azimuth = f64::from(step) * 15.0;
            let snapshots =
                snapshots_from(geometry.clone(), azimuth, -20.0, u64::from(step as u32) + 1);
            let found = estimate(&mut scan, &snapshots, 100.0).unwrap();
            let error = wrap_deg(f64::from(found.azimuth_deg) - azimuth).abs();
            assert!(error < 2.0, "{azimuth}: {}", found.azimuth_deg);
            assert!(found.quality > 0.9);
            assert!(found.mirror_deg.is_none());
        }
    }

    #[test]
    fn a_line_array_reports_its_mirror() {
        let geometry = Geometry::ula(0.5 * lambda(), 4, 90.0).unwrap();
        let axis = geometry.line_axis_deg().unwrap() as f32;
        let mut scan = Beamscan::new(geometry.positions(), lambda(), 0.5, Some(axis)).unwrap();
        let snapshots = snapshots_from(geometry, 30.0, -25.0, 3);
        let found = estimate(&mut scan, &snapshots, 300.0).unwrap();
        let mirror = found.mirror_deg.unwrap();
        let pair = [found.azimuth_deg, mirror];
        assert!(pair.iter().any(|a| (a - 30.0).abs() < 2.0), "{pair:?}");
        assert!(pair.iter().any(|a| (a - 150.0).abs() < 2.0), "{pair:?}");
    }

    #[test]
    fn incoherent_lanes_have_low_quality() {
        let geometry = Geometry::uca(0.4 * lambda(), 4, 0.0, Winding::Clockwise).unwrap();
        let mut scan = Beamscan::new(geometry.positions(), lambda(), 0.5, None).unwrap();
        let mut quiet = ArrayScene::new(geometry, FREQ, 1e5)
            .with_noise_db(0.0)
            .with_seed(9);
        let lanes = quiet.render(2000).unwrap();
        let mut total = 0.0f32;
        for trial in 0..50 {
            let snapshots: Vec<Vec<Complex<f32>>> = (0..5)
                .map(|k| lanes.iter().map(|lane| lane[trial * 37 + k * 7]).collect())
                .collect();
            total += estimate(&mut scan, &snapshots, 10.0).unwrap().quality;
        }
        let mean = total / 50.0;
        assert!(mean < 0.5, "{mean}");
    }

    #[test]
    fn sigma_shrinks_with_snr() {
        let geometry = Geometry::uca(0.4 * lambda(), 4, 0.0, Winding::Clockwise).unwrap();
        let mut scan = Beamscan::new(geometry.positions(), lambda(), 0.5, None).unwrap();
        let snapshots = snapshots_from(geometry, 80.0, -20.0, 4);
        let weak = estimate(&mut scan, &snapshots, 10.0).unwrap().sigma_deg;
        let strong = estimate(&mut scan, &snapshots, 1000.0).unwrap().sigma_deg;
        assert!(strong < weak / 5.0, "{strong} {weak}");
        assert!((MIN_SIGMA_DEG..=MAX_SIGMA_DEG).contains(&strong));
    }

    #[test]
    fn silence_and_bad_input_give_nothing() {
        let geometry = Geometry::uca(0.4 * lambda(), 4, 0.0, Winding::Clockwise).unwrap();
        let mut scan = Beamscan::new(geometry.positions(), lambda(), 0.5, None).unwrap();
        let zeros = [Complex::default(); 4];
        assert!(scan.estimate(&[&zeros], 10.0).is_none());
        let nan = [Complex::new(f32::NAN, 0.0); 4];
        assert!(scan.estimate(&[&nan], 10.0).is_none());
        assert!(scan.estimate(&[&zeros[..2]], 10.0).is_none());
        assert!(scan.estimate(&[], 10.0).is_none());
        assert!(Beamscan::new(&[], lambda(), 0.5, None).is_err());
        assert!(Beamscan::new(geometry.positions(), 0.0, 0.5, None).is_err());
        assert!(Beamscan::new(geometry.positions(), lambda(), 0.0, None).is_err());
    }
}
