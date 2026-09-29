use std::collections::VecDeque;
use std::f64::consts::TAU;
use std::sync::Arc;

use num_complex::Complex;
use sdrmm_channels::passive_radar::{
    CpiJob, CpiStage, CpuCaf, FrontStage, InLanes, JobSink, RadarCtx, RadarPod, fill_surface,
    fill_update, plan,
};
use sdrmm_test_support::CountingAlloc;
use sdrmm_wire::frame::{RangeDopplerOwned, SurfaceFrame};
use sdrmm_wire::radar::{
    CfarParams, ClutterMethod, ClutterParams, Illuminator, LIGHT_SPEED_M_S, PassiveRadarParams,
    RadarUpdate, ReferenceCleaning,
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

#[cfg(test)]
mod passive_radar {
    use super::*;
    use sdrmm_test_support::assert_no_alloc;

    type C32 = Complex<f32>;

    const BLOCK: usize = 16_384;
    const CENTER_HZ: f64 = 100e6;

    struct Pool {
        free: Vec<Arc<CpiJob>>,
        ready: VecDeque<Arc<CpiJob>>,
        submitted: usize,
    }

    impl Pool {
        fn new(lanes: usize, window: usize) -> Self {
            Self {
                free: (0..2)
                    .map(|_| Arc::new(CpiJob::new(lanes, window)))
                    .collect(),
                ready: VecDeque::with_capacity(4),
                submitted: 0,
            }
        }

        fn recycle(&mut self) {
            while let Some(job) = self.ready.pop_front() {
                self.free.push(job);
            }
        }
    }

    impl JobSink for Pool {
        fn take(&mut self) -> Option<Arc<CpiJob>> {
            self.free.pop()
        }

        fn submit(&mut self, job: Arc<CpiJob>) {
            self.submitted += 1;
            self.ready.push_back(job);
        }

        fn dropped(&mut self, unused: Option<Arc<CpiJob>>) {
            self.free.extend(unused);
        }
    }

    fn uniform(state: &mut u64) -> f64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        ((*state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn gaussian(state: &mut u64) -> C32 {
        let radius = (-uniform(state).ln()).sqrt() as f32;
        C32::from_polar(radius, (TAU * uniform(state)) as f32)
    }

    fn random_fm(len: usize, seed: u64) -> Vec<C32> {
        let mut state = seed;
        let (mut phase, mut drive) = (0.0f64, 0.0f64);
        (0..len)
            .map(|_| {
                drive = 0.995 * drive + 0.1 * (uniform(&mut state) - 0.5);
                phase += drive;
                C32::from_polar(1.0, phase as f32)
            })
            .collect()
    }

    fn positions() -> Vec<[f64; 3]> {
        let radius = 0.4 * LIGHT_SPEED_M_S / CENTER_HZ;
        let mut positions = vec![[0.0, 0.0, 0.0]];
        positions.extend((0..4).map(|element| {
            let (sin, cos) = (f64::from(element) * 90.0).to_radians().sin_cos();
            [radius * sin, radius * cos, 0.0]
        }));
        positions
    }

    fn steering(azimuth_deg: f64) -> Vec<C32> {
        let (sin, cos) = azimuth_deg.to_radians().sin_cos();
        let k = TAU * CENTER_HZ / LIGHT_SPEED_M_S;
        positions()
            .iter()
            .map(|p| C32::from_polar(1.0, (k * (p[0] * sin + p[1] * cos)) as f32))
            .collect()
    }

    fn scene(x: &[C32], elements: usize, rate: f64, target: bool) -> Vec<Vec<C32>> {
        let len = x.len();
        let direct = steering(200.0);
        let echo = steering(30.0);
        let mut state = 99u64;
        (0..elements)
            .map(|element| {
                (0..len)
                    .map(|n| {
                        let mut value = x[n] * direct[element] * 100.0 + gaussian(&mut state);
                        if target && element > 0 && n >= 35 {
                            let turn = C32::from_polar(1.0, (TAU * 30.0 * n as f64 / rate) as f32);
                            value += x[n - 35] * echo[element] * turn * 0.1;
                        }
                        value
                    })
                    .collect()
            })
            .collect()
    }

    fn dab_scene(frames: usize) -> Vec<Vec<C32>> {
        let clean = sdrmm_channels::synth::dab::ensemble(frames);
        let mut state = 7u64;
        let reference = clean
            .iter()
            .map(|&x| x + gaussian(&mut state) * 0.03)
            .collect();
        let surveillance = clean
            .iter()
            .map(|&x| x * C32::new(0.6, -0.8) + gaussian(&mut state) * 0.03)
            .collect();
        vec![reference, surveillance]
    }

    fn push(front: &mut FrontStage, pool: &mut Pool, lanes: &[Vec<C32>], start: usize, rate: f64) {
        let end = (start + BLOCK).min(lanes[0].len());
        let slices: [&[C32]; 16] = std::array::from_fn(|lane| {
            lanes
                .get(lane)
                .map_or(&[][..], |samples| &samples[start..end])
        });
        let input = InLanes {
            lanes: &slices[..lanes.len()],
            first_index: start as u64,
            unix_ns: (start as f64 / rate * 1e9) as u64,
            gap_before: false,
            phase_ready: true,
            generation: 1,
        };
        let stats = front.push(&input, pool);
        assert!(!stats.mismatched);
        assert_eq!(stats.overflowed_samples, 0);
        pool.recycle();
    }

    fn check_front(
        label: &str,
        ctx: &RadarCtx,
        params: &PassiveRadarParams,
        lanes: &[Vec<C32>],
        warm: usize,
    ) {
        let plan = plan(ctx, params).unwrap();
        let mut front = FrontStage::new(&plan).unwrap();
        let mut pool = Pool::new(plan.front.lanes.len(), plan.shape.window());
        let rate = ctx.sample_rate;
        let mut start = 0;
        while start < warm {
            push(&mut front, &mut pool, lanes, start, rate);
            start += BLOCK;
        }
        let warmed = pool.submitted;
        assert!(warmed >= 1, "{label}: no CPI during warm-up");
        assert_no_alloc(label, || {
            while start + BLOCK <= lanes[0].len() {
                push(&mut front, &mut pool, lanes, start, rate);
                start += BLOCK;
            }
        });
        assert!(pool.submitted > warmed, "{label}: no CPI while measured");
        let health = front.reference_health();
        if params.reference == ReferenceCleaning::DabRemod {
            assert!(health.locked, "{label}: {health:?}");
        }
    }

    #[test]
    fn front_stage_does_not_allocate_after_warm_up() {
        let rate = 500_000.0;
        let ctx = RadarCtx {
            sample_rate: rate,
            center_hz: CENTER_HZ,
            elements: 3,
            positions_m: Vec::new(),
            manifold: None,
            tuned_together: true,
        };
        let lanes = scene(&random_fm(1_600_000, 5), 3, rate, false);
        let cma = PassiveRadarParams {
            aoa: false,
            ..PassiveRadarParams::default()
        };
        check_front("CMA and ECA", &ctx, &cma, &lanes, 600_000);
        let nlms = PassiveRadarParams {
            clutter: ClutterParams {
                method: ClutterMethod::Nlms,
                ..ClutterParams::default()
            },
            ..cma
        };
        check_front("NLMS", &ctx, &nlms, &lanes, 600_000);
        let dab = PassiveRadarParams {
            illuminator: Illuminator::Dab,
            reference: ReferenceCleaning::DabRemod,
            cpi_ms: 100,
            clutter: ClutterParams {
                method: ClutterMethod::BlockNlms,
                ..ClutterParams::default()
            },
            ..cma
        };
        let dab_ctx = RadarCtx {
            sample_rate: 2_048_000.0,
            center_hz: 220e6,
            elements: 2,
            ..ctx
        };
        check_front("DAB remod", &dab_ctx, &dab, &dab_scene(5), 3 * 196_608);
    }

    #[test]
    fn cpi_stage_does_not_allocate_after_warm_up() {
        let rate = 250_000.0;
        let ctx = RadarCtx {
            sample_rate: rate,
            center_hz: CENTER_HZ,
            elements: 5,
            positions_m: positions(),
            manifold: None,
            tuned_together: true,
        };
        let params = PassiveRadarParams {
            reference: ReferenceCleaning::Off,
            cfar: CfarParams {
                min_doppler_hz: 15.0,
                ..CfarParams::default()
            },
            ..PassiveRadarParams::default()
        };
        let mut state = 5u64;
        let wideband: Vec<C32> = (0..1_020_000).map(|_| gaussian(&mut state)).collect();
        let lanes = scene(&wideband, 5, rate, true);
        let plan = plan(&ctx, &params).unwrap();
        let mut front = FrontStage::new(&plan).unwrap();
        let mut cpi = CpiStage::new(&plan, Box::new(CpuCaf::new(&plan).unwrap())).unwrap();
        let mut pool = Pool::new(plan.front.lanes.len(), plan.shape.window());
        let mut pod = RadarPod::new(&plan);
        let mut update = RadarUpdate::reserved();
        let mut surface = SurfaceFrame::RangeDoppler(RangeDopplerOwned::default());
        let mut runs = 0;
        let mut start = 0;
        while start + BLOCK <= lanes[0].len() {
            let end = start + BLOCK;
            let slices: Vec<&[C32]> = lanes.iter().map(|lane| &lane[start..end]).collect();
            let input = InLanes {
                lanes: &slices,
                first_index: start as u64,
                unix_ns: (start as f64 / rate * 1e9) as u64,
                gap_before: false,
                phase_ready: true,
                generation: 1,
            };
            front.push(&input, &mut pool);
            while let Some(job) = pool.ready.pop_front() {
                let mut step = || {
                    cpi.run(&job, &mut pod);
                    fill_update(&pod, &mut update);
                    fill_surface(&pod, &mut surface);
                };
                if runs < 5 {
                    step();
                } else {
                    assert_no_alloc("CPI stage", step);
                    assert_eq!(update.tracks.len(), 1);
                }
                runs += 1;
                pool.free.push(job);
            }
            start = end;
        }
        assert_eq!(runs, 8);
        assert!(!update.detections.is_empty());
    }
}
