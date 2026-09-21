use sdrmm_channels::coherent::RangeDopplerSurface;
use sdrmm_test_support::assert_no_alloc;

use super::*;

struct Echo {
    released: Arc<AtomicBool>,
}

impl CoherentRx for Echo {
    fn descriptor() -> &'static CoherentDescriptor {
        PassiveRadarProcessor::descriptor()
    }
    fn new(_: CoherentCtx, _: &CoherentParams) -> Result<Self, ChannelError> {
        Ok(Self {
            released: Arc::new(AtomicBool::new(true)),
        })
    }
    fn apply(&mut self, _: &CoherentParams) -> Result<(), ChannelError> {
        Ok(())
    }
    fn process(&mut self, lanes: &[&[Complex<f32>]], output: &mut CoherentOutputs) {
        while !self.released.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        output.surface = Some(RangeDopplerSurface {
            cells: lanes[0].iter().map(|value| value.re as u8).collect(),
            ..Default::default()
        });
    }
}

fn worker(released: Arc<AtomicBool>) -> RadarWorker {
    RadarWorker::start(
        CoherentCtx {
            lanes: 2,
            sample_rate: 1000.0,
            center_hz: 100e6,
        },
        Box::new(Echo { released }),
        64,
    )
    .unwrap()
}

fn receive(worker: &mut RadarWorker) -> CoherentOutputs {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = CoherentOutputs::default();
    while output.surface.is_none() {
        worker.poll(&mut output);
        assert!(Instant::now() < deadline, "radar completion timed out");
        thread::sleep(Duration::from_millis(1));
    }
    output
}

#[test]
fn ragged_input_reaches_the_worker_without_allocating_or_changing_samples() {
    let mut worker = worker(Arc::new(AtomicBool::new(true)));
    let input: Vec<_> = (0..64)
        .map(|index| Complex::new(index as f32, 0.0))
        .collect();
    let mut output = CoherentOutputs::default();
    for range in [0..1, 1..18, 18..18, 18..64] {
        assert_no_alloc("radar submission", || {
            worker.process(&[&input[range.clone()], &input[range]], &mut output);
        });
    }
    let output = receive(&mut worker);
    assert_eq!(output.surface.unwrap().cells, (0..64).collect::<Vec<u8>>());
    assert_eq!(worker.dropped_samples, 0);
}

#[test]
fn a_retune_discards_old_completions_and_partial_intervals() {
    let released = Arc::new(AtomicBool::new(false));
    let mut worker = worker(released.clone());
    let old = vec![Complex::new(1.0, 0.0); 80];
    let mut output = CoherentOutputs::default();
    worker.process(&[&old, &old], &mut output);
    worker.retuned(101e6);
    let new = vec![Complex::new(5.0, 0.0); 64];
    worker.process(&[&new, &new], &mut output);
    released.store(true, Ordering::Release);
    let output = receive(&mut worker);
    assert_eq!(output.surface.unwrap().cells, vec![5; 64]);
}

#[test]
fn overload_is_bounded_and_counted() {
    let released = Arc::new(AtomicBool::new(false));
    let mut worker = worker(released.clone());
    let input = vec![Complex::new(1.0, 0.0); 64 * 4];
    let mut output = CoherentOutputs::default();
    worker.process(&[&input, &input], &mut output);
    released.store(true, Ordering::Release);
    assert_eq!(worker.dropped_samples, 64);
    assert_eq!(worker.available.len(), 0);
    assert!(worker.pending.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut completed = 0;
    while worker.available.len() < JOBS {
        output.reset();
        worker.poll(&mut output);
        completed += usize::from(output.surface.is_some());
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(completed, JOBS);
    assert_no_alloc("radar recovered submission", || {
        worker.process(&[&input[..64], &input[..64]], &mut output);
    });
    assert_eq!(receive(&mut worker).surface.unwrap().cells, vec![1; 64]);
}

#[cfg(feature = "gpu-fft")]
#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_pipeline() {
    use crate::gpu::benchmarks::{measure, samples};
    let ctx = CoherentCtx {
        lanes: 2,
        sample_rate: 2_000_000.0,
        center_hz: 100e6,
    };
    let params = CoherentParams::PassiveRadar(sdrmm_wire::PassiveRadarParams::default());
    let mut cpu = PassiveRadarProcessor::new(ctx, &params).unwrap();
    let mut cpi = 0;
    let mut gpu = PassiveRadarProcessor::with_correlation(ctx, &params, |cpu| {
        cpi = cpu.cpi();
        let correlation = acceleration::Correlation::new(cpu);
        assert!(
            correlation.is_accelerated(),
            "benchmark requires GPU acceleration"
        );
        Box::new(correlation)
    })
    .unwrap();
    let reference = samples(cpi, 0x12345678);
    let surveillance = samples(cpi, 0x87654321);
    let lanes = [&reference[..], &surveillance[..]];
    let mut output = CoherentOutputs::default();
    measure("radar_pipeline/cpu", || {
        output.reset();
        cpu.process(std::hint::black_box(&lanes), &mut output);
        std::hint::black_box(&output);
    });
    measure("radar_pipeline/gpu", || {
        output.reset();
        gpu.process(std::hint::black_box(&lanes), &mut output);
        std::hint::black_box(&output);
    });
}

#[test]
fn worker_preserves_the_complete_radar_report() {
    let ctx = CoherentCtx {
        lanes: 2,
        sample_rate: 100_000.0,
        center_hz: 100e6,
    };
    let params = CoherentParams::PassiveRadar(sdrmm_wire::PassiveRadarParams {
        cpi_ms: 10,
        max_range_bins: 64,
        doppler_span_hz: 2000.0,
        ..Default::default()
    });
    let mut cpu = PassiveRadarProcessor::new(ctx, &params).unwrap();
    let receiver = PassiveRadarProcessor::new(ctx, &params).unwrap();
    let mut worker = RadarWorker::start(ctx, Box::new(receiver), 1000).unwrap();
    let input: Vec<_> = (0..1000)
        .map(|index| {
            Complex::new(
                (index * 71 % 257) as f32 / 257.0,
                (index * 37 % 127) as f32 / 127.0,
            )
        })
        .collect();
    let surveillance: Vec<_> = (0..1000)
        .map(|index| {
            if index < 17 {
                Complex::default()
            } else {
                input[index - 17]
                    * Complex::from_polar(0.5, std::f32::consts::TAU * 3.0 * index as f32 / 1000.0)
            }
        })
        .collect();
    let mut expected = CoherentOutputs::default();
    cpu.process(&[&input, &surveillance], &mut expected);
    let mut output = CoherentOutputs::default();
    for (reference, surveillance) in input.chunks(37).zip(surveillance.chunks(37)) {
        worker.process(&[reference, surveillance], &mut output);
    }
    assert_eq!(receive(&mut worker), expected);
}
