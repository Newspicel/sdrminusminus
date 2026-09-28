use num_complex::Complex;
use sdrmm_dsp::beamform::WeightSet;
use sdrmm_dsp::correlator::FxCorrelator;
use sdrmm_dsp::linalg::{CMat, Eigen, HermitianEigen};
use sdrmm_dsp::polar::{Stokes, matched_weights};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const LANES: usize = 5;
const FFT: usize = 1_024;
const BLOCK: usize = 3_000;

fn lane(index: usize, len: usize) -> Vec<Complex<f32>> {
    let mut state = 0x9E37_79B9u32 ^ (index as u32 + 1).wrapping_mul(0x85EB_CA6B);
    (0..len)
        .map(|t| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = state as f32 / u32::MAX as f32 - 0.5;
            Complex::from_polar(1.0, 0.013 * t as f32) + Complex::new(noise, -noise)
        })
        .collect()
}

mod correlator {
    use super::*;

    #[test]
    fn correlator_push_band_and_delay_do_not_allocate() {
        let lanes: Vec<Vec<Complex<f32>>> = (0..LANES).map(|index| lane(index, BLOCK)).collect();
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        let short: Vec<&[Complex<f32>]> = lanes.iter().map(|lane| &lane[..77]).collect();
        let mut correlator = FxCorrelator::new(LANES, FFT, true).unwrap();
        correlator.push(&views).unwrap();
        let mut sink = 0.0f64;
        assert_no_alloc("fx correlator", || {
            for _ in 0..4 {
                sink += f64::from(correlator.push(&views).unwrap());
                sink += f64::from(correlator.push(&short).unwrap());
                for baseline in 0..correlator.baselines() {
                    sink += correlator.band(baseline, 100..900).unwrap().coherence;
                    sink += correlator.delay_samples(baseline, 0..FFT).unwrap();
                    sink += correlator.visibility(baseline, 512).unwrap().re;
                }
                sink += correlator.auto(2, 3).unwrap();
                correlator.clear_integration();
                correlator.push(&views).unwrap();
            }
            correlator.reset();
        });
        assert!(sink.is_finite());
    }
}

mod polar {
    use super::*;

    #[test]
    fn stokes_and_matched_weights_do_not_allocate() {
        let mut eigen = HermitianEigen::new(2).unwrap();
        let mut values = Eigen::new();
        let mut weights = WeightSet::zeros(2);
        let mut r = CMat::zeros(2).unwrap();
        let mut sink = 0.0f32;
        assert_no_alloc("polar", || {
            for step in 0..16 {
                let r_ab = Complex::from_polar(0.4, 0.1 * step as f32);
                r.set(0, 0, Complex::new(1.0, 0.0));
                r.set(1, 1, Complex::new(0.7, 0.0));
                r.set(0, 1, r_ab);
                r.set(1, 0, r_ab.conj());
                let stokes = Stokes::from_covariance(1.0, 0.7, r_ab);
                sink += stokes.degree() + stokes.angle_deg() + stokes.ellipticity_deg();
                matched_weights(&r, step % 2 == 0, &mut eigen, &mut values, &mut weights).unwrap();
                sink += weights.norm_sqr();
            }
        });
        assert!(sink.is_finite());
    }
}
