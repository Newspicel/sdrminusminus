use num_complex::Complex;

use crate::{
    linalg::{Eigen, HermitianEigen, LinalgError},
    steering::SteeringGrid,
};

/// What a scan of the grid came to: where the peak sits, how far it stands above the rest of the
/// circle, and the whole surface so an operator can see whether that peak was the only one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bearing {
    pub bearing_deg: f32,
    /// Peak height above the surface's own floor, squashed into `0..=1`.
    pub confidence: f32,
    pub peak_to_floor_db: f32,
}

/// Conventional beamforming: the power an array steered at each bearing would see.
///
/// It resolves nothing two beamwidths apart, and it always answers — which is exactly why it is
/// the baseline the operator can fall back to when the covariance is too short for MUSIC.
pub fn correlative(r: &[Complex<f32>], grid: &SteeringGrid, out: &mut Vec<f32>) {
    let n = grid.elements();
    out.clear();
    out.reserve(grid.points());
    for point in 0..grid.points() {
        let steering = grid.vector(point);
        let mut power = 0.0f32;
        for row in 0..n {
            let mut sum = Complex::default();
            for col in 0..n {
                sum += r[row * n + col] * steering[col];
            }
            power += (steering[row].conj() * sum).re;
        }
        out.push(power.max(f32::MIN_POSITIVE) / n as f32);
    }
}

/// The MUSIC pseudospectrum for a given number of sources.
///
/// A steering vector that lies in the signal subspace is nearly orthogonal to every noise
/// eigenvector, so the reciprocal of that projection spikes — far more sharply than any
/// beamformer, at the price of needing the source count to be right.
pub struct Music {
    solver: HermitianEigen,
    eigen: Eigen,
}

impl Music {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        Ok(Self {
            solver: HermitianEigen::new(order)?,
            eigen: Eigen::default(),
        })
    }

    #[must_use]
    pub const fn eigenvalues(&self) -> &Vec<f32> {
        &self.eigen.values
    }

    pub fn pseudospectrum(
        &mut self,
        r: &[Complex<f32>],
        grid: &SteeringGrid,
        sources: usize,
        out: &mut Vec<f32>,
    ) {
        let n = self.solver.order();
        self.solver.solve(r, &mut self.eigen);
        let noise = n.saturating_sub(sources.clamp(1, n.saturating_sub(1).max(1)));
        out.clear();
        out.reserve(grid.points());
        for point in 0..grid.points() {
            let steering = grid.vector(point);
            let mut projection = 0.0f32;
            for index in 0..noise {
                let vector = self.eigen.vector(index);
                let mut sum = Complex::<f32>::default();
                for (element, value) in steering.iter().zip(vector) {
                    sum += element.conj() * value;
                }
                projection += sum.norm_sqr();
            }
            out.push(1.0 / projection.max(1e-12));
        }
    }
}

/// Reads a bearing off a surface, interpolating between grid points and measuring the peak
/// against the median of everything else.
#[must_use]
pub fn peak(surface: &[f32], grid: &SteeringGrid, scratch: &mut Vec<f32>) -> Bearing {
    if surface.is_empty() {
        return Bearing::default();
    }
    let points = surface.len();
    let index = surface
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(index, _)| index);
    let db = |value: f32| 10.0 * value.max(1e-30).log10();
    let left = db(surface[(index + points - 1) % points]);
    let centre = db(surface[index]);
    let right = db(surface[(index + 1) % points]);
    let denominator = left - 2.0 * centre + right;
    let fraction = if denominator.abs() > f32::MIN_POSITIVE {
        (0.5 * (left - right) / denominator).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let step = grid.step_deg() as f32;
    let bearing = (index as f32 + fraction) * step;
    scratch.clear();
    scratch.extend_from_slice(surface);
    scratch.sort_by(f32::total_cmp);
    let floor = db(scratch[points / 2]);
    let peak_to_floor_db = centre - floor;
    Bearing {
        bearing_deg: bearing.rem_euclid(360.0),
        confidence: (peak_to_floor_db / 20.0).clamp(0.0, 1.0),
        peak_to_floor_db,
    }
}

/// Quantises a surface to the byte-per-degree form the wire carries: full scale is the peak, and
/// the bottom of the scale sits `span_db` below it.
pub fn quantize(surface: &[f32], span_db: f32, out: &mut [u8]) {
    let peak = surface.iter().copied().fold(f32::MIN_POSITIVE, f32::max);
    let top = 10.0 * peak.max(1e-30).log10();
    let span = span_db.max(1.0);
    let slots = out.len().max(1);
    for (index, slot) in out.iter_mut().enumerate() {
        let source = surface.get(index * surface.len() / slots).copied();
        let db = 10.0 * source.unwrap_or(0.0).max(1e-30).log10();
        let level = ((db - top + span) / span * 255.0).clamp(0.0, 255.0);
        *slot = level as u8;
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;
    use crate::{
        covariance::Covariance,
        steering::{SteeringGrid, uca, ula},
    };

    const FREQ_HZ: f64 = 300e6;

    fn array_snapshots(
        elements: &[crate::steering::Element],
        bearings: &[f64],
        len: usize,
        noise: f32,
    ) -> Vec<Vec<Complex<f32>>> {
        let mut state = 0x2024_0413u64;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32 - 1.0
        };
        let wavelength = crate::steering::LIGHT_SPEED_M_S / FREQ_HZ;
        let mut lanes = vec![vec![Complex::default(); len]; elements.len()];
        for (source, &bearing) in bearings.iter().enumerate() {
            let tone = 0.03 + 0.017 * source as f32;
            for sample in 0..len {
                let carrier = Complex::from_polar(1.0f32, TAU * tone * sample as f32);
                for (lane, element) in lanes.iter_mut().zip(elements) {
                    let phase =
                        (std::f64::consts::TAU * element.projected(bearing) / wavelength) as f32;
                    lane[sample] += carrier * Complex::from_polar(1.0, phase);
                }
            }
        }
        for lane in &mut lanes {
            for sample in lane.iter_mut() {
                *sample += Complex::new(next() * noise, next() * noise);
            }
        }
        lanes
    }

    fn covariance_of(lanes: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
        let borrowed: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut covariance = Covariance::new(lanes.len());
        covariance.set_forward_backward(false);
        covariance.accumulate(&borrowed);
        let mut r = Vec::new();
        covariance.matrix(&mut r);
        r
    }

    #[test]
    fn music_on_a_four_element_circle_recovers_the_bearing_within_a_degree() {
        let elements = uca(0.35, 4);
        let grid = SteeringGrid::new(&elements, FREQ_HZ, 1.0);
        for want in [0.0f64, 37.0, 137.0, 271.0, 359.0] {
            let lanes = array_snapshots(&elements, &[want], 4_096, 0.05);
            let r = covariance_of(&lanes);
            let mut music = Music::new(4).expect("order");
            let mut surface = Vec::new();
            music.pseudospectrum(&r, &grid, 1, &mut surface);
            let mut scratch = Vec::new();
            let reading = peak(&surface, &grid, &mut scratch);
            let error = (f64::from(reading.bearing_deg) - want).abs();
            let error = error.min(360.0 - error);
            assert!(error < 1.0, "wanted {want}, got {reading:?}");
            assert!(reading.confidence > 0.5, "{reading:?}");
        }
    }

    #[test]
    fn the_beamformer_agrees_with_music_on_where_the_source_is() {
        let elements = uca(0.35, 4);
        let grid = SteeringGrid::new(&elements, FREQ_HZ, 1.0);
        let lanes = array_snapshots(&elements, &[212.0], 4_096, 0.05);
        let r = covariance_of(&lanes);
        let mut surface = Vec::new();
        correlative(&r, &grid, &mut surface);
        let mut scratch = Vec::new();
        let reading = peak(&surface, &grid, &mut scratch);
        let error = (f64::from(reading.bearing_deg) - 212.0).abs();
        assert!(error.min(360.0 - error) < 4.0, "{reading:?}");
    }

    #[test]
    fn music_separates_two_sources_a_beamwidth_apart_on_a_line() {
        let elements = ula(0.5, 6);
        let grid = SteeringGrid::new(&elements, FREQ_HZ, 0.5);
        let lanes = array_snapshots(&elements, &[70.0, 100.0], 8_192, 0.05);
        let r = covariance_of(&lanes);
        let mut music = Music::new(6).expect("order");
        let mut surface = Vec::new();
        music.pseudospectrum(&r, &grid, 2, &mut surface);
        let mut peaks: Vec<(usize, f32)> = surface
            .iter()
            .enumerate()
            .filter(|(index, value)| {
                let points = surface.len();
                let left = surface[(index + points - 1) % points];
                let right = surface[(index + 1) % points];
                **value > left && **value > right
            })
            .map(|(index, value)| (index, *value))
            .collect();
        peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let found: Vec<f64> = peaks
            .iter()
            .take(4)
            .map(|(index, _)| grid.bearing_deg(*index))
            .collect();
        for want in [70.0, 100.0] {
            assert!(
                found.iter().any(|got| (got - want).abs() < 2.0),
                "no peak near {want} among {found:?}"
            );
        }
    }

    #[test]
    fn eigenvalues_come_back_with_the_signal_subspace_on_top() {
        let elements = uca(0.35, 4);
        let lanes = array_snapshots(&elements, &[45.0], 4_096, 0.05);
        let r = covariance_of(&lanes);
        let grid = SteeringGrid::new(&elements, FREQ_HZ, 5.0);
        let mut music = Music::new(4).expect("order");
        let mut surface = Vec::new();
        music.pseudospectrum(&r, &grid, 1, &mut surface);
        let values = music.eigenvalues();
        assert!(values[3] > 20.0 * values[2], "{values:?}");
    }

    #[test]
    fn noise_alone_gives_a_flat_surface_and_no_confidence() {
        let elements = uca(0.35, 4);
        let grid = SteeringGrid::new(&elements, FREQ_HZ, 1.0);
        let lanes = array_snapshots(&elements, &[], 4_096, 1.0);
        let r = covariance_of(&lanes);
        let mut surface = Vec::new();
        correlative(&r, &grid, &mut surface);
        let mut scratch = Vec::new();
        let reading = peak(&surface, &grid, &mut scratch);
        assert!(reading.peak_to_floor_db < 3.0, "{reading:?}");
        assert!(reading.confidence < 0.2, "{reading:?}");
    }

    #[test]
    fn quantising_puts_the_peak_at_full_scale_and_the_floor_at_the_bottom() {
        let mut surface = vec![1.0f32; 360];
        surface[137] = 1_000.0;
        let mut bytes = [0u8; 360];
        quantize(&surface, 30.0, &mut bytes);
        assert_eq!(bytes[137], 255);
        assert_eq!(bytes[0], 0);
    }
}
