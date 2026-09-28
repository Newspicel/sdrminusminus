use num_complex::Complex;
use sdrmm_dsp::beamform::{
    BlockSolver, Constraints, LaneNoise, WeightRamp, WeightSet, beam_metrics, pattern,
};
use sdrmm_dsp::covariance::SampleCovariance;
use sdrmm_dsp::linalg::CMat;
use sdrmm_dsp::manifold::{Direction, Geometry, GridSpec, Manifold, SteeringGrid, Winding};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const FREQ: f64 = 433.92e6;
const LANES: usize = 5;
const LEN: usize = 4_096;

fn lane(index: usize, len: usize) -> Vec<Complex<f32>> {
    let mut state = 0x9E37_79B9u32 ^ (index as u32 + 1).wrapping_mul(0x85EB_CA6B);
    (0..len)
        .map(|t| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = state as f32 / u32::MAX as f32 - 0.5;
            let phase = 0.021 * t as f32 + 1.3 * index as f32;
            Complex::from_polar(1.0, phase) + Complex::new(noise, 0.5 * noise)
        })
        .collect()
}

fn steering(manifold: &Manifold, azimuth: f64) -> [Complex<f32>; LANES] {
    let mut out = [Complex::new(0.0, 0.0); LANES];
    manifold.steer(FREQ, Direction::horizon(azimuth), &mut out);
    out
}

#[test]
fn block_solvers_ramp_and_metrics_do_not_allocate() {
    let manifold = Manifold::ideal(Geometry::uca(0.35, LANES, 0.0, Winding::Clockwise).unwrap());
    let ring = SteeringGrid::new(&manifold, GridSpec::ring(1.0), FREQ).unwrap();
    let lanes: Vec<Vec<Complex<f32>>> = (0..LANES).map(|index| lane(index, LEN)).collect();
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut covariance = SampleCovariance::new(LANES).unwrap();
    let mut r = CMat::zeros(LANES).unwrap();
    let mut solver = BlockSolver::new(LANES).unwrap();
    let mut noise = LaneNoise::new(LANES).unwrap();
    let mut ramp = WeightRamp::new(LANES);
    let mut weights = WeightSet::zeros(LANES);
    let mut previous = WeightSet::unit(LANES, 0);
    let mut constraints = Constraints::new(LANES);
    let mut shape = [0.0f32; 360];
    let mut out = Vec::with_capacity(4 * LEN);
    let steer = steering(&manifold, 137.0);
    let nulls = [steering(&manifold, 20.0), steering(&manifold, 250.0)];
    let mut sink = 0.0f64;
    assert_no_alloc("block solvers, ramp and metrics", || {
        for _ in 0..3 {
            out.clear();
            covariance.decay(0.7);
            covariance.accumulate(&views);
            covariance.matrix(&mut r);
            noise.push(&views).unwrap();
            solver.das(&steer, &mut weights).unwrap();
            solver.mrc(&r, noise.noise(), &mut weights).unwrap();
            weights.align_phase_to(&previous);
            previous.clone_from(&weights);
            sink += f64::from(solver.mvdr(&r, &steer, 0.1, &mut weights).unwrap());
            constraints.clear();
            constraints.push(&steer, Complex::new(1.0, 0.0)).unwrap();
            for null in &nulls {
                constraints.push(null, Complex::new(0.0, 0.0)).unwrap();
            }
            sink += f64::from(
                solver
                    .lcmv(Some(&r), &constraints, 0.1, &mut weights)
                    .unwrap(),
            );
            sink += f64::from(solver.lcmv(None, &constraints, 0.0, &mut weights).unwrap());
            ramp.set_target(&weights, 1_000);
            ramp.apply(&views, &mut out).unwrap();
            pattern(&manifold, FREQ, &weights, &ring, &mut shape).unwrap();
            pattern(&manifold, FREQ * 1.01, &weights, &ring, &mut shape).unwrap();
            let metrics = beam_metrics(&r, &weights, noise.noise(), Some(0), &constraints).unwrap();
            sink += f64::from(metrics.output_power);
            sink += f64::from(solver.slc(&r, 0, &[1, 2, 3], &mut weights).unwrap());
        }
    });
    assert!(sink.is_finite());
    assert_eq!(out.len(), LEN);
    assert!(shape.iter().all(|value| value.is_finite()));
}
