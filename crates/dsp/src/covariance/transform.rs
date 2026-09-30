use num_complex::Complex;

use super::CovarianceError;
use crate::linalg::{CMat, LinalgError};
use crate::manifold::{MAX_ELEMENTS, Permutation};

pub fn load_diagonal(r: &mut CMat, fraction: f32) -> f32 {
    let n = r.order();
    let delta = fraction * r.trace_re() / n as f32;
    for i in 0..n {
        r.add(i, i, Complex::new(delta, 0.0));
    }
    delta
}

pub fn forward_backward(r: &mut CMat, perm: &Permutation) -> Result<(), CovarianceError> {
    let n = r.order();
    if perm.len() != n {
        return Err(LinalgError::Order(perm.len()).into());
    }
    let original = r.clone();
    for i in 0..n {
        for j in 0..n {
            let mirrored = original.get(perm.get(i), perm.get(j)).conj();
            r.set(i, j, (original.get(i, j) + mirrored) * 0.5);
        }
    }
    Ok(())
}

pub fn smooth(
    r: &CMat,
    order: &Permutation,
    subarrays: usize,
    fb: bool,
    out: &mut CMat,
) -> Result<(), CovarianceError> {
    let n = r.order();
    if order.len() != n {
        return Err(LinalgError::Order(order.len()).into());
    }
    let len = subarray_len(n, subarrays).ok_or(CovarianceError::TooFewForSmoothing)?;
    let mut indices = [0usize; MAX_ELEMENTS];
    for (slot, index) in indices.iter_mut().zip(order.iter()) {
        *slot = index;
    }
    let mut along = CMat::zeros(n)?;
    r.permuted(&indices[..n], &mut along)?;
    out.resize(len)?;
    for start in 0..subarrays {
        for i in 0..len {
            for j in 0..len {
                out.add(i, j, along.get(start + i, start + j));
            }
        }
    }
    out.scale(1.0 / subarrays as f32);
    if fb {
        forward_backward(out, &Permutation::reversal(len))?;
    }
    Ok(())
}

pub fn smooth_diagonal(d: &[f32], subarrays: usize, fb: bool, out: &mut [f32]) -> usize {
    let Some(len) = subarray_len(d.len(), subarrays).filter(|&len| out.len() >= len) else {
        return 0;
    };
    for (i, value) in out[..len].iter_mut().enumerate() {
        *value = d[i..i + subarrays].iter().sum::<f32>() / subarrays as f32;
    }
    if fb {
        for i in 0..len / 2 {
            let mean = 0.5 * (out[i] + out[len - 1 - i]);
            out[i] = mean;
            out[len - 1 - i] = mean;
        }
    }
    len
}

fn subarray_len(elements: usize, subarrays: usize) -> Option<usize> {
    if subarrays == 0 || subarrays > elements {
        return None;
    }
    let len = elements - subarrays + 1;
    (len >= 2).then_some(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::{Eigen, HermitianEigen};
    use crate::manifold::{Direction, Geometry, Manifold, Winding, steer};
    use crate::scene::{ArrayScene, SceneCopy, SceneSignal, SceneSource};
    use crate::special::{norm_deg, wrap_deg};

    const FREQ: f64 = 433.92e6;

    fn wavelength() -> f64 {
        crate::manifold::LIGHT_SPEED_M_S / FREQ
    }

    fn covariance_of(lanes: &[Vec<Complex<f32>>]) -> CMat {
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let mut covariance = super::super::SampleCovariance::new(lanes.len()).unwrap();
        covariance.accumulate(&views);
        let mut r = CMat::zeros(1).unwrap();
        assert!(covariance.matrix(&mut r));
        r
    }

    fn music(r: &CMat, sources: usize, steering: &[Complex<f32>]) -> f32 {
        let n = r.order();
        let mut eigen = HermitianEigen::new(n).unwrap();
        let mut decomposition = Eigen::new();
        eigen.solve(r, &mut decomposition).unwrap();
        let noise: f32 = (0..n - sources)
            .map(|k| {
                let v = decomposition.vector(k);
                v.iter()
                    .zip(steering)
                    .map(|(e, a)| e.conj() * a)
                    .sum::<Complex<f32>>()
                    .norm_sqr()
            })
            .sum();
        steering.iter().map(Complex::norm_sqr).sum::<f32>() / noise.max(1e-12)
    }

    fn music_at(r: &CMat, sources: usize, positions: &[crate::manifold::Vec3], az: f64) -> f32 {
        let mut a = vec![Complex::new(0.0f32, 0.0); positions.len()];
        steer(positions, FREQ, Direction::horizon(az), &mut a);
        music(r, sources, &a)
    }

    fn peaks(spectrum: &[f32], floor_ratio: f32) -> Vec<usize> {
        let top = spectrum.iter().copied().fold(0.0f32, f32::max);
        (1..spectrum.len() - 1)
            .filter(|&i| spectrum[i] > spectrum[i - 1] && spectrum[i] >= spectrum[i + 1])
            .filter(|&i| spectrum[i] >= top * floor_ratio)
            .collect()
    }

    #[test]
    fn forward_backward_keeps_an_even_uca_bearing_without_a_ghost() {
        let geometry = Geometry::uca(0.4 * wavelength(), 6, 0.0, Winding::Clockwise).unwrap();
        let mut scene = ArrayScene::new(geometry.clone(), FREQ, 1e6)
            .with_source(SceneSource::new(
                Direction::horizon(40.0),
                0.0,
                SceneSignal::Noise {
                    offset_hz: 1e4,
                    bandwidth_hz: 2e5,
                },
            ))
            .with_noise_db(-10.0)
            .with_seed(9);
        let mut r = covariance_of(&scene.render(4096).unwrap());
        let pairs = geometry.antipodes().unwrap();
        forward_backward(&mut r, &pairs).unwrap();
        let positions = geometry.positions();
        let spectrum: Vec<f32> = (0..3600)
            .map(|k| music_at(&r, 1, positions, k as f64 * 0.1))
            .collect();
        let best = spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, _)| k as f64 * 0.1)
            .unwrap();
        assert!((best - 40.0).abs() < 1.0, "peak at {best}");
        let ghost = music_at(&r, 1, positions, 220.0) / music_at(&r, 1, positions, 40.0);
        assert!(10.0 * ghost.log10() < -20.0, "ghost {ghost}");
        let peak = spectrum.iter().copied().fold(0.0f32, f32::max);
        let elsewhere = spectrum
            .iter()
            .enumerate()
            .filter(|&(k, _)| wrap_deg(k as f64 * 0.1 - 40.0).abs() > 15.0)
            .map(|(_, &power)| power)
            .fold(0.0f32, f32::max);
        assert!(
            10.0 * (elsewhere / peak).log10() < -20.0,
            "{elsewhere} vs {peak}"
        );
    }

    #[test]
    fn forward_backward_is_refused_for_an_odd_uca() {
        let odd = Geometry::uca(0.3, 5, 0.0, Winding::Clockwise).unwrap();
        assert_eq!(odd.antipodes(), None);
        let mut r = CMat::identity(5).unwrap();
        assert_eq!(
            forward_backward(&mut r, &Permutation::identity(4)),
            Err(CovarianceError::Linalg(LinalgError::Order(4)))
        );
    }

    fn line_spectrum(r: &CMat, sources: usize, positions: &[crate::manifold::Vec3]) -> Vec<f32> {
        (0..=180)
            .map(|az| music_at(r, sources, positions, f64::from(az)))
            .collect()
    }

    fn sharpness(spectrum: &[f32]) -> f32 {
        let mut sorted = spectrum.to_vec();
        sorted.sort_by(f32::total_cmp);
        sorted[sorted.len() - 1] / sorted[sorted.len() / 2]
    }

    fn signal_count(r: &CMat) -> usize {
        let mut eigen = HermitianEigen::new(r.order()).unwrap();
        let mut decomposition = Eigen::new();
        eigen.solve(r, &mut decomposition).unwrap();
        let floor = decomposition.values()[0].max(1e-12);
        decomposition
            .values()
            .iter()
            .filter(|&&value| value > 20.0 * floor)
            .count()
    }

    #[test]
    fn smoothing_resolves_coherent_multipath_on_a_ula() {
        let geometry = Geometry::ula(wavelength() / 2.0, 8, 180.0).unwrap();
        let direct = SceneSource::new(
            Direction::horizon(70.0),
            0.0,
            SceneSignal::Tone { offset_hz: 2e3 },
        );
        let bounce = SceneSource {
            direction: Direction::horizon(100.0),
            copy_of: Some(SceneCopy {
                source: 0,
                amplitude: 0.9,
                phase_deg: 60.0,
            }),
            ..direct
        };
        let mut scene = ArrayScene::new(geometry.clone(), FREQ, 1e5)
            .with_source(direct)
            .with_source(bounce)
            .with_noise_db(-25.0)
            .with_seed(4);
        let r = covariance_of(&scene.render(8192).unwrap());
        assert_eq!(signal_count(&r), 1);
        let raw = line_spectrum(&r, 1, geometry.positions());
        assert!(sharpness(&raw) < 10.0, "{}", sharpness(&raw));

        let order = geometry.axis_order().unwrap();
        let mut smoothed = CMat::zeros(1).unwrap();
        smooth(&r, &order, 3, true, &mut smoothed).unwrap();
        assert_eq!(smoothed.order(), 6);
        assert_eq!(signal_count(&smoothed), 2);
        let sub: Vec<_> = order
            .iter()
            .take(6)
            .map(|i| geometry.positions()[i])
            .collect();
        let resolved = line_spectrum(&smoothed, 2, &sub);
        assert!(sharpness(&resolved) > 1e3, "{}", sharpness(&resolved));
        let found = peaks(&resolved, 0.01);
        assert_eq!(found.len(), 2, "{found:?}");
        let mut found: Vec<f64> = found.iter().map(|&k| norm_deg(k as f64)).collect();
        found.sort_by(f64::total_cmp);
        assert!((found[0] - 70.0).abs() < 2.0, "{found:?}");
        assert!((found[1] - 100.0).abs() < 2.0, "{found:?}");
        let manifold = Manifold::ideal(geometry);
        assert_eq!(manifold.len(), 8);
    }

    #[test]
    fn smoothing_needs_two_elements_per_subarray() {
        let r = CMat::identity(4).unwrap();
        let mut out = CMat::zeros(1).unwrap();
        let order = Permutation::identity(4);
        assert_eq!(
            smooth(&r, &order, 4, false, &mut out),
            Err(CovarianceError::TooFewForSmoothing)
        );
        assert_eq!(
            smooth(&r, &order, 0, false, &mut out),
            Err(CovarianceError::TooFewForSmoothing)
        );
        smooth(&r, &order, 3, false, &mut out).unwrap();
        assert_eq!(out, CMat::identity(2).unwrap());
        let mut diagonal = [0.0f32; 4];
        assert_eq!(
            smooth_diagonal(&[1.0, 2.0, 3.0, 6.0], 2, false, &mut diagonal),
            3
        );
        assert_eq!(&diagonal[..3], &[1.5, 2.5, 4.5]);
        assert_eq!(
            smooth_diagonal(&[1.0, 2.0, 3.0, 6.0], 2, true, &mut diagonal),
            3
        );
        assert_eq!(&diagonal[..3], &[3.0, 2.5, 3.0]);
        assert_eq!(smooth_diagonal(&[1.0, 2.0], 2, false, &mut diagonal), 0);
    }

    #[test]
    fn loading_adds_a_fraction_of_the_mean_power() {
        let mut r = CMat::identity(4).unwrap();
        r.set(0, 0, Complex::new(5.0, 0.0));
        let delta = load_diagonal(&mut r, 0.1);
        assert!((delta - 0.2).abs() < 1e-6);
        assert!((r.get(0, 0).re - 5.2).abs() < 1e-6);
        assert!((r.get(3, 3).re - 1.2).abs() < 1e-6);
    }
}
