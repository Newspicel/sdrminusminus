use num_complex::Complex;
use sdrmm_dsp::beamform::{
    Adaptation, BlockSolver, Cma, Constraints, Gsc, TdlCanceller, WeightRamp, WeightSet,
};
use sdrmm_dsp::covariance::SampleCovariance;
use sdrmm_dsp::linalg::{CMat, Qr};
use sdrmm_dsp::manifold::{Direction, Geometry, Manifold, Winding};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const FREQ: f64 = 433.92e6;
const LANES: usize = 5;
const LEN: usize = 4_096;

fn lane(index: usize, len: usize) -> Vec<Complex<f32>> {
    let mut state = 0x2545_F491u32 ^ (index as u32 + 1).wrapping_mul(0x9E37_79B9);
    (0..len)
        .map(|t| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = state as f32 / u32::MAX as f32 - 0.5;
            let phase = 0.017 * t as f32 + 0.9 * index as f32;
            Complex::from_polar(1.0, phase) + Complex::new(0.5 * noise, noise)
        })
        .collect()
}

fn steering(manifold: &Manifold, azimuth: f64) -> [Complex<f32>; LANES] {
    let mut out = [Complex::new(0.0, 0.0); LANES];
    manifold.steer(FREQ, Direction::horizon(azimuth), &mut out);
    out
}

#[test]
fn beamform_paths_do_not_allocate() {
    let manifold = Manifold::ideal(Geometry::uca(0.35, LANES, 0.0, Winding::Clockwise).unwrap());
    let lanes: Vec<Vec<Complex<f32>>> = (0..LANES).map(|index| lane(index, LEN)).collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut covariance = SampleCovariance::new(LANES).unwrap();
    let mut r = CMat::zeros(LANES).unwrap();
    let mut solver = BlockSolver::new(LANES).unwrap();
    let mut ramp = WeightRamp::new(LANES);
    let mut weights = WeightSet::zeros(LANES);
    let mut effective = WeightSet::zeros(LANES);
    let mut constraints = Constraints::new(LANES);
    let mut qr = Qr::new(LANES, 1).unwrap();
    let mut gsc = Gsc::new(LANES).unwrap();
    let mut nlms = TdlCanceller::new(0, &[1, 2], 16, Adaptation::Nlms { step: 0.05 }).unwrap();
    let mut rls = TdlCanceller::new(0, &[3], 8, Adaptation::Rls { forget: 0.999 }).unwrap();
    let mut cma = Cma::new(LANES, 0.05).unwrap();
    let mut out = Vec::with_capacity(LEN);
    let mut sink = 0.0f64;
    let mut run = |azimuth: f64| {
        covariance.accumulate(&views);
        covariance.matrix(&mut r);
        solver.mrc(&r, None, &mut weights).unwrap();
        cma.seed(&weights).unwrap();
        ramp.set_target(&weights, 512);
        out.clear();
        ramp.apply(&views, &mut out).unwrap();
        constraints.clear();
        constraints
            .push(&steering(&manifold, azimuth), Complex::new(1.0, 0.0))
            .unwrap();
        constraints
            .push(
                &steering(&manifold, azimuth + 120.0),
                Complex::new(0.0, 0.0),
            )
            .unwrap();
        sink += f64::from(
            solver
                .lcmv(Some(&r), &constraints, 0.1, &mut weights)
                .unwrap(),
        );
        gsc.set_constraints(&constraints, &mut qr).unwrap();
        out.clear();
        gsc.process(&views, &mut out).unwrap();
        gsc.weights(&mut effective);
        out.clear();
        nlms.process(&views, &mut out).unwrap();
        out.clear();
        rls.process(&views, &mut out).unwrap();
        out.clear();
        cma.process(&views, &mut out).unwrap();
        cma.weights(&mut effective);
        sink += f64::from(nlms.suppression_db() + rls.suppression_db());
        sink += f64::from(effective.norm_sqr());
    };
    run(30.0);
    assert_no_alloc("ramp, gsc, tdl, cma and block solves", || {
        for step in 0..3 {
            run(30.0 + 7.0 * f64::from(step));
        }
    });
    assert!(sink.is_finite());
    assert_eq!(
        gsc.resets() + cma.resets() + nlms.resets() + rls.resets(),
        0
    );
}
