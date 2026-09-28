use sdrmm_dsp::sweep::{HeadingSample, LevelSample, SweepBearing, SweepConfig, SweepEstimator};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn heading(t: f64) -> f64 {
    let turning = (t - 1.0).clamp(0.0, 3.0);
    10.0 + 90.0 * turning
}

fn level(t: f64) -> f32 {
    let offset = (heading(t - 0.1) - 123.0).to_radians();
    (-60.0 + 100.0 * ((1.0 + offset.cos()) / 2.0).log10().max(-0.15)) as f32
}

#[test]
fn sweep_pushes_do_not_allocate() {
    let mut estimator = SweepEstimator::new(SweepConfig::default());
    let mut out = SweepBearing::default();
    let mut closed = 0;
    assert_no_alloc("sweep estimator", || {
        for tick in 0..120 {
            let t = f64::from(tick) * 0.05;
            estimator.push_heading(HeadingSample {
                t_s: t,
                heading_deg: heading(t) % 360.0,
                sigma_deg: 2.0,
            });
            let sample = LevelSample {
                t_s: t + 0.025,
                level_db: level(t + 0.025),
            };
            if estimator.push_level(sample, &mut out).is_some() {
                closed += 1;
            }
        }
    });
    assert_eq!(closed, 1);
    assert!((out.bearing_deg - 123.0).abs() < 3.0);
}
