use std::{sync::Arc, time::Instant};

use sdrmm_channels::passive_radar::{
    CafBackend, CpiJob, CpuCaf, CubeOut, RadarCtx, RadarPlan, plan,
};
use sdrmm_wire::radar::{Illuminator, PassiveRadarParams, ReferenceCleaning};

use super::{super::caf::GpuCaf, samples};

const KRAKEN_RATE: f64 = 2_400_000.0;
const ELEMENTS: usize = 5;
const RUNS: usize = 5;

fn kraken(center_hz: f64, illuminator: Illuminator) -> RadarPlan {
    let params = PassiveRadarParams {
        illuminator,
        reference: ReferenceCleaning::Off,
        aoa: false,
        ..PassiveRadarParams::default()
    };
    let ctx = RadarCtx {
        sample_rate: KRAKEN_RATE,
        center_hz,
        elements: ELEMENTS,
        positions_m: Vec::new(),
        tuned_together: true,
    };
    plan(&ctx, &params).unwrap()
}

fn job(stage: &RadarPlan) -> CpiJob {
    let mut job = CpiJob::new(stage.shape.lanes + 1, stage.shape.window());
    job.samples = samples(job.samples.len(), 0x5EED);
    job
}

fn median_ms(mut run: impl FnMut()) -> f64 {
    run();
    let mut times: Vec<f64> = (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            run();
            started.elapsed().as_secs_f64() * 1e3
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[RUNS / 2]
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_caf() {
    let context = Arc::clone(crate::gpu::context().expect("hardware GPU required"));
    for (name, stage) in [
        ("fm", kraken(100e6, Illuminator::Fm)),
        ("dab", kraken(220e6, Illuminator::Dab)),
    ] {
        let job = job(&stage);
        let mut out = CubeOut::new(&stage.shape);
        let mut cpu = CpuCaf::new(&stage).unwrap();
        let mut gpu = GpuCaf::new(Arc::clone(&context), &stage).unwrap();
        let cpu_ms = median_ms(|| cpu.run(&job, &mut out).unwrap());
        let gpu_ms = median_ms(|| gpu.run(&job, &mut out).unwrap());
        println!("radar_caf/{name},cpu_ms={cpu_ms:.2},gpu_ms={gpu_ms:.2}");
    }
}
