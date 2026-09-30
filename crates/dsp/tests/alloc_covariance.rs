use num_complex::Complex;
use sdrmm_dsp::covariance::{
    CovarianceBank, CovarianceError, PhaseMode, SampleCovariance, forward_backward, load_diagonal,
    smooth, smooth_diagonal,
};
use sdrmm_dsp::linalg::CMat;
use sdrmm_dsp::manifold::{Geometry, Permutation, Winding};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn lane(index: usize, len: usize) -> Vec<Complex<f32>> {
    (0..len)
        .map(|t| {
            let phase = 0.013 * (t * (index + 3)) as f32 + index as f32;
            Complex::from_polar(1.0 + 0.1 * index as f32, phase)
        })
        .collect()
}

fn phase_mode() -> Result<PhaseMode, CovarianceError> {
    let geometry = Geometry::uca(0.35, 8, 0.0, Winding::Clockwise)
        .map_err(|_| CovarianceError::NotStructured)?;
    PhaseMode::new(&geometry, 433.92e6)
}

#[test]
fn bank_and_covariance_do_not_allocate() {
    let lanes: Vec<Vec<Complex<f32>>> = (0..8).map(|index| lane(index, 4096)).collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut covariance = SampleCovariance::new(8).unwrap();
    let mut bank = CovarianceBank::new(8, 256, 128).unwrap();
    let mut modes = phase_mode().unwrap();
    let mut r = CMat::zeros(8).unwrap();
    let mut out = CMat::zeros(8).unwrap();
    let mut smoothed = CMat::zeros(8).unwrap();
    let mut virtual_ = [Complex::new(0.0f32, 0.0); 16];
    let mut diagonal = [0.0f32; 16];
    let reversal = Permutation::reversal(8);
    let mut sink = 0.0f64;
    assert_no_alloc("covariance, transforms and bank", || {
        for _ in 0..3 {
            covariance.decay(0.9);
            sink += covariance.accumulate(&views) as f64;
            covariance.matrix(&mut r);
            sink += f64::from(load_diagonal(&mut r, 1e-3));
            forward_backward(&mut r, &reversal).unwrap();
            smooth(&r, &reversal, 3, true, &mut smoothed).unwrap();
            sink += smooth_diagonal(&[1.0; 8], 3, true, &mut diagonal) as f64;
            modes.transform(&r, &mut out).unwrap();
            modes.transform_vandermonde(&r, &mut out).unwrap();
            sink += modes.project(&lanes[0][..8], &mut virtual_).unwrap() as f64;
            modes.retune(400e6).unwrap();
            sink += f64::from(modes.mode_bias_deg());
            sink += f64::from(bank.push(&views, 0.95).unwrap());
            bank.group_matrix(10, 8, &mut out).unwrap();
            sink += f64::from(bank.bin_power(100));
            sink += covariance.effective_snapshots();
        }
    });
    assert!(sink.is_finite());
    assert!(covariance.matrix(&mut r));
}
