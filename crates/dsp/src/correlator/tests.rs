use std::f64::consts::{PI, TAU};

use super::*;
use crate::manifold::{Direction, Geometry};
use crate::scene::{ArrayScene, SceneSignal, SceneSource};

const RATE: f64 = 1_000_000.0;
const CENTER_HZ: f64 = 100e6;

fn views(lanes: &[Vec<Complex<f32>>]) -> Vec<&[Complex<f32>]> {
    lanes.iter().map(Vec::as_slice).collect()
}

fn noise_lanes(lanes: usize, len: usize, seed: u64) -> Vec<Vec<Complex<f32>>> {
    let geometry = Geometry::ula(0.1, lanes, 90.0).unwrap();
    let mut scene = ArrayScene::new(geometry, CENTER_HZ, RATE)
        .with_noise_db(0.0)
        .with_seed(seed);
    scene.render(len).unwrap()
}

fn wrap(angle: f64) -> f64 {
    (angle + PI).rem_euclid(TAU) - PI
}

fn mean_coherence(correlator: &FxCorrelator, width: usize) -> f64 {
    let bands = correlator.fft_size() / width;
    let mut sum = 0.0;
    for baseline in 0..correlator.baselines() {
        for band in 0..bands {
            let bins = band * width..(band + 1) * width;
            sum += correlator.band(baseline, bins).unwrap().coherence;
        }
    }
    sum / (correlator.baselines() * bands) as f64
}

#[test]
fn fx_visibility_phase_equals_the_geometric_delay() {
    let fft = 256;
    let delay = 1.37;
    let geometry = Geometry::ula(0.1, 2, 90.0).unwrap();
    let mut scene = ArrayScene::new(geometry, CENTER_HZ, RATE)
        .with_source(SceneSource::new(
            Direction::horizon(0.0),
            0.0,
            SceneSignal::Broadband,
        ))
        .with_noise_db(-30.0)
        .with_seed(7);
    scene.lane_delay_samples = vec![0.0, delay as f32];
    let lanes = scene.render(400 * fft).unwrap();
    let mut correlator = FxCorrelator::new(2, fft, false).unwrap();
    assert_eq!(correlator.push(&views(&lanes)).unwrap(), 400);
    let edge = fft * 15 / 100;
    for bin in edge..fft - edge {
        let f = (bin as f64 - (fft / 2) as f64) / fft as f64;
        let phase = correlator.visibility(0, bin).unwrap().arg();
        let error = wrap(phase - TAU * f * delay);
        assert!(error.abs() < 0.05, "bin {bin}: phase error {error}");
    }
    let measured = correlator.delay_samples(0, 0..fft).unwrap();
    assert!((measured - delay).abs() < 0.05, "delay {measured}");
    let narrow = correlator.band(0, fft / 2 - 2..fft / 2 + 2).unwrap();
    assert!(narrow.coherence > 0.95, "coherence {}", narrow.coherence);
    let wide = correlator.band(0, edge..fft - edge).unwrap();
    assert!(wide.coherence < 0.5, "a delay decorrelates the band");
}

#[test]
fn a_negative_delay_reads_negative() {
    let fft = 512;
    let lead = [Complex::new(0.0f32, 0.0); 2];
    let lanes = noise_lanes(2, 200 * fft + 2, 11);
    let early: Vec<Complex<f32>> = lanes[0][2..].to_vec();
    let late: Vec<Complex<f32>> = lead.iter().chain(&lanes[0][..200 * fft]).copied().collect();
    let mut correlator = FxCorrelator::new(2, fft, true).unwrap();
    correlator
        .push(&[&late[..200 * fft], &early[..200 * fft]])
        .unwrap();
    let measured = correlator.delay_samples(0, 0..fft).unwrap();
    assert!((measured + 4.0).abs() < 0.05, "delay {measured}");
}

#[test]
fn noise_only_coherence_falls_as_one_over_root_frames() {
    let fft = 1024;
    let width = 32;
    let lanes = noise_lanes(5, 256 * fft, 3);
    let mut long = FxCorrelator::new(5, fft, false).unwrap();
    let early: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[..16 * fft]).collect();
    let mut quick = FxCorrelator::new(5, fft, false).unwrap();
    assert_eq!(quick.push(&early).unwrap(), 16);
    long.push(&views(&lanes)).unwrap();
    assert_eq!((quick.frames(), long.frames()), (16, 256));
    for correlator in [&quick, &long] {
        let expected = 1.0 / (correlator.frames() as f64 * width as f64).sqrt();
        let measured = mean_coherence(correlator, width);
        let ratio = measured / expected;
        assert!(
            (0.5..=1.5).contains(&ratio),
            "{} frames: coherence {measured}, expected {expected}",
            correlator.frames()
        );
    }
    let fall = mean_coherence(&quick, width) / mean_coherence(&long, width);
    assert!((3.2..=4.8).contains(&fall), "fall {fall}");
}

#[test]
fn common_noise_gives_unit_coherence() {
    let fft = 256;
    let lanes = noise_lanes(2, 64 * fft, 5);
    let mut correlator = FxCorrelator::new(2, fft, true).unwrap();
    correlator.push(&[&lanes[0], &lanes[0]]).unwrap();
    let band = correlator.band(0, 0..fft).unwrap();
    assert!(band.coherence > 0.99, "coherence {}", band.coherence);
    assert!(band.phase_rad.abs() < 1e-6);
    assert!(correlator.delay_samples(0, 0..fft).unwrap().abs() < 0.05);
    assert!(band.snr_db > 40.0);
}

#[test]
fn long_integration_stays_exact() {
    let fft = MAX_CORRELATOR_FFT;
    let bin = 1_000;
    let turn = Complex::from_polar(1.0f32, 0.7);
    let a: Vec<Complex<f32>> = (0..fft)
        .map(|n| Complex::from_polar(1.0, (TAU * (bin * n) as f64 / fft as f64) as f32))
        .collect();
    let b: Vec<Complex<f32>> = a.iter().map(|value| value * turn).collect();
    let mut correlator = FxCorrelator::new(2, fft, false).unwrap();
    correlator.push(&[&a, &b]).unwrap();
    let shifted = bin + fft / 2;
    let first = correlator.visibility(0, shifted).unwrap();
    let first_auto = correlator.auto(1, shifted).unwrap();
    let frames = 100_000_000usize.div_ceil(fft);
    for _ in 1..frames {
        correlator.push(&[&a, &b]).unwrap();
    }
    assert_eq!(correlator.frames(), frames as u64);
    assert!(correlator.frames() * fft as u64 >= 100_000_000);
    let last = correlator.visibility(0, shifted).unwrap();
    let drift = (last - first).norm() / first.norm();
    assert!(drift < 1e-9, "drift {drift}");
    let auto_drift = (correlator.auto(1, shifted).unwrap() - first_auto).abs() / first_auto;
    assert!(auto_drift < 1e-9, "auto drift {auto_drift}");
    assert!((last.arg() + 0.7).abs() < 1e-5);
}

#[test]
fn five_lanes_give_ten_baselines_in_order() {
    let correlator = FxCorrelator::new(5, 64, false).unwrap();
    assert_eq!(correlator.baselines(), 10);
    assert_eq!(correlator.pair(0), Some((0, 1)));
    assert_eq!(correlator.pair(3), Some((0, 4)));
    assert_eq!(correlator.pair(4), Some((1, 2)));
    assert_eq!(correlator.pair(9), Some((3, 4)));
    assert_eq!(correlator.pair(10), None);
}

#[test]
fn overlap_halves_the_hop() {
    let lanes = noise_lanes(2, 64 * 128, 9);
    let mut plain = FxCorrelator::new(2, 128, false).unwrap();
    let mut overlapped = FxCorrelator::new(2, 128, true).unwrap();
    assert_eq!(plain.push(&views(&lanes)).unwrap(), 64);
    assert_eq!(overlapped.push(&views(&lanes)).unwrap(), 127);
    assert_eq!(overlapped.hop(), 64);
    let split: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[..100]).collect();
    let mut pieces = FxCorrelator::new(2, 128, false).unwrap();
    assert_eq!(pieces.push(&split).unwrap(), 0);
    let rest: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[100..300]).collect();
    assert_eq!(pieces.push(&rest).unwrap(), 2);
}

#[test]
fn bad_sizes_lanes_and_bins_are_refused() {
    assert_eq!(
        FxCorrelator::new(2, 100, false).err(),
        Some(CorrelatorError::FftSize(100))
    );
    assert_eq!(
        FxCorrelator::new(2, 16_384, false).err(),
        Some(CorrelatorError::FftSize(16_384))
    );
    assert_eq!(
        FxCorrelator::new(1, 64, false).err(),
        Some(CorrelatorError::Lanes(1))
    );
    let mut correlator = FxCorrelator::new(3, 64, false).unwrap();
    let lane = vec![Complex::new(1.0f32, 0.0); 64];
    assert_eq!(
        correlator.push(&[&lane, &lane]).err(),
        Some(CorrelatorError::Lanes(2))
    );
    assert_eq!(
        correlator.push(&[&lane, &lane, &lane[..10]]).err(),
        Some(CorrelatorError::LaneLength)
    );
    assert_eq!(
        correlator.band(0, 10..65).err(),
        Some(CorrelatorError::Bins(10, 65))
    );
    assert_eq!(
        correlator.band(3, 0..64).err(),
        Some(CorrelatorError::Baseline(3))
    );
    assert_eq!(
        correlator.band(0, 0..64).unwrap(),
        BandVisibility::default()
    );
    assert_eq!(correlator.visibility(0, 0), None);
}

#[test]
fn reset_forgets_the_integration_and_the_partial_frame() {
    let lanes = noise_lanes(2, 300, 4);
    let mut correlator = FxCorrelator::new(2, 128, false).unwrap();
    assert_eq!(correlator.push(&views(&lanes)).unwrap(), 2);
    correlator.clear_integration();
    assert_eq!(correlator.frames(), 0);
    let more: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[..84]).collect();
    assert_eq!(correlator.push(&more).unwrap(), 1);
    correlator.reset();
    assert_eq!(correlator.push(&more).unwrap(), 0);
    assert_eq!(correlator.frames(), 0);
}
