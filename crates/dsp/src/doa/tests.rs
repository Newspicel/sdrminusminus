use std::sync::Arc;

use num_complex::Complex;

use super::*;
use crate::covariance::SampleCovariance;
use crate::manifold::{Geometry, LIGHT_SPEED_M_S, Vec3, Winding, phase_rates};
use crate::scene::{ArrayScene, SceneCopy, SceneSignal, SceneSource};
use crate::special::wrap_deg;

const FREQ: f64 = 433.92e6;
const RATE: f64 = 2.4e6;
const SNAPSHOTS: usize = 4096;
const NO_CAL_SIGMA_DEG: f32 = 5.831;

fn wavelength() -> f64 {
    LIGHT_SPEED_M_S / FREQ
}

fn kraken() -> Geometry {
    Geometry::uca(0.2939, 5, 0.0, Winding::Clockwise).unwrap()
}

fn uca_with_chord(count: usize, chord_wavelengths: f64) -> Geometry {
    let radius =
        chord_wavelengths * wavelength() / (2.0 * (std::f64::consts::PI / count as f64).sin());
    Geometry::uca(radius, count, 0.0, Winding::Clockwise).unwrap()
}

fn uca_beta(count: usize, beta: f64) -> Geometry {
    Geometry::uca(beta / wavenumber(FREQ), count, 0.0, Winding::Clockwise).unwrap()
}

fn ula(count: usize, axis_deg: f64) -> Geometry {
    Geometry::ula(wavelength() / 2.0, count, axis_deg).unwrap()
}

fn broadband(direction: Direction, power_db: f32) -> SceneSource {
    SceneSource::new(direction, power_db, SceneSignal::Broadband)
}

fn scene(geometry: &Geometry, sources: &[(f64, f32)], seed: u64) -> ArrayScene {
    sources.iter().fold(
        ArrayScene::new(geometry.clone(), FREQ, RATE)
            .with_noise_db(0.0)
            .with_seed(seed),
        |scene, &(azimuth, power_db)| {
            scene.with_source(broadband(Direction::horizon(azimuth), power_db))
        },
    )
}

fn covariance(scene: &mut ArrayScene, len: usize) -> CMat {
    let lanes = scene.render(len).unwrap();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut sum = SampleCovariance::new(lanes.len()).unwrap();
    sum.accumulate(&views);
    let mut r = CMat::zeros(lanes.len()).unwrap();
    assert!(sum.matrix(&mut r));
    r
}

fn estimate_with(
    manifold: &Manifold,
    config: &DoaConfig,
    r: &CMat,
    snapshots: usize,
    cal_sigma_deg: f32,
) -> DoaReport {
    let mut doa = Doa::new(manifold, config, FREQ).unwrap();
    let mut report = DoaReport::default();
    doa.estimate(manifold, r, snapshots as f64, cal_sigma_deg, &mut report)
        .unwrap();
    report
}

fn estimate(geometry: &Geometry, config: &DoaConfig, r: &CMat) -> DoaReport {
    estimate_with(
        &Manifold::ideal(geometry.clone()),
        config,
        r,
        SNAPSHOTS,
        NO_CAL_SIGMA_DEG,
    )
}

fn with(estimator: Estimator) -> DoaConfig {
    DoaConfig {
        estimator,
        ..DoaConfig::default()
    }
}

fn error_deg(found: f64, want: f64) -> f64 {
    wrap_deg(found - want).abs()
}

fn primary(report: &DoaReport) -> Peak {
    assert!(report.peak_count > 0, "{report:?}");
    report.peaks[0]
}

fn sorted_azimuths(report: &DoaReport) -> Vec<f64> {
    let mut found: Vec<f64> = report.peaks().iter().map(|p| p.azimuth_deg).collect();
    found.sort_by(f64::total_cmp);
    found
}

#[test]
fn bartlett_capon_music_find_one_source_on_the_kraken_uca() {
    let geometry = kraken();
    for (index, bearing) in [0.0, 37.0, 137.0, 271.0, 359.5].into_iter().enumerate() {
        let r = covariance(
            &mut scene(&geometry, &[(bearing, 10.0)], index as u64 + 1),
            SNAPSHOTS,
        );
        for (estimator, limit) in [
            (Estimator::Bartlett, 2.0),
            (Estimator::Capon, 1.0),
            (Estimator::Music, 1.0),
        ] {
            let report = estimate(&geometry, &with(estimator), &r);
            let found = primary(&report).azimuth_deg;
            assert!(
                error_deg(found, bearing) < limit,
                "{estimator:?} at {bearing}: {found}"
            );
            assert_eq!(report.sources, 1);
        }
    }
}

#[test]
fn music_resolves_two_sources_20_degrees_apart_on_an_8_element_ula() {
    let geometry = ula(8, 180.0);
    let r = covariance(
        &mut scene(&geometry, &[(70.0, 20.0), (90.0, 20.0)], 3),
        SNAPSHOTS,
    );
    let report = estimate(&geometry, &DoaConfig::default(), &r);
    assert_eq!(report.peak_count, 2, "{report:?}");
    let found = sorted_azimuths(&report);
    assert!(error_deg(found[0], 70.0) < 1.5, "{found:?}");
    assert!(error_deg(found[1], 90.0) < 1.5, "{found:?}");
}

#[test]
fn root_music_matches_music_on_a_ula_and_reports_the_mirror() {
    let geometry = ula(8, 90.0);
    let r = covariance(&mut scene(&geometry, &[(30.0, 20.0)], 5), SNAPSHOTS);
    let rooted = primary(&estimate(&geometry, &with(Estimator::RootMusic), &r));
    let gridded = primary(&estimate(&geometry, &with(Estimator::Music), &r));
    assert!(error_deg(rooted.azimuth_deg, 30.0) < 0.5, "{rooted:?}");
    assert!(error_deg(rooted.azimuth_deg, gridded.azimuth_deg) < 0.5);
    let mirror = rooted.mirror_deg.unwrap();
    assert!(error_deg(mirror, 180.0 - rooted.azimuth_deg) < 1e-9);
    assert_eq!(rooted.ambiguity, 0.5);
}

#[test]
fn root_music_on_a_uca_uses_phase_mode() {
    let geometry = uca_beta(8, 2.0);
    let manifold = Manifold::ideal(geometry.clone());
    for (seed, bearing) in [(7, 37.0), (8, 200.0)] {
        let r = covariance(&mut scene(&geometry, &[(bearing, 20.0)], seed), SNAPSHOTS);
        let report = estimate(&geometry, &with(Estimator::RootMusic), &r);
        let found = primary(&report).azimuth_deg;
        assert!(error_deg(found, bearing) < 1.0, "{found}");
    }
    let doa = Doa::new(&manifold, &with(Estimator::RootMusic), FREQ).unwrap();
    assert!(doa.mode_bias_deg().is_some_and(|bias| bias < 1.0));
    let element = Doa::new(&manifold, &with(Estimator::Music), FREQ).unwrap();
    assert_eq!(element.mode_bias_deg(), None);
}

#[test]
fn root_music_on_the_kraken_uca_near_the_bessel_null() {
    for chord in [0.45, 0.5] {
        let geometry = uca_with_chord(5, chord);
        for (seed, bearing) in [0.0, 37.0, 137.0, 271.0].into_iter().enumerate() {
            let r = covariance(
                &mut scene(&geometry, &[(bearing, 20.0)], seed as u64 + 11),
                SNAPSHOTS,
            );
            let report = estimate(&geometry, &with(Estimator::RootMusic), &r);
            let found = primary(&report).azimuth_deg;
            assert!(
                error_deg(found, bearing) < 1.5,
                "chord {chord} at {bearing}: {found}"
            );
        }
    }
}

#[test]
fn esprit_near_the_bessel_null_says_so() {
    let manifold = Manifold::ideal(uca_with_chord(5, 0.45));
    assert!(matches!(
        Doa::new(&manifold, &with(Estimator::Esprit), FREQ),
        Err(DoaError::Covariance(CovarianceError::BesselNull))
    ));
    let smoothed = DoaConfig {
        smoothing: 1,
        ..DoaConfig::default()
    };
    assert!(matches!(
        Doa::new(&manifold, &smoothed, FREQ),
        Err(DoaError::Covariance(CovarianceError::BesselNull))
    ));
}

#[test]
fn esprit_finds_two_sources_on_a_ula() {
    let geometry = ula(8, 180.0);
    let r = covariance(
        &mut scene(&geometry, &[(60.0, 20.0), (110.0, 20.0)], 13),
        SNAPSHOTS,
    );
    let report = estimate(&geometry, &with(Estimator::Esprit), &r);
    assert_eq!(report.peak_count, 2, "{report:?}");
    let found = sorted_azimuths(&report);
    assert!(error_deg(found[0], 60.0) < 1.0, "{found:?}");
    assert!(error_deg(found[1], 110.0) < 1.0, "{found:?}");
    assert!(report.peaks().iter().all(|peak| peak.mirror_deg.is_some()));
}

#[test]
fn esprit_on_a_uca_uses_phase_mode() {
    let geometry = uca_beta(8, 2.0);
    for (seed, bearing) in [(17, 37.0), (18, 305.0)] {
        let r = covariance(&mut scene(&geometry, &[(bearing, 20.0)], seed), SNAPSHOTS);
        let report = estimate(&geometry, &with(Estimator::Esprit), &r);
        let found = primary(&report).azimuth_deg;
        assert!(error_deg(found, bearing) < 1.0, "{found}");
    }
}

fn counted(rule: OrderRule, sources: &[(f64, f32)], seed: u64) -> usize {
    let geometry = kraken();
    let r = covariance(&mut scene(&geometry, sources, seed), SNAPSHOTS);
    let config = DoaConfig {
        rule,
        ..DoaConfig::default()
    };
    estimate(&geometry, &config, &r).sources_raw
}

#[test]
fn dominance_counts_zero_one_two() {
    assert_eq!(counted(OrderRule::Dominance, &[], 21), 0);
    assert_eq!(counted(OrderRule::Dominance, &[(40.0, 10.0)], 22), 1);
    assert_eq!(
        counted(OrderRule::Dominance, &[(40.0, 10.0), (200.0, 4.0)], 23),
        2
    );
}

#[test]
fn mdl_counts_sources_on_ideal_data() {
    assert_eq!(counted(OrderRule::Mdl, &[], 21), 0);
    assert_eq!(counted(OrderRule::Mdl, &[(40.0, 10.0)], 22), 1);
    assert_eq!(
        counted(OrderRule::Mdl, &[(40.0, 10.0), (200.0, 4.0)], 23),
        2
    );
}

#[test]
fn dominance_ignores_calibration_leakage() {
    let geometry = kraken();
    let mut leaky = scene(&geometry, &[(120.0, 40.0)], 25);
    leaky.lane_delay_samples = vec![0.0, 0.2, 0.0, 0.0, 0.0];
    let r = covariance(&mut leaky, SNAPSHOTS);
    let config = |rule| DoaConfig {
        rule,
        ..DoaConfig::default()
    };
    let dominance = estimate(&geometry, &config(OrderRule::Dominance), &r);
    let mdl = estimate(&geometry, &config(OrderRule::Mdl), &r);
    assert_eq!(dominance.sources_raw, 1, "{:?}", dominance.eigenvalues);
    assert!(mdl.sources_raw >= 2, "{:?}", mdl.eigenvalues);
}

#[test]
fn noise_alone_closes_the_squelch_and_reports_no_peaks() {
    let geometry = kraken();
    let manifold = Manifold::ideal(geometry.clone());
    let r = covariance(&mut scene(&geometry, &[], 31), SNAPSHOTS);
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    let mut report = DoaReport::default();
    doa.estimate(&manifold, &r, SNAPSHOTS as f64, 3.0, &mut report)
        .unwrap();
    assert!(!report.squelch_open);
    assert_eq!(report.peak_count, 0);
    assert!(report.eig_ratio_db < 1.0);
    let mut likelihood = [1.0f32; LIKELIHOOD_POINTS];
    assert!(
        !doa.likelihood(&manifold, &r, &report, &mut likelihood)
            .unwrap()
    );
    assert!(likelihood.iter().all(|&v| v == 1.0));
    assert_eq!(doa.spectrum().len(), 360);
}

#[test]
fn two_peaks_come_strongest_first() {
    let geometry = kraken();
    let r = covariance(
        &mut scene(&geometry, &[(40.0, 10.0), (200.0, 4.0)], 33),
        SNAPSHOTS,
    );
    let report = estimate(&geometry, &DoaConfig::default(), &r);
    assert_eq!(report.peak_count, 2, "{report:?}");
    assert!(
        error_deg(report.peaks[0].azimuth_deg, 40.0) < 2.0,
        "{report:?}"
    );
    assert!(
        error_deg(report.peaks[1].azimuth_deg, 200.0) < 2.0,
        "{report:?}"
    );
    assert!(report.peaks[0].power > report.peaks[1].power);
    for estimator in [Estimator::Capon, Estimator::RootMusic, Estimator::Esprit] {
        let single = DoaConfig {
            max_peaks: 1,
            ..with(estimator)
        };
        let report = estimate(&geometry, &single, &r);
        assert_eq!(report.sources, 2, "{estimator:?}");
        assert_eq!(report.peak_count, 1, "{estimator:?}");
        assert!(
            error_deg(report.peaks[0].azimuth_deg, 40.0) < 6.0,
            "{estimator:?}: {report:?}"
        );
    }
}

#[test]
fn refinement_reaches_a_tenth_of_a_degree_off_grid() {
    let geometry = kraken();
    let r = covariance(&mut scene(&geometry, &[(37.37, 20.0)], 35), SNAPSHOTS);
    let found = primary(&estimate(&geometry, &DoaConfig::default(), &r)).azimuth_deg;
    assert!(error_deg(found, 37.37) < 0.1, "{found}");
}

fn rates_at(geometry: &Geometry, azimuth_deg: f64) -> Vec<f64> {
    let mut rates = vec![0.0; geometry.len()];
    phase_rates(
        geometry.positions(),
        FREQ,
        Direction::horizon(azimuth_deg),
        &mut rates,
    );
    rates
}

#[test]
fn crb_sigma_matches_the_empirical_spread() {
    let geometry = kraken();
    let manifold = Manifold::ideal(geometry.clone());
    let config = DoaConfig {
        sources: SourceCount::Fixed(1),
        squelch: None,
        ..DoaConfig::default()
    };
    let mut doa = Doa::new(&manifold, &config, FREQ).unwrap();
    let mut report = DoaReport::default();
    let snapshots = 1000;
    let errors: Vec<f64> = (0..200)
        .map(|trial| {
            let r = covariance(
                &mut scene(&geometry, &[(37.0, 0.0)], 1000 + trial),
                snapshots,
            );
            doa.estimate(&manifold, &r, snapshots as f64, 0.0, &mut report)
                .unwrap();
            wrap_deg(primary(&report).azimuth_deg - 37.0)
        })
        .collect();
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    let spread = (errors.iter().map(|e| (e - mean) * (e - mean)).sum::<f64>()
        / (errors.len() - 1) as f64)
        .sqrt();
    let bound = crb_sigma_rad(&rates_at(&geometry, 37.0), 1.0, 5, snapshots as f64).to_degrees();
    let ratio = spread / bound;
    assert!((0.7..1.5).contains(&ratio), "{spread} vs {bound}");
}

#[test]
fn calibration_error_widens_sigma() {
    let geometry = kraken();
    let r = covariance(&mut scene(&geometry, &[(37.0, 20.0)], 41), SNAPSHOTS);
    let manifold = Manifold::ideal(geometry.clone());
    let peak = primary(&estimate_with(
        &manifold,
        &DoaConfig::default(),
        &r,
        SNAPSHOTS,
        5.0,
    ));
    let spread = rate_spread(&rates_at(&geometry, peak.azimuth_deg));
    let model = (5f64.to_radians() / spread.sqrt()).to_degrees();
    assert!(
        f64::from(peak.sigma_deg) >= model,
        "{} vs {model}",
        peak.sigma_deg
    );
    let tight = primary(&estimate_with(
        &manifold,
        &DoaConfig::default(),
        &r,
        SNAPSHOTS,
        0.0,
    ));
    assert!(tight.sigma_deg < peak.sigma_deg);
    assert!(f64::from(tight.sigma_deg) >= SIGMA_FLOOR_DEG);
}

#[test]
fn confidence_is_low_for_noise_and_high_for_a_clean_source() {
    let geometry = kraken();
    let open = DoaConfig {
        squelch: None,
        ..DoaConfig::default()
    };
    let noise = covariance(&mut scene(&geometry, &[], 43), SNAPSHOTS);
    let report = estimate(&geometry, &open, &noise);
    assert!(report.squelch_open);
    assert!(primary(&report).confidence < 0.1, "{report:?}");
    let clean = covariance(&mut scene(&geometry, &[(137.0, 20.0)], 44), SNAPSHOTS);
    let report = estimate(&geometry, &open, &clean);
    assert!(primary(&report).confidence > 0.9, "{report:?}");
}

#[test]
fn fit_drops_with_phase_errors() {
    let geometry = kraken();
    let clean = covariance(&mut scene(&geometry, &[(137.0, 20.0)], 45), SNAPSHOTS);
    let report = estimate(&geometry, &DoaConfig::default(), &clean);
    let fit_clean = report.fit;
    assert!(fit_clean > 0.98, "{fit_clean}");
    assert!((primary(&report).fit - fit_clean).abs() < 1e-3);
    let mut skewed = scene(&geometry, &[(137.0, 20.0)], 45);
    skewed.lane_phase_deg = vec![0.0, 52.0, -31.0, 44.0, -38.0];
    let r = covariance(&mut skewed, SNAPSHOTS);
    let report = estimate(&geometry, &DoaConfig::default(), &r);
    assert!(
        report.fit <= fit_clean - 0.05,
        "{} vs {fit_clean}",
        report.fit
    );
}

#[test]
fn ula_mirror_is_reported_and_halves_confidence() {
    let geometry = ula(4, 90.0);
    let r = covariance(&mut scene(&geometry, &[(30.0, 20.0)], 47), SNAPSHOTS);
    let peak = primary(&estimate(&geometry, &DoaConfig::default(), &r));
    assert!(error_deg(peak.azimuth_deg, 30.0) < 1.0, "{peak:?}");
    assert!(error_deg(peak.mirror_deg.unwrap(), 150.0) < 1.0, "{peak:?}");
    assert_eq!(peak.ambiguity, 0.5);
    assert!(peak.confidence <= 0.5);
}

#[test]
fn ula_front_side_keeps_the_front_bearing_only() {
    let geometry = ula(4, 90.0);
    let r = covariance(&mut scene(&geometry, &[(30.0, 20.0)], 49), SNAPSHOTS);
    let front = DoaConfig {
        ula_side: UlaSide::Front,
        ..DoaConfig::default()
    };
    let manifold = Manifold::ideal(geometry.clone());
    let doa = Doa::new(&manifold, &front, FREQ).unwrap();
    assert_eq!(doa.grid().points(), 181);
    let peak = primary(&estimate(&geometry, &front, &r));
    assert!(error_deg(peak.azimuth_deg, 30.0) < 1.0, "{peak:?}");
    assert_eq!(peak.mirror_deg, None);
    assert_eq!(peak.ambiguity, 0.0);
    let back = DoaConfig {
        ula_side: UlaSide::Back,
        ..front
    };
    let peak = primary(&estimate(&geometry, &back, &r));
    assert!(error_deg(peak.azimuth_deg, 150.0) < 1.0, "{peak:?}");
    for estimator in [Estimator::RootMusic, Estimator::Esprit] {
        for (side, want) in [(front, 30.0), (back, 150.0)] {
            let config = DoaConfig { estimator, ..side };
            let peak = primary(&estimate(&geometry, &config, &r));
            assert!(
                error_deg(peak.azimuth_deg, want) < 1.0,
                "{estimator:?}: {peak:?}"
            );
            assert_eq!(peak.mirror_deg, None);
        }
    }
}

#[test]
fn aliasing_halves_the_confidence_of_a_single_source() {
    let geometry = kraken();
    let manifold = Manifold::ideal(geometry.clone());
    let doubled = 2.0 * FREQ;
    let mut aliased = ArrayScene::new(geometry, doubled, RATE)
        .with_noise_db(0.0)
        .with_seed(71)
        .with_source(broadband(Direction::horizon(137.0), 20.0));
    let r = covariance(&mut aliased, SNAPSHOTS);
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), doubled).unwrap();
    assert!(doa.alias().aliased);
    let mut report = DoaReport::default();
    doa.estimate(&manifold, &r, SNAPSHOTS as f64, 3.0, &mut report)
        .unwrap();
    let peak = primary(&report);
    assert_eq!(report.sources, 1);
    assert_eq!(peak.ambiguity, 0.5);
    assert_eq!(peak.mirror_deg, None);
    assert!(peak.confidence <= 0.5, "{peak:?}");
}

#[test]
fn doa_moves_between_threads() {
    fn sendable<T: Send>() {}
    sendable::<Doa>();
}

fn likelihood_of(
    geometry: &Geometry,
    azimuth: f64,
    seed: u64,
) -> ([f32; LIKELIHOOD_POINTS], DoaReport) {
    let manifold = Manifold::ideal(geometry.clone());
    let r = covariance(&mut scene(geometry, &[(azimuth, 10.0)], seed), SNAPSHOTS);
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    let mut report = DoaReport::default();
    doa.estimate(
        &manifold,
        &r,
        SNAPSHOTS as f64,
        NO_CAL_SIGMA_DEG,
        &mut report,
    )
    .unwrap();
    let mut out = [0.0f32; LIKELIHOOD_POINTS];
    assert!(doa.likelihood(&manifold, &r, &report, &mut out).unwrap());
    (out, report)
}

fn maxima(values: &[f32; LIKELIHOOD_POINTS]) -> Vec<usize> {
    (0..LIKELIHOOD_POINTS)
        .filter(|&i| {
            let left = values[(i + LIKELIHOOD_POINTS - 1) % LIKELIHOOD_POINTS];
            let right = values[(i + 1) % LIKELIHOOD_POINTS];
            values[i] > left && values[i] >= right
        })
        .collect()
}

#[test]
fn likelihood_curvature_matches_sigma() {
    let (out, report) = likelihood_of(&kraken(), 37.0, 51);
    let top = maxima(&out)
        .into_iter()
        .max_by(|&a, &b| out[a].total_cmp(&out[b]))
        .unwrap();
    assert!(error_deg(top as f64, 37.0) <= 1.0);
    let step = 1f64.to_radians();
    let second = f64::from(out[top + 1] - 2.0 * out[top] + out[top - 1]) / (step * step);
    let sigma = f64::from(primary(&report).sigma_deg).to_radians();
    let measured = -1.0 / second;
    assert!(
        (measured - sigma * sigma).abs() < 0.1 * sigma * sigma,
        "{measured} vs {}",
        sigma * sigma
    );
}

#[test]
fn likelihood_is_bimodal_on_a_ula() {
    let (out, _) = likelihood_of(&ula(4, 90.0), 30.0, 53);
    let mut peaks = maxima(&out);
    peaks.sort_by(|&a, &b| out[b].total_cmp(&out[a]));
    let mut top: Vec<f64> = peaks.iter().take(2).map(|&i| i as f64).collect();
    top.sort_by(f64::total_cmp);
    assert!(error_deg(top[0], 30.0) <= 1.0, "{top:?}");
    assert!(error_deg(top[1], 150.0) <= 1.0, "{top:?}");
}

#[test]
fn likelihood_never_drops_below_the_floor() {
    let (out, _) = likelihood_of(&kraken(), 200.0, 55);
    let floor = LIKELIHOOD_FLOOR.ln();
    assert!(out.iter().all(|&v| v >= floor));
    assert!(out.contains(&floor));
    let mut bytes = [0u8; LIKELIHOOD_POINTS];
    quantize_likelihood(&out, &mut bytes);
    assert!(bytes[199..=201].contains(&255));
    assert!(bytes.contains(&0));
}

#[test]
fn elevation_is_found_on_a_raised_centre_element() {
    let ring = uca_with_chord(5, 0.5);
    let mut positions: Vec<Vec3> = ring.positions().to_vec();
    positions.push(Vec3::new(0.0, 0.0, 0.3 * wavelength()));
    let geometry = Geometry::explicit(&positions).unwrap();
    let mut raised = ArrayScene::new(geometry.clone(), FREQ, RATE)
        .with_noise_db(0.0)
        .with_seed(57)
        .with_source(broadband(Direction::new(60.0, 30.0), 20.0));
    let r = covariance(&mut raised, SNAPSHOTS);
    let config = DoaConfig {
        elevation: Some(ElevationSpan::default()),
        ..DoaConfig::default()
    };
    let peak = primary(&estimate(&geometry, &config, &r));
    assert!(error_deg(peak.azimuth_deg, 60.0) < 2.0, "{peak:?}");
    assert!((peak.elevation_deg - 30.0).abs() < 2.0, "{peak:?}");
    assert!(peak.sigma_el_deg > 0.0);
}

#[test]
fn elevation_is_refused_on_a_line() {
    let manifold = Manifold::ideal(ula(4, 90.0));
    let config = DoaConfig {
        elevation: Some(ElevationSpan::default()),
        ..DoaConfig::default()
    };
    assert!(matches!(
        Doa::new(&manifold, &config, FREQ),
        Err(DoaError::Unsupported(ELEVATION_NEEDS_2D))
    ));
}

fn warp(element: usize, azimuth_deg: f64) -> Complex<f32> {
    let phase = 0.9 * (2.0 * azimuth_deg.to_radians() + 1.3 * element as f64).sin();
    Complex::from_polar(1.0, phase as f32)
}

#[test]
fn measured_table_corrects_a_distorted_array() {
    let geometry = kraken();
    let mut warped = scene(&geometry, &[(100.0, 20.0)], 59);
    warped.distortion = Some(warp);
    let r = covariance(&mut warped, SNAPSHOTS);
    let ideal = primary(&estimate(&geometry, &DoaConfig::default(), &r));
    assert!(error_deg(ideal.azimuth_deg, 100.0) > 5.0, "{ideal:?}");
    let table = warped.distortion_table(&[FREQ], 1.0).unwrap();
    let manifold = Manifold::measured(geometry, Arc::new(table)).unwrap();
    let measured = primary(&estimate_with(
        &manifold,
        &DoaConfig::default(),
        &r,
        SNAPSHOTS,
        NO_CAL_SIGMA_DEG,
    ));
    assert!(error_deg(measured.azimuth_deg, 100.0) < 1.0, "{measured:?}");
}

#[test]
fn configure_scalar_changes_in_place() {
    let geometry = ula(6, 90.0);
    let manifold = Manifold::ideal(geometry.clone());
    let r = covariance(&mut scene(&geometry, &[(20.0, 20.0)], 61), SNAPSHOTS);
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    for estimator in [
        Estimator::Capon,
        Estimator::RootMusic,
        Estimator::Esprit,
        Estimator::Bartlett,
    ] {
        assert_eq!(
            doa.configure(&manifold, &with(estimator)),
            Ok(Reconfigure::InPlace)
        );
        let mut report = DoaReport::default();
        doa.estimate(&manifold, &r, SNAPSHOTS as f64, 3.0, &mut report)
            .unwrap();
        assert!(
            error_deg(primary(&report).azimuth_deg, 20.0) < 1.5,
            "{estimator:?}"
        );
        assert_eq!(doa.config().estimator, estimator);
    }
    let coarse = DoaConfig {
        azimuth_step_deg: 2.0,
        ..DoaConfig::default()
    };
    assert_eq!(doa.configure(&manifold, &coarse), Ok(Reconfigure::NeedsNew));
    let front = DoaConfig {
        ula_side: UlaSide::Front,
        ..DoaConfig::default()
    };
    assert_eq!(doa.configure(&manifold, &front), Ok(Reconfigure::NeedsNew));
    let mut half = Doa::new(&manifold, &front, FREQ).unwrap();
    let back = DoaConfig {
        ula_side: UlaSide::Back,
        ..front
    };
    assert_eq!(half.configure(&manifold, &back), Ok(Reconfigure::InPlace));
    let mut report = DoaReport::default();
    half.estimate(&manifold, &r, SNAPSHOTS as f64, 3.0, &mut report)
        .unwrap();
    assert!(
        error_deg(primary(&report).azimuth_deg, 160.0) < 1.0,
        "{report:?}"
    );
    let refused = DoaConfig {
        max_peaks: 9,
        ..DoaConfig::default()
    };
    assert_eq!(
        half.configure(&manifold, &refused),
        Err(DoaError::Unsupported(PEAKS_OUT_OF_RANGE))
    );
    assert_eq!(half.config().ula_side, UlaSide::Back);
}

#[test]
fn retune_rebuilds_the_grid_and_keeps_bearings() {
    let geometry = kraken();
    let manifold = Manifold::ideal(geometry.clone());
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    let alias = doa.retune(&manifold, 2.0 * FREQ).unwrap();
    assert!(alias.aliased);
    let shifted = 400e6;
    let alias = doa.retune(&manifold, shifted).unwrap();
    assert!(!alias.aliased);
    assert_eq!(doa.freq_hz(), shifted);
    let mut moved = ArrayScene::new(geometry, shifted, RATE)
        .with_noise_db(0.0)
        .with_seed(63)
        .with_source(broadband(Direction::horizon(250.0), 15.0));
    let r = covariance(&mut moved, SNAPSHOTS);
    let mut report = DoaReport::default();
    doa.estimate(&manifold, &r, SNAPSHOTS as f64, 3.0, &mut report)
        .unwrap();
    assert!(error_deg(primary(&report).azimuth_deg, 250.0) < 1.0);
    assert!(matches!(
        doa.retune(&manifold, f64::NAN),
        Err(DoaError::Manifold(ManifoldError::Frequency))
    ));
    let other = Manifold::ideal(uca_with_chord(5, 0.4));
    assert_eq!(
        doa.configure(&other, &DoaConfig::default()),
        Ok(Reconfigure::NeedsNew)
    );
    assert_eq!(
        doa.retune(&other, FREQ),
        Err(DoaError::Unsupported(ARRAY_CHANGED))
    );
    assert_eq!(
        doa.estimate(&other, &r, SNAPSHOTS as f64, 3.0, &mut report),
        Err(DoaError::Unsupported(ARRAY_CHANGED))
    );
    let table = scene(manifold.geometry(), &[], 1)
        .distortion_table(&[FREQ], 5.0)
        .unwrap();
    let measured = Manifold::measured(manifold.geometry().clone(), Arc::new(table)).unwrap();
    assert_eq!(
        doa.configure(&measured, &DoaConfig::default()),
        Ok(Reconfigure::NeedsNew)
    );
}

#[test]
fn a_wrong_or_empty_matrix_is_an_error() {
    let manifold = Manifold::ideal(kraken());
    let mut doa = Doa::new(&manifold, &DoaConfig::default(), FREQ).unwrap();
    let mut report = DoaReport::default();
    let small = CMat::identity(4).unwrap();
    assert_eq!(
        doa.estimate(&manifold, &small, 10.0, 3.0, &mut report),
        Err(DoaError::Linalg(LinalgError::Order(4)))
    );
    let empty = CMat::zeros(5).unwrap();
    assert_eq!(
        doa.estimate(&manifold, &empty, 10.0, 3.0, &mut report),
        Err(DoaError::Linalg(LinalgError::NotPositiveDefinite(0)))
    );
    let mut broken = CMat::identity(5).unwrap();
    broken.set(1, 2, Complex::new(f32::NAN, 0.0));
    assert_eq!(
        doa.estimate(&manifold, &broken, 10.0, 3.0, &mut report),
        Err(DoaError::Linalg(LinalgError::NonFinite))
    );
}

fn coherent(geometry: &Geometry, first: f64, second: f64, seed: u64) -> ArrayScene {
    let direct = SceneSource::new(
        Direction::horizon(first),
        0.0,
        SceneSignal::Tone { offset_hz: 2e3 },
    );
    let bounce = SceneSource {
        direction: Direction::horizon(second),
        copy_of: Some(SceneCopy {
            source: 0,
            amplitude: 0.9,
            phase_deg: 60.0,
        }),
        ..direct
    };
    ArrayScene::new(geometry.clone(), FREQ, 1e5)
        .with_source(direct)
        .with_source(bounce)
        .with_noise_db(-25.0)
        .with_seed(seed)
}

#[test]
fn smoothing_separates_coherent_sources_on_a_ula() {
    let geometry = ula(8, 180.0);
    let r = covariance(&mut coherent(&geometry, 70.0, 100.0, 65), 8192);
    let raw = estimate(&geometry, &DoaConfig::default(), &r);
    assert_eq!(raw.sources, 1);
    let smoothed = DoaConfig {
        smoothing: 2,
        forward_backward: true,
        ..DoaConfig::default()
    };
    let report = estimate(&geometry, &smoothed, &r);
    assert_eq!(report.order, 6);
    assert_eq!(report.peak_count, 2, "{report:?}");
    let found = sorted_azimuths(&report);
    assert!(error_deg(found[0], 70.0) < 2.0, "{found:?}");
    assert!(error_deg(found[1], 100.0) < 2.0, "{found:?}");
    let rooted = DoaConfig {
        estimator: Estimator::RootMusic,
        ..smoothed
    };
    let found = sorted_azimuths(&estimate(&geometry, &rooted, &r));
    assert!(error_deg(found[0], 70.0) < 2.0, "{found:?}");
    assert!(error_deg(found[1], 100.0) < 2.0, "{found:?}");
}

#[test]
fn smoothing_separates_coherent_sources_on_a_uca() {
    let geometry = uca_beta(8, 2.0);
    let r = covariance(&mut coherent(&geometry, 40.0, 150.0, 67), 8192);
    let smoothed = DoaConfig {
        smoothing: 2,
        forward_backward: true,
        ..DoaConfig::default()
    };
    let report = estimate(&geometry, &smoothed, &r);
    assert_eq!(report.order, 5);
    assert_eq!(report.peak_count, 2, "{report:?}");
    let found = sorted_azimuths(&report);
    assert!(error_deg(found[0], 40.0) < 2.0, "{found:?}");
    assert!(error_deg(found[1], 150.0) < 2.0, "{found:?}");
    let esprit = DoaConfig {
        estimator: Estimator::Esprit,
        ..smoothed
    };
    let found = sorted_azimuths(&estimate(&geometry, &esprit, &r));
    assert!(error_deg(found[0], 40.0) < 2.0, "{found:?}");
    assert!(error_deg(found[1], 150.0) < 2.0, "{found:?}");
}

#[test]
fn forward_backward_on_an_even_uca_keeps_the_bearing() {
    let geometry = uca_with_chord(6, 0.4);
    let r = covariance(&mut scene(&geometry, &[(40.0, 10.0)], 69), SNAPSHOTS);
    let config = DoaConfig {
        forward_backward: true,
        ..DoaConfig::default()
    };
    let found = primary(&estimate(&geometry, &config, &r)).azimuth_deg;
    assert!(error_deg(found, 40.0) < 1.0, "{found}");
    assert!(matches!(
        Doa::new(&Manifold::ideal(kraken()), &config, FREQ),
        Err(DoaError::Unsupported(FB_NEEDS_SYMMETRY))
    ));
}

#[test]
fn a_two_element_circle_runs_as_a_line() {
    let pair = Manifold::ideal(Geometry::uca(0.2, 2, 90.0, Winding::Clockwise).unwrap());
    let doa = Doa::new(&pair, &with(Estimator::RootMusic), FREQ).unwrap();
    assert_eq!(doa.mode_bias_deg(), None);
    assert!(Doa::new(&pair, &with(Estimator::Music), FREQ).is_ok());
}
