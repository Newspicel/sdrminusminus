use std::f64::consts::PI;

use num_complex::Complex;

use super::RadarDspError;
use crate::manifold::{Direction, MAX_ELEMENTS, Manifold};
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
    step_deg: f64,
    mirror_axis_deg: Option<f32>,
    steering: Vec<Complex<f32>>,
    spreads: Vec<f64>,
    response: Vec<f32>,
}

impl Beamscan {
    pub fn new(
        manifold: &Manifold,
        freq_hz: f64,
        grid_step_deg: f32,
        mirror_axis_deg: Option<f32>,
    ) -> Result<Self, RadarDspError> {
        let elements = manifold.len();
        let step_deg = f64::from(grid_step_deg);
        let points = (360.0 / step_deg).round();
        let valid = (1..=MAX_ELEMENTS).contains(&elements)
            && freq_hz.is_finite()
            && freq_hz > 0.0
            && step_deg > 0.0
            && (1.0..=MAX_AOA_POINTS as f64).contains(&points)
            && mirror_axis_deg.is_none_or(f32::is_finite);
        if !valid {
            return Err(RadarDspError::Setting);
        }
        let points = points as usize;
        let step_deg = 360.0 / points as f64;
        let mut steering = vec![Complex::default(); points * elements];
        let mut spreads = vec![0.0; points];
        let mut rates = [0.0f64; MAX_ELEMENTS];
        for (point, (vector, spread)) in steering
            .chunks_exact_mut(elements)
            .zip(spreads.iter_mut())
            .enumerate()
        {
            let direction = Direction::horizon(point as f64 * step_deg);
            manifold.steer(freq_hz, direction, vector);
            manifold.phase_rates(freq_hz, direction, &mut rates[..elements]);
            *spread = rates[..elements]
                .iter()
                .copied()
                .fold(Spread::default(), Spread::push)
                .rms();
        }
        Ok(Self {
            elements,
            step_deg,
            mirror_axis_deg,
            steering,
            spreads,
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
        let points = self.spreads.len();
        let nearest = (azimuth_deg / self.step_deg).round() as usize % points;
        let denominator =
            self.spreads[nearest] * (2.0 * self.elements as f64 * f64::from(snr)).sqrt();
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
    use std::f64::consts::TAU;
    use std::sync::Arc;

    use super::*;
    use crate::manifold::{Geometry, LIGHT_SPEED_M_S, Winding};
    use crate::scene::{ArrayScene, SceneSignal, SceneSource};
    use crate::special::wrap_deg;

    const FREQ: f64 = 100e6;
    const SKEW_DEG: [f64; 4] = [0.0, 25.0, -20.0, 15.0];

    fn lambda() -> f64 {
        LIGHT_SPEED_M_S / FREQ
    }

    fn uca() -> Geometry {
        Geometry::uca(0.4 * lambda(), 4, 0.0, Winding::Clockwise).unwrap()
    }

    fn ideal_scan(geometry: &Geometry, mirror_axis_deg: Option<f32>) -> Beamscan {
        let manifold = Manifold::ideal(geometry.clone());
        Beamscan::new(&manifold, FREQ, 0.5, mirror_axis_deg).unwrap()
    }

    fn scene_of(geometry: Geometry, azimuth: f64, noise_db: f32, seed: u64) -> ArrayScene {
        ArrayScene::new(geometry, FREQ, 1e5)
            .with_source(SceneSource::new(
                Direction::horizon(azimuth),
                0.0,
                SceneSignal::Noise {
                    offset_hz: 0.0,
                    bandwidth_hz: 2e4,
                },
            ))
            .with_noise_db(noise_db)
            .with_seed(seed)
    }

    fn snapshots_of(scene: &mut ArrayScene) -> Vec<Vec<Complex<f32>>> {
        let lanes = scene.render(400).unwrap();
        [50, 130, 210, 290, 370]
            .iter()
            .map(|&n| lanes.iter().map(|lane| lane[n]).collect())
            .collect()
    }

    fn snapshots_from(
        geometry: Geometry,
        azimuth: f64,
        noise_db: f32,
        seed: u64,
    ) -> Vec<Vec<Complex<f32>>> {
        snapshots_of(&mut scene_of(geometry, azimuth, noise_db, seed))
    }

    fn estimate(
        scan: &mut Beamscan,
        snapshots: &[Vec<Complex<f32>>],
        snr: f32,
    ) -> Option<AoaEstimate> {
        let views: Vec<&[Complex<f32>]> = snapshots.iter().map(Vec::as_slice).collect();
        scan.estimate(&views, snr)
    }

    fn skewed(element: usize, _azimuth_deg: f64) -> Complex<f32> {
        let phase = SKEW_DEG[element % SKEW_DEG.len()].to_radians();
        Complex::from_polar(1.0, phase as f32)
    }

    #[test]
    fn a_plane_wave_is_found_on_a_uca() {
        let geometry = uca();
        let mut scan = ideal_scan(&geometry, None);
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
    fn a_measured_table_removes_the_bias_of_skewed_element_phases() {
        let geometry = uca();
        let mut measuring = scene_of(geometry.clone(), 0.0, -30.0, 1);
        measuring.distortion = Some(skewed);
        let table = measuring
            .distortion_table(&[FREQ - 1e6, FREQ + 1e6], 1.0)
            .unwrap();
        let manifold = Manifold::measured(geometry.clone(), Arc::new(table)).unwrap();
        assert!(manifold.uses_table_at(FREQ));
        let mut measured = Beamscan::new(&manifold, FREQ, 0.5, None).unwrap();
        let mut ideal = ideal_scan(&geometry, None);
        let mut biased = 0;
        for step in 0..12u32 {
            let azimuth = f64::from(step) * 30.0 + 7.0;
            let mut scene = scene_of(geometry.clone(), azimuth, -30.0, u64::from(step) + 40);
            scene.distortion = Some(skewed);
            let snapshots = snapshots_of(&mut scene);
            let error = |scan: &mut Beamscan| {
                let found = estimate(scan, &snapshots, 1000.0).unwrap();
                wrap_deg(f64::from(found.azimuth_deg) - azimuth).abs()
            };
            let right = error(&mut measured);
            assert!(right < 0.75, "{azimuth}: {right} deg off with the table");
            if error(&mut ideal) > 2.5 {
                biased += 1;
            }
        }
        assert!(biased >= 6, "only {biased} of 12 biased without the table");
    }

    #[test]
    fn a_line_array_reports_its_mirror() {
        let geometry = Geometry::ula(0.5 * lambda(), 4, 90.0).unwrap();
        let axis = geometry.line_axis_deg().unwrap() as f32;
        let mut scan = ideal_scan(&geometry, Some(axis));
        let snapshots = snapshots_from(geometry, 30.0, -25.0, 3);
        let found = estimate(&mut scan, &snapshots, 300.0).unwrap();
        let mirror = found.mirror_deg.unwrap();
        let pair = [found.azimuth_deg, mirror];
        assert!(pair.iter().any(|a| (a - 30.0).abs() < 2.0), "{pair:?}");
        assert!(pair.iter().any(|a| (a - 150.0).abs() < 2.0), "{pair:?}");
    }

    #[test]
    fn incoherent_lanes_have_low_quality() {
        let geometry = uca();
        let mut scan = ideal_scan(&geometry, None);
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
        let geometry = uca();
        let mut scan = ideal_scan(&geometry, None);
        let snapshots = snapshots_from(geometry, 80.0, -20.0, 4);
        let weak = estimate(&mut scan, &snapshots, 10.0).unwrap().sigma_deg;
        let strong = estimate(&mut scan, &snapshots, 1000.0).unwrap().sigma_deg;
        assert!(strong < weak / 5.0, "{strong} {weak}");
        assert!((MIN_SIGMA_DEG..=MAX_SIGMA_DEG).contains(&strong));
    }

    #[test]
    fn sigma_follows_the_aperture_seen_from_the_azimuth() {
        let geometry = uca();
        let mut scan = ideal_scan(&geometry, None);
        for azimuth in [0.0, 45.0, 80.0, 200.0] {
            let snapshots = snapshots_from(geometry.clone(), azimuth, -20.0, 6);
            let found = estimate(&mut scan, &snapshots, 100.0).unwrap();
            let (sin, cos) = f64::from(found.azimuth_deg).to_radians().sin_cos();
            let across: Vec<f64> = geometry
                .positions()
                .iter()
                .map(|p| p.x * cos - p.y * sin)
                .collect();
            let mean = across.iter().sum::<f64>() / across.len() as f64;
            let aperture = (across.iter().map(|a| (a - mean).powi(2)).sum::<f64>()
                / across.len() as f64)
                .sqrt();
            let expected = 180.0 / PI / (TAU / lambda() * aperture * (2.0 * 4.0 * 100.0f64).sqrt());
            let sigma = f64::from(found.sigma_deg);
            assert!(
                (sigma - expected).abs() <= 0.02 * expected,
                "{azimuth}: {sigma} vs {expected}"
            );
        }
    }

    #[test]
    fn silence_and_bad_input_give_nothing() {
        let geometry = uca();
        let manifold = Manifold::ideal(geometry.clone());
        let mut scan = ideal_scan(&geometry, None);
        let zeros = [Complex::default(); 4];
        assert!(scan.estimate(&[&zeros], 10.0).is_none());
        let nan = [Complex::new(f32::NAN, 0.0); 4];
        assert!(scan.estimate(&[&nan], 10.0).is_none());
        assert!(scan.estimate(&[&zeros[..2]], 10.0).is_none());
        assert!(scan.estimate(&[], 10.0).is_none());
        assert!(Beamscan::new(&manifold, 0.0, 0.5, None).is_err());
        assert!(Beamscan::new(&manifold, FREQ, 0.0, None).is_err());
        assert!(Beamscan::new(&manifold, FREQ, 0.5, Some(f32::NAN)).is_err());
    }
}
