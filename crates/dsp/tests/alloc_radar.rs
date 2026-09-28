use num_complex::Complex;
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

type C32 = Complex<f32>;

struct Rng(u64);

impl Rng {
    fn uniform(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn complex(&mut self) -> C32 {
        C32::new(self.uniform() - 0.5, self.uniform() - 0.5)
    }

    fn noise(&mut self, len: usize) -> Vec<C32> {
        (0..len).map(|_| self.complex()).collect()
    }
}

mod radar {
    mod batch {
        use sdrmm_dsp::radar::batch::{BatchKernel, BatchShape, DopplerTaper, WeightsAt};
        use sdrmm_dsp::radar::wiener::{GroupPlan, WeightTable};

        use super::super::*;

        #[test]
        fn kernel_does_not_allocate() {
            let shape = BatchShape::new(32, 260, 73, 2, 14, 2).unwrap();
            let plan = GroupPlan::split(32, 8, 0, 1, true, false).unwrap();
            let table = WeightTable::new(&shape, &plan).unwrap();
            let mut kernel = BatchKernel::new(shape);
            let mut rng = Rng(1);
            let window = rng.noise(shape.window());
            let m = shape.fft_len;
            let mut spectrum = vec![C32::default(); m];
            let mut model = vec![C32::default(); m];
            let mut product = vec![C32::default(); m];
            let mut gates = vec![C32::default(); shape.gates];
            let mut series = rng.noise(shape.batches);
            let mut taper = vec![0.0f32; shape.batches];
            DopplerTaper::Hann.fill(&mut taper);
            let mut sink = 0.0f64;
            assert_no_alloc("batch kernel", || {
                for batch in 0..shape.batches {
                    kernel
                        .reference(&window, batch, &mut spectrum, &mut model)
                        .unwrap();
                    kernel
                        .surveillance(&window, batch, &spectrum, &mut product)
                        .unwrap();
                    kernel
                        .residual(&product, &model, table.at(1, batch), &mut gates)
                        .unwrap();
                    kernel
                        .residual(&product, &model, WeightsAt::none(), &mut gates)
                        .unwrap();
                    sink += shape.energy(&window, batch);
                }
                kernel.doppler(&mut series, &taper).unwrap();
            });
            assert!(sink.is_finite() && gates[0].is_finite());
        }
    }

    mod wiener {
        use sdrmm_dsp::radar::batch::{BatchKernel, BatchShape};
        use sdrmm_dsp::radar::wiener::{GroupPlan, GroupSums, WeightTable, WienerSolver};

        use super::super::*;

        #[test]
        fn solve_does_not_allocate() {
            let shape = BatchShape::new(32, 128, 40, 2, 10, 2).unwrap();
            let plan = GroupPlan::split(32, 8, 2, 1, false, true).unwrap();
            let mut solver = WienerSolver::new(shape, &plan, 1e-4).unwrap();
            let mut sums = GroupSums::new(&shape, &plan);
            let mut table = WeightTable::new(&shape, &plan).unwrap();
            let mut kernel = BatchKernel::new(shape);
            let mut rng = Rng(2);
            let reference = rng.noise(shape.window());
            let surveillance = rng.noise(shape.window());
            let m = shape.fft_len;
            let mut spectrum = vec![C32::default(); m];
            let mut model = vec![C32::default(); m];
            let mut products = vec![C32::default(); 2 * m];
            let energy = [1.0f64, 2.0];
            let mut gates = vec![C32::default(); shape.gates];
            let mut unsuppressed = 0;
            assert_no_alloc("wiener solve", || {
                sums.clear();
                for batch in 0..shape.batches {
                    kernel
                        .reference(&reference, batch, &mut spectrum, &mut model)
                        .unwrap();
                    for lane in 0..2 {
                        let product = &mut products[lane * m..(lane + 1) * m];
                        kernel
                            .surveillance(&surveillance, batch, &spectrum, product)
                            .unwrap();
                    }
                    solver
                        .accumulate(&mut sums, batch, &model, &products, &energy)
                        .unwrap();
                }
                unsuppressed += solver.solve(&sums, &mut table).unwrap().unsuppressed_groups;
                for batch in 0..shape.batches {
                    for lane in 0..2 {
                        let product = &products[lane * m..(lane + 1) * m];
                        kernel
                            .residual(product, &model, table.at(lane, batch), &mut gates)
                            .unwrap();
                    }
                }
            });
            assert_eq!(unsuppressed, 0);
            assert!(gates.iter().all(|gate| gate.is_finite()));
        }
    }

    mod nlms {
        use sdrmm_dsp::radar::cma::ReferenceCma;
        use sdrmm_dsp::radar::nlms::{BlockNlms, SurveillanceNlms};

        use super::super::*;

        #[test]
        fn cancellers_do_not_allocate() {
            let fs = 266_666.67;
            let mut nlms = SurveillanceNlms::new(14, 2, 0.05, fs).unwrap();
            let mut block = BlockNlms::new(14, 2, 0.05, fs).unwrap();
            let mut cma = ReferenceCma::new(16, 1e-3, fs).unwrap();
            let mut rng = Rng(3);
            let reference = rng.noise(4096);
            let surveillance = rng.noise(4096);
            let mut out = vec![C32::default(); 4096];
            let mut sink = 0.0f32;
            assert_no_alloc("cancellers", || {
                for chunk in [100usize, 1, 3995] {
                    nlms.process(&reference[..chunk], &surveillance[..chunk], &mut out);
                    block.process(&reference[..chunk], &surveillance[..chunk], &mut out);
                    cma.process(&reference[..chunk], &mut out);
                }
                sink += nlms.suppression_db() + block.suppression_db() + cma.gain_db();
                nlms.reset();
                block.reset();
                cma.reset();
            });
            assert!(sink.is_finite());
        }
    }

    mod cfar {
        use sdrmm_dsp::radar::cfar::{Cfar, CfarSpec, Hit};
        use sdrmm_dsp::radar::threshold::CfarStatistic;

        use super::super::*;

        fn spec(stat: CfarStatistic, plane: bool) -> CfarSpec {
            CfarSpec {
                stat,
                plane,
                guard_range: 2,
                train_range: 8,
                guard_doppler: 1,
                train_doppler: 4,
                alpha: 8.0,
                alpha_edge: 9.0,
                min_snr: 1.0,
                min_gate: 1,
                clutter_half_rows: 1,
            }
        }

        #[test]
        fn cfar_does_not_allocate() {
            let (rows, gates) = (64, 80);
            let mut rng = Rng(4);
            let mut power: Vec<f32> = (0..rows * gates).map(|_| rng.uniform() * 2.0).collect();
            for k in 0..40 {
                power[(k % rows) * gates + (k * 7) % gates] = 500.0;
            }
            let mut cfar = Cfar::new(spec(CfarStatistic::Ca, false), gates, rows).unwrap();
            let mut hits: Vec<Hit> = Vec::with_capacity(16);
            let mut truncated = 0;
            assert_no_alloc("cfar", || {
                for stat in [
                    CfarStatistic::Ca,
                    CfarStatistic::Os { rank: 0.75 },
                    CfarStatistic::Go,
                ] {
                    for plane in [false, true] {
                        cfar.set_spec(spec(stat, plane)).unwrap();
                        truncated += cfar.detect(&power, 4..60, &mut hits, 16).unwrap();
                    }
                }
            });
            assert!(truncated > 0);
            assert_eq!(hits.len(), 16);
        }
    }

    mod cluster {
        use sdrmm_dsp::radar::cfar::Hit;
        use sdrmm_dsp::radar::cluster::{Cluster, Clusterer};

        use super::super::*;

        #[test]
        fn clustering_does_not_allocate() {
            let (rows, gates) = (64, 80);
            let mut power = vec![1.0f32; rows * gates];
            let mut hits = Vec::new();
            for k in 0..60u32 {
                let (row, gate) = (4 + (k % 20) * 3, 2 + (k / 20) * 20 + k % 4);
                hits.push(Hit {
                    row,
                    gate,
                    power: 20.0 + k as f32,
                    noise: 1.0,
                });
                power[row as usize * gates + gate as usize] = 20.0 + k as f32;
            }
            let mut clusterer = Clusterer::new(gates, rows).unwrap();
            let mut clusters: Vec<Cluster> = Vec::with_capacity(32);
            let mut dropped = 0;
            assert_no_alloc("clustering", || {
                for _ in 0..3 {
                    dropped += clusterer
                        .cluster(&hits, &power, 8, &mut clusters, 32)
                        .unwrap();
                    dropped += clusterer
                        .cluster(&hits[..10], &power, 8, &mut clusters, 32)
                        .unwrap();
                }
            });
            assert!(dropped > 0);
            assert!(!clusters.is_empty());
        }
    }

    mod aoa {
        use sdrmm_dsp::manifold::{Geometry, LIGHT_SPEED_M_S, Winding};
        use sdrmm_dsp::radar::aoa::Beamscan;

        use super::super::*;

        #[test]
        fn aoa_does_not_allocate() {
            let lambda = LIGHT_SPEED_M_S / 100e6;
            let geometry = Geometry::uca(0.4 * lambda, 4, 0.0, Winding::Clockwise).unwrap();
            let mut scan = Beamscan::new(geometry.positions(), lambda, 0.5, Some(90.0)).unwrap();
            let mut rng = Rng(6);
            let snapshots: Vec<Vec<C32>> = (0..5).map(|_| rng.noise(4)).collect();
            let mut sink = 0.0f32;
            assert_no_alloc("aoa", || {
                let views = [
                    snapshots[0].as_slice(),
                    snapshots[1].as_slice(),
                    snapshots[2].as_slice(),
                ];
                sink += scan
                    .estimate(&views, 20.0)
                    .map_or(0.0, |aoa| aoa.azimuth_deg);
            });
            assert!(sink.is_finite());
        }
    }

    mod track {
        use sdrmm_dsp::radar::track::{Measurement, Tracker, TrackerConfig};

        use super::super::*;

        #[test]
        fn tracker_does_not_allocate() {
            let config = TrackerConfig {
                wavelength_m: 3.0,
                confirm_hits: 3,
                confirm_window: 5,
                coast_looks: 2,
                max_accel: 30.0,
                gate: 11.8,
                jerk: 5.0,
                range_resolution_m: 1124.0,
                doppler_resolution_hz: 3.0,
            };
            let mut tracker = Tracker::new(config).unwrap();
            let mut assigned = Vec::with_capacity(128);
            let mut ended = Vec::with_capacity(64);
            let look = |step: u32, count: usize| -> Vec<Measurement> {
                (0..count)
                    .map(|k| Measurement {
                        range_m: 5_000.0 + 3_000.0 * k as f64 - 50.0 * f64::from(step),
                        doppler_hz: 33.0,
                        snr: 50.0 + k as f64,
                        aoa: None,
                    })
                    .collect()
            };
            let looks: Vec<Vec<Measurement>> = (0..12).map(|step| look(step, 40)).collect();
            let flood = look(0, 200);
            tracker.update(0.5, &flood, &mut assigned, &mut ended);
            let mut views = 0;
            assert_no_alloc("tracker", || {
                for measurements in &looks {
                    tracker.update(0.5, measurements, &mut assigned, &mut ended);
                    tracker.for_each_confirmed(|_| views += 1);
                }
                for _ in 0..4 {
                    tracker.update(0.5, &[], &mut assigned, &mut ended);
                }
                tracker.reset(&mut ended);
            });
            assert!(views > 0);
        }
    }
}
