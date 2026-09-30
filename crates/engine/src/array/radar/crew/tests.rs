use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use num_complex::Complex;
use sdrmm_channels::passive_radar::{
    CafBackend, CpiJob, CpiStage, CpuCaf, CubeOut, RadarCtx, RadarPlan as StagePlan, RadarPod, plan,
};
use sdrmm_test_support::assert_no_alloc;
use sdrmm_wire::radar::{ClutterMethod, ClutterParams, PassiveRadarParams, ReferenceCleaning};

use super::*;

fn stage(method: ClutterMethod, doppler_taps: u32) -> StagePlan {
    let ctx = RadarCtx {
        sample_rate: 500_000.0,
        center_hz: 100e6,
        elements: 4,
        positions_m: Vec::new(),
        manifold: None,
        tuned_together: true,
    };
    let params = PassiveRadarParams {
        cpi_ms: 200,
        aoa: false,
        reference: ReferenceCleaning::Off,
        clutter: ClutterParams {
            method,
            doppler_taps,
            ..ClutterParams::default()
        },
        ..PassiveRadarParams::default()
    };
    plan(&ctx, &params).expect("plan")
}

fn job(stage: &StagePlan, seed: u64) -> Arc<CpiJob> {
    let lanes = stage.front.lanes.len();
    let window = stage.shape.window();
    let mut job = CpiJob::new(lanes, window);
    let mut state = seed | 1;
    let mut uniform = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let reference: Vec<Complex<f32>> = (0..window)
        .map(|_| Complex::new(uniform(), uniform()))
        .collect();
    job.lane_mut(0).copy_from_slice(&reference);
    for lane in 1..lanes {
        let gain = Complex::from_polar(0.8, lane as f32);
        for (sample, direct) in job.lane_mut(lane).iter_mut().zip(&reference) {
            *sample = direct * gain + Complex::new(uniform(), uniform()) * 0.1;
        }
    }
    job.front_suppression_db = [3.0; 15];
    Arc::new(job)
}

fn same_bits(want: &CubeOut, got: &CubeOut) -> bool {
    want.cube.len() == got.cube.len()
        && want
            .cube
            .iter()
            .zip(&got.cube)
            .all(|(a, b)| a.re.to_bits() == b.re.to_bits() && a.im.to_bits() == b.im.to_bits())
}

fn run_both(
    stage: &StagePlan,
    single: &mut CpuCaf,
    crew: &mut CrewCaf,
    seed: u64,
) -> (CubeOut, CubeOut) {
    let job = job(stage, seed);
    let mut want = CubeOut::new(&stage.shape);
    single.run(&job, &mut want).expect("single thread");
    let mut got = CubeOut::new(&stage.shape);
    crew.run_shared(&job, &mut got).expect("crew");
    assert_eq!(Arc::strong_count(&job), 1);
    (want, got)
}

fn wait_dead(crew: &CrewCaf, helper: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while crew.helper_alive(helper) {
        assert!(Instant::now() < deadline, "helper {helper} did not stop");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn crew_output_equals_single_thread_output() {
    for (method, taps) in [
        (ClutterMethod::EcaBatch, 0),
        (ClutterMethod::EcaSliding, 1),
        (ClutterMethod::Off, 0),
    ] {
        let stage = stage(method, taps);
        let shared = Arc::new(Shared::default());
        let mut single = CpuCaf::new(&stage).expect("single thread");
        let mut crew = CrewCaf::new(&stage, 3, &shared).expect("crew");
        assert_eq!(crew.threads(), 3);
        for seed in [7, 11, 13] {
            let (want, got) = run_both(&stage, &mut single, &mut crew, seed);
            assert!(same_bits(&want, &got), "{method:?} seed {seed}");
            assert_eq!(want.suppression_db, got.suppression_db, "{method:?}");
            assert_eq!(want.unsuppressed_groups, got.unsuppressed_groups);
        }
        assert_eq!(crew.threads(), 3);
        drop(crew);
        assert_eq!(shared.live.load(Ordering::Acquire), 0);
    }
}

#[test]
fn a_crew_cpi_does_not_allocate_on_the_coordinator() {
    let stage = stage(ClutterMethod::EcaBatch, 0);
    let shared = Arc::new(Shared::default());
    let mut crew = CrewCaf::new(&stage, 2, &shared).expect("crew");
    let job = job(&stage, 5);
    let mut out = CubeOut::new(&stage.shape);
    crew.run_shared(&job, &mut out).expect("warm up");
    assert_no_alloc("crew CPI", || {
        crew.run_shared(&job, &mut out).expect("crew CPI");
    });
}

#[test]
fn a_dead_helper_falls_back_to_single_thread_and_says_so() {
    let stage = stage(ClutterMethod::EcaBatch, 0);
    let shared = Arc::new(Shared::default());
    let mut single = CpuCaf::new(&stage).expect("single thread");
    let mut crew = CrewCaf::new(&stage, 2, &shared).expect("crew");
    let (want, got) = run_both(&stage, &mut single, &mut crew, 3);
    assert!(same_bits(&want, &got));
    assert_eq!(crew.threads(), 2);
    crew.poison(1, POISON_NOW);
    wait_dead(&crew, 1);
    let (want, got) = run_both(&stage, &mut single, &mut crew, 4);
    assert!(same_bits(&want, &got), "the CPI after the loss is complete");
    assert_eq!(crew.threads(), 0);
    let (want, got) = run_both(&stage, &mut single, &mut crew, 5);
    assert!(same_bits(&want, &got));
    drop(crew);
    assert_eq!(shared.live.load(Ordering::Acquire), 0);

    let crew = CrewCaf::new(&stage, 2, &shared).expect("crew");
    crew.poison(0, POISON_NOW);
    wait_dead(&crew, 0);
    let mut stage_run = CpiStage::new(&stage, Box::new(crew)).expect("stage");
    let mut pod = RadarPod::new(&stage);
    let outcome = stage_run.run_shared(&job(&stage, 6), &mut pod);
    assert!(outcome.failed.is_none());
    assert_eq!(pod.threads, 0, "the report says the crew is gone");
}

#[test]
fn a_helper_lost_inside_a_task_is_recomputed_on_one_thread() {
    let stage = stage(ClutterMethod::EcaSliding, 1);
    let shared = Arc::new(Shared::default());
    let mut single = CpuCaf::new(&stage).expect("single thread");
    let mut crew = CrewCaf::new(&stage, 3, &shared).expect("crew");
    let (want, got) = run_both(&stage, &mut single, &mut crew, 8);
    assert!(same_bits(&want, &got));
    crew.poison(2, POISON_ON_TASK);
    let (want, got) = run_both(&stage, &mut single, &mut crew, 9);
    assert!(same_bits(&want, &got), "the lost shard was recomputed");
    assert_eq!(crew.threads(), 0);
    let (want, got) = run_both(&stage, &mut single, &mut crew, 10);
    assert!(same_bits(&want, &got));
}
