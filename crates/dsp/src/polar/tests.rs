use super::*;
use crate::testutil::XorShift32;

fn gaussian(rng: &mut XorShift32) -> Complex<f32> {
    let mut sum = Complex::new(0.0f32, 0.0);
    for _ in 0..6 {
        sum += Complex::new(rng.next_f32(), rng.next_f32());
    }
    sum * 0.5
}

fn signal(len: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut rng = XorShift32(seed);
    (0..len).map(|_| gaussian(&mut rng)).collect()
}

fn stokes_of(a: &[Complex<f32>], b: &[Complex<f32>]) -> Stokes {
    let len = a.len() as f64;
    let mut r_aa = 0.0f64;
    let mut r_bb = 0.0f64;
    let mut r_ab = Complex::new(0.0f64, 0.0);
    for (x, y) in a.iter().zip(b) {
        r_aa += f64::from(x.norm_sqr());
        r_bb += f64::from(y.norm_sqr());
        let product = x * y.conj();
        r_ab += Complex::new(f64::from(product.re), f64::from(product.im));
    }
    Stokes::from_covariance(
        (r_aa / len) as f32,
        (r_bb / len) as f32,
        Complex::new((r_ab.re / len) as f32, (r_ab.im / len) as f32),
    )
}

fn matrix(r_aa: f32, r_bb: f32, r_ab: Complex<f32>) -> CMat {
    let mut r = CMat::zeros(2).unwrap();
    r.set(0, 0, Complex::new(r_aa, 0.0));
    r.set(1, 1, Complex::new(r_bb, 0.0));
    r.set(0, 1, r_ab);
    r.set(1, 0, r_ab.conj());
    r
}

#[test]
fn horizontal_linear_is_q_plus_one() {
    let a = signal(4_096, 3);
    let b = vec![Complex::new(0.0, 0.0); a.len()];
    let stokes = stokes_of(&a, &b);
    assert!((stokes.q / stokes.i - 1.0).abs() < 1e-6);
    assert!(stokes.angle_deg().abs() < 1e-3);
    assert!((stokes.degree() - 1.0).abs() < 1e-5);
    assert_eq!(stokes.hand(false), Hand::Linear);
}

#[test]
fn linear_45_degrees_is_u_plus_one() {
    let a = signal(4_096, 5);
    let stokes = stokes_of(&a, &a);
    assert!((stokes.u / stokes.i - 1.0).abs() < 1e-6);
    assert!((stokes.angle_deg() - 45.0).abs() < 1e-3);
    assert!(stokes.ellipticity_deg().abs() < 1e-3);
    let vertical = stokes_of(&vec![Complex::new(0.0, 0.0); a.len()], &a);
    assert!((vertical.angle_deg().abs() - 90.0).abs() < 1e-3);
}

#[test]
fn quadrature_feeds_give_v_plus_one() {
    let a = signal(4_096, 7);
    let b: Vec<Complex<f32>> = a.iter().map(|x| x * Complex::new(0.0, -1.0)).collect();
    let stokes = stokes_of(&a, &b);
    assert!((stokes.v / stokes.i - 1.0).abs() < 1e-5);
    assert!((stokes.ellipticity_deg() - 45.0).abs() < 0.1);
    assert_eq!(stokes.hand(false), Hand::Right);
    assert_eq!(stokes.hand(true), Hand::Left);
    let left: Vec<Complex<f32>> = a.iter().map(|x| x * Complex::new(0.0, 1.0)).collect();
    assert_eq!(stokes_of(&a, &left).hand(false), Hand::Left);
}

#[test]
fn degree_matches_the_eigen_form() {
    let mut rng = XorShift32(0x1234_5678);
    let mut eigen = HermitianEigen::new(2).unwrap();
    let mut values = Eigen::new();
    for _ in 0..200 {
        let x = [gaussian(&mut rng), gaussian(&mut rng)];
        let y = [gaussian(&mut rng), gaussian(&mut rng)];
        let r_aa = x[0].norm_sqr() + y[0].norm_sqr();
        let r_bb = x[1].norm_sqr() + y[1].norm_sqr();
        let r_ab = x[0] * x[1].conj() + y[0] * y[1].conj();
        let stokes = Stokes::from_covariance(r_aa, r_bb, r_ab);
        eigen.solve(&matrix(r_aa, r_bb, r_ab), &mut values).unwrap();
        let (small, large) = (values.values()[0], values.values()[1]);
        let eigen_form = (large - small) / (large + small);
        assert!(
            (stokes.degree() - eigen_form).abs() < 1e-5,
            "{} vs {eigen_form}",
            stokes.degree()
        );
    }
}

#[test]
fn unpolarised_noise_has_low_degree() {
    let a = signal(100_000, 11);
    let b = signal(100_000, 13);
    let stokes = stokes_of(&a, &b);
    assert!(stokes.degree() < 0.05, "degree {}", stokes.degree());
}

#[test]
fn matched_weights_maximise_output_snr() {
    let mut eigen = HermitianEigen::new(2).unwrap();
    let mut values = Eigen::new();
    let noise = 0.01f32;
    for (angle_deg, phase_deg) in [
        (30.0f32, 0.0f32),
        (45.0, 90.0),
        (70.0, -40.0),
        (10.0, 170.0),
    ] {
        let jones = [
            Complex::new(angle_deg.to_radians().cos(), 0.0),
            Complex::from_polar(angle_deg.to_radians().sin(), phase_deg.to_radians()),
        ];
        let r_ab = jones[0] * jones[1].conj();
        let r = matrix(
            jones[0].norm_sqr() + noise,
            jones[1].norm_sqr() + noise,
            r_ab,
        );
        let mut weights = WeightSet::zeros(2);
        matched_weights(&r, false, &mut eigen, &mut values, &mut weights).unwrap();
        let gain = weights.response(&jones).norm_sqr();
        let snr = gain / (noise * weights.norm_sqr());
        let best = values.values()[1] / values.values()[0] - 1.0;
        let error_db = 10.0 * (snr / best).log10();
        assert!(error_db.abs() < 0.1, "{angle_deg}: {error_db} dB");
        let mut minor = WeightSet::zeros(2);
        matched_weights(&r, true, &mut eigen, &mut values, &mut minor).unwrap();
        assert!(minor.response(&jones).norm_sqr() < 1e-6);
    }
}

#[test]
fn matched_weights_keep_their_phase_between_updates() {
    let mut eigen = HermitianEigen::new(2).unwrap();
    let mut values = Eigen::new();
    let mut weights = WeightSet::unit(2, 0);
    let r = matrix(1.0, 0.5, Complex::from_polar(0.6, 0.4));
    matched_weights(&r, false, &mut eigen, &mut values, &mut weights).unwrap();
    let before = weights.clone();
    let nudged = matrix(1.0, 0.52, Complex::from_polar(0.61, 0.41));
    matched_weights(&nudged, false, &mut eigen, &mut values, &mut weights).unwrap();
    let overlap: Complex<f32> = before
        .as_slice()
        .iter()
        .zip(weights.as_slice())
        .map(|(old, new)| new * old.conj())
        .sum();
    assert!(overlap.arg().abs().to_degrees() < 2.0);
    let wide = CMat::zeros(3).unwrap();
    assert_eq!(
        matched_weights(&wide, false, &mut eigen, &mut values, &mut weights).err(),
        Some(LinalgError::Order(3))
    );
}
