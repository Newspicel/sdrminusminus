use std::f64::consts::TAU;

use num_complex::Complex;
use sdrmm_dsp::{array_sync::BinSolver, fft::FftPair};

use super::*;

const LEN: usize = 65_536;

fn gaussian(len: usize, seed: u64) -> Vec<Complex<f32>> {
    let mut state = seed.max(1);
    let mut uniform = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    (0..len)
        .map(|_| {
            let radius = (-2.0 * uniform().ln()).sqrt();
            let angle = TAU * uniform();
            Complex::new(
                (radius * angle.cos() * 0.1) as f32,
                (radius * angle.sin() * 0.1) as f32,
            )
        })
        .collect()
}

fn shaped(source: &[Complex<f32>], response: impl Fn(f64) -> Complex<f64>) -> Vec<Complex<f32>> {
    let len = source.len();
    let mut bins = source.to_vec();
    let mut fft = FftPair::new(len);
    fft.forward(&mut bins);
    for (bin, value) in bins.iter_mut().enumerate() {
        let nu = if bin < len / 2 {
            bin as f64 / len as f64
        } else {
            bin as f64 / len as f64 - 1.0
        };
        let gain = response(nu);
        let scaled = Complex::new(f64::from(value.re), f64::from(value.im)) * gain;
        *value = Complex::new(scaled.re as f32, scaled.im as f32);
    }
    fft.inverse_scaled(&mut bins);
    bins
}

fn band(nu: f64) -> Complex<f64> {
    if nu.abs() <= 0.4 {
        Complex::new(1.0, 0.0)
    } else {
        Complex::new(0.0, 0.0)
    }
}

fn run(
    corrector: &mut Corrector,
    lanes: &[&[Complex<f32>]],
    index: u64,
) -> (Vec<Vec<Complex<f32>>>, u64) {
    let mut out = vec![Vec::new(); lanes.len()];
    let mut first = None;
    let len = lanes[0].len();
    let mut at = 0;
    while at < len {
        let take = ALIGN_BLOCK.min(len - at);
        let chunk: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[at..at + take]).collect();
        corrector.push(&chunk, index + at as u64);
        corrector.with_corrected(|lanes, start| {
            first.get_or_insert(start);
            for (out, lane) in out.iter_mut().zip(lanes) {
                out.extend_from_slice(lane);
            }
        });
        corrector.consume();
        at += take;
    }
    (out, first.unwrap_or(0))
}

#[test]
fn identity_only_delays_by_the_group_delay() {
    let source = gaussian(LEN, 3);
    let mut corrector = Corrector::new(2);
    let (out, first) = run(&mut corrector, &[&source, &source], 10_000);
    assert_eq!(first, 10_000 - CORR_DELAY as u64);
    for (k, sample) in out[0].iter().enumerate().skip(CORR_TAPS) {
        let raw = (first + k as u64 - 10_000) as usize;
        assert!((sample - source[raw]).norm() < 1e-4, "sample {k}");
    }
    assert_eq!(out[0], out[1]);
}

#[test]
fn a_solved_set_puts_lanes_on_top_of_each_other() {
    let source = gaussian(LEN, 50);
    let reference = shaped(&source, band);
    let lane = shaped(&source, |nu| {
        band(nu) * Complex::from_polar(0.75, 73f64.to_radians() - TAU * nu * 0.37)
    });
    let mut solver = BinSolver::new(2, 1_024);
    let solution = solver.solve(&[&reference, &lane], 0.8, 0.9).expect("solve");
    let response = &solution.lanes[1];
    let mut set = CorrectionSet::identity(2);
    design_correction(
        CORR_FFT,
        CORR_TAPS,
        CORR_BETA,
        response.delay_frac,
        Complex::from_polar(1.0 / response.gain, -response.phase_rad),
        Some(&response.equaliser),
        &mut set.spectra[1],
    );
    let mut corrector = Corrector::new(2);
    assert!(corrector.swap(Box::new(set)).is_ok());
    let (out, _) = run(&mut corrector, &[&reference, &lane], 0);
    let settled = CORR_TAPS..out[0].len() - CORR_TAPS;
    let error: f64 = out[0][settled.clone()]
        .iter()
        .zip(&out[1][settled.clone()])
        .map(|(a, b)| f64::from((a - b).norm_sqr()))
        .sum();
    let power: f64 = out[0][settled]
        .iter()
        .map(|a| f64::from(a.norm_sqr()))
        .sum();
    let residual_db = 10.0 * (error / power).log10();
    assert!(residual_db < -40.0, "{residual_db} dB");
}

#[test]
fn a_reset_gates_the_filter_transient() {
    let source = gaussian(4 * CORR_HOP, 9);
    let mut corrector = Corrector::new(1);
    assert_eq!(corrector.transient(), CORR_TAPS);
    corrector.push(&[&source[..2 * CORR_HOP]], 0);
    assert_eq!(corrector.ready(), 2 * CORR_HOP);
    corrector.consume();
    assert_eq!(corrector.transient(), 0);
    corrector.push(&[&source[..CORR_HOP]], 5 * CORR_HOP as u64);
    assert_eq!(corrector.transient(), CORR_TAPS);
    assert_eq!(
        corrector.first_index(),
        5 * CORR_HOP as u64 - CORR_DELAY as u64
    );
    corrector.reset();
    assert_eq!(corrector.ready(), 0);
    assert_eq!(corrector.transient(), CORR_TAPS);
}

#[test]
fn a_set_of_the_wrong_shape_is_refused() {
    let mut corrector = Corrector::new(2);
    assert!(
        corrector
            .swap(Box::new(CorrectionSet::identity(3)))
            .is_err()
    );
    let mut short = CorrectionSet::identity(2);
    short.spectra[1].truncate(10);
    assert!(corrector.swap(Box::new(short)).is_err());
    assert!(corrector.active().fits(2));
}

#[test]
fn clearing_to_identity_needs_no_new_set() {
    let mut corrector = Corrector::new(2);
    let mut set = CorrectionSet::identity(2);
    set.spectra[1]
        .iter_mut()
        .for_each(|bin| *bin *= Complex::new(0.0, 1.0));
    assert!(corrector.swap(Box::new(set)).is_ok());
    corrector.clear_to_identity(7);
    assert_eq!(corrector.active().generation, 7);
    let view = corrector.view(48_000.0);
    assert!((view.response(1, 1_000.0) - Complex::new(1.0, 0.0)).norm() < 1e-5);
}

#[test]
fn a_stream_starting_at_index_zero_has_no_labels_before_it() {
    let source = gaussian(ALIGN_BLOCK, 21);
    let mut corrector = Corrector::new(1);
    corrector.push(&[&source], 0);
    assert_eq!(corrector.transient(), CORR_TAPS - CORR_DELAY);
    let (first, out) = corrector.with_corrected(|lanes, first| (first, lanes[0].to_vec()));
    assert_eq!(first, 0);
    assert_eq!(out.len(), corrector.ready() - CORR_DELAY);
    for (k, sample) in out.iter().enumerate().skip(CORR_TAPS) {
        assert!((sample - source[k]).norm() < 1e-4, "sample {k}");
    }
    corrector.consume();
    corrector.push(&[&source[..CORR_HOP]], ALIGN_BLOCK as u64);
    let next = corrector.with_corrected(|_, first| first);
    assert_eq!(next, first + out.len() as u64);
}
