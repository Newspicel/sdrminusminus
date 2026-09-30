#[cfg(feature = "gpu-fft")]
#[test]
fn keeps_the_gpu_only_when_1_3x_faster() {
    use std::time::Duration;

    use super::keeps_gpu;

    let ms = Duration::from_millis;
    assert!(keeps_gpu(ms(10), ms(14)));
    assert!(keeps_gpu(ms(30), ms(120)));
    assert!(!keeps_gpu(ms(10), ms(12)));
    assert!(!keeps_gpu(ms(20), ms(10)));
}

#[cfg(feature = "gpu-fft")]
mod on_gpu {
    use std::{
        f64::consts::TAU,
        sync::{Arc, atomic::Ordering},
        time::Duration,
    };

    use num_complex::Complex;
    use sdrmm_channels::passive_radar::{
        CafBackend, CpiJob, CpuCaf, CubeOut, RadarCtx, RadarPlan as StagePlan, plan,
    };
    use sdrmm_wire::{
        GpuUse,
        radar::{ClutterMethod, ClutterParams, Illuminator, PassiveRadarParams, ReferenceCleaning},
    };

    use super::super::{super::worker::Shared, GpuBackend, Offer, decide};
    use crate::array::radar::{CafBackendKind, select_backend};

    type C32 = Complex<f32>;

    const KRAKEN_RATE: f64 = 2_400_000.0;
    const ELEMENTS: usize = 5;
    const ECHO_DELAY: usize = 40;
    const ECHO_HZ: f64 = 60.0;
    const TOLERANCE: f32 = 2e-4;

    fn stage(clutter: ClutterParams, illuminator: Illuminator) -> StagePlan {
        let params = PassiveRadarParams {
            illuminator,
            clutter,
            aoa: false,
            reference: ReferenceCleaning::Off,
            gpu: GpuUse::Auto,
            ..PassiveRadarParams::default()
        };
        let ctx = RadarCtx {
            sample_rate: KRAKEN_RATE,
            center_hz: if illuminator == Illuminator::Dab {
                220e6
            } else {
                100e6
            },
            elements: ELEMENTS,
            positions_m: Vec::new(),
            manifold: None,
            tuned_together: true,
        };
        plan(&ctx, &params).expect("plan")
    }

    fn uniform(seed: u64) -> impl FnMut() -> f32 {
        let mut state = seed | 1;
        move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5
        }
    }

    fn job(stage: &StagePlan, seed: u64) -> CpiJob {
        let shape = stage.shape;
        let width = shape.window();
        let rate = stage.front.radar_rate;
        let mut next = uniform(seed);
        let mut job = CpiJob::new(shape.lanes + 1, width);
        job.front_suppression_db = [7.5; 15];
        let mut phase = 0.0f32;
        let mut drive = 0.0f32;
        for sample in job.lane_mut(0) {
            drive = 0.995 * drive + 0.1 * next();
            phase += drive;
            *sample = C32::from_polar(1.0, phase);
        }
        let reference = job.lane(0).to_vec();
        for lane in 1..=shape.lanes {
            let gain = C32::from_polar(3.0, lane as f32);
            for (n, sample) in job.lane_mut(lane).iter_mut().enumerate() {
                let delayed = |delay: usize| {
                    n.checked_sub(delay)
                        .map_or(C32::default(), |at| reference[at])
                };
                let turn = (TAU * ECHO_HZ * n as f64 / rate) as f32;
                let direct = reference[n] * gain + delayed(3) * C32::new(0.4, -0.2);
                let echo = delayed(ECHO_DELAY) * C32::from_polar(0.03, turn);
                *sample = direct + echo + C32::new(next(), next()) * 0.05;
            }
        }
        job
    }

    fn on_the_cpu(stage: &StagePlan, job: &CpiJob) -> CubeOut {
        let mut out = CubeOut::new(&stage.shape);
        CpuCaf::new(stage)
            .expect("cpu")
            .run(job, &mut out)
            .expect("cpu cpi");
        out
    }

    fn backend(stage: &StagePlan, shared: &Arc<Shared>) -> GpuBackend {
        let cpu = Box::new(CpuCaf::new(stage).expect("cpu"));
        GpuBackend::new(stage, cpu, shared)
            .map_err(|(_, reason)| reason)
            .expect("GPU adapter")
    }

    fn worst_error(want: &CubeOut, got: &CubeOut) -> f32 {
        let peak = want.cube.iter().map(|cell| cell.norm()).fold(0.0, f32::max);
        let worst = want
            .cube
            .iter()
            .zip(&got.cube)
            .map(|(a, b)| (a - b).norm())
            .fold(0.0, f32::max);
        worst / peak
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn gpu_matches_cpu_within_tolerance() {
        let methods = [
            (ClutterParams::default(), Illuminator::Fm),
            (
                ClutterParams {
                    method: ClutterMethod::EcaSliding,
                    doppler_taps: 1,
                    ..ClutterParams::default()
                },
                Illuminator::Fm,
            ),
            (
                ClutterParams {
                    taper: false,
                    doppler_taps: 2,
                    ..ClutterParams::default()
                },
                Illuminator::Fm,
            ),
            (
                ClutterParams {
                    method: ClutterMethod::Nlms,
                    ..ClutterParams::default()
                },
                Illuminator::Fm,
            ),
            (ClutterParams::default(), Illuminator::Dab),
        ];
        for (clutter, illuminator) in methods {
            let stage = stage(clutter, illuminator);
            let job = Arc::new(job(&stage, 11));
            let want = on_the_cpu(&stage, &job);
            let shared = Arc::new(Shared::default());
            let mut gpu = backend(&stage, &shared);
            let mut got = CubeOut::new(&stage.shape);
            for _ in 0..2 {
                gpu.run_shared(&job, &mut got).expect("gpu cpi");
            }
            assert!(
                gpu.gpu(),
                "{illuminator:?} {clutter:?} fell back to the CPU"
            );
            assert_eq!(shared.gpu_failures.load(Ordering::Relaxed), 0);
            let error = worst_error(&want, &got);
            assert!(error < TOLERANCE, "{illuminator:?} {clutter:?}: {error}");
            assert_eq!(got.unsuppressed_groups, want.unsuppressed_groups);
            for (lane, (a, b)) in got
                .suppression_db
                .iter()
                .zip(&want.suppression_db)
                .enumerate()
            {
                assert!((a - b).abs() < 0.1, "{clutter:?} lane {lane}: {a} vs {b}");
            }
        }
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn a_gpu_error_recomputes_on_cpu() {
        let stage = stage(ClutterParams::default(), Illuminator::Fm);
        let job = Arc::new(job(&stage, 5));
        let want = on_the_cpu(&stage, &job);
        let shared = Arc::new(Shared::default());
        let mut backend = backend(&stage, &shared);
        let mut got = CubeOut::new(&stage.shape);
        backend.run_shared(&job, &mut got).expect("gpu cpi");
        assert!(backend.gpu());
        backend.sabotage();
        got.cube.fill(C32::default());
        backend.run_shared(&job, &mut got).expect("recomputed cpi");
        assert!(!backend.gpu());
        assert_eq!(backend.threads(), 0);
        assert_eq!(shared.gpu_failures.load(Ordering::Relaxed), 1);
        assert_eq!(got.cube, want.cube);
        assert_eq!(got.suppression_db, want.suppression_db);
        backend.run_shared(&job, &mut got).expect("cpu cpi");
        assert_eq!(shared.gpu_failures.load(Ordering::Relaxed), 1);
    }

    #[test]
    #[ignore = "requires a GPU adapter"]
    fn auto_keeps_the_gpu_only_when_it_wins() {
        let stage = stage(ClutterParams::default(), Illuminator::Fm);
        let shared = Arc::new(Shared::default());
        let ms = Duration::from_millis;
        let races = [
            (Ok((ms(10), ms(20))), true),
            (Ok((ms(10), ms(12))), false),
            (Err("no answer".to_owned()), false),
        ];
        for (race, wins) in races {
            match decide(backend(&stage, &shared), race) {
                Offer::Gpu(kept) => assert!(wins && kept.gpu()),
                Offer::Cpu(kept) => assert!(!wins && !kept.gpu()),
            }
        }
        let (backend, kind) = select_backend(&stage, GpuUse::Auto, &shared).expect("backend");
        assert_eq!(backend.gpu(), kind == CafBackendKind::Gpu);
        let job = Arc::new(job(&stage, 3));
        let mut got = CubeOut::new(&stage.shape);
        let mut backend = backend;
        backend.run_shared(&job, &mut got).expect("cpi");
        assert!(worst_error(&on_the_cpu(&stage, &job), &got) < TOLERANCE);
    }
}
