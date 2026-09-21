use std::hint::black_box;

use sdrmm_dsp::{
    SpectrumAnalyzer,
    caf::{Caf, Surface},
    eca::{Eca, EcaParams},
    subband::SubbandPlan,
};

use super::*;

mod compute;
mod spectrum;
mod wideband;

pub(crate) fn samples(size: usize, seed: u32) -> Vec<Complex<f32>> {
    let mut state = seed;
    (0..size)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            Complex::new(
                (state as i32) as f32 / i32::MAX as f32,
                (state.rotate_left(13) as i32) as f32 / i32::MAX as f32,
            )
        })
        .collect()
}

pub(crate) fn measure(name: &str, mut run: impl FnMut()) {
    for _ in 0..8 {
        run();
    }
    let mut timings = Vec::with_capacity(101);
    for _ in 0..101 {
        let start = Instant::now();
        run();
        timings.push(start.elapsed().as_secs_f64() * 1e6);
    }
    timings.sort_by(f64::total_cmp);
    println!(
        "{name},median_us={:.3},p95_us={:.3}",
        timings[50], timings[95]
    );
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_offload() {
    let context = Arc::new(Context::new().expect("hardware GPU required"));
    let info = context.device.adapter_info();
    println!(
        "adapter={},backend={:?},device={:?}",
        info.name, info.backend, info.device_type
    );
    if cfg!(target_os = "macos") {
        assert_eq!(info.backend, wgpu::Backend::Metal);
    }
    for size in [4096, 16_384, 65_536] {
        let input = samples(size, 0x1234567);
        let mut output = vec![0.0; size];
        let mut cpu = SpectrumAnalyzer::new(size);
        let mut gpu = Processor::new(context.clone(), size).unwrap();
        measure(&format!("spectrum/{size}/cpu"), || {
            cpu.power_db(black_box(&input), &mut output);
            black_box(&output);
        });
        measure(&format!("spectrum/{size}/gpu"), || {
            gpu.power_db(black_box(&input), &mut output).unwrap();
            black_box(&output);
        });
    }
    for cpi in [16_384, 65_536] {
        let reference = samples(cpi, 0x1234567);
        let surveillance = samples(cpi, 0x7654321);
        let mut eca = Eca::new(EcaParams::default(), 2_000_000.0).unwrap();
        let mut residual = Vec::with_capacity(cpi);
        measure(&format!("radar/{cpi}/eca_cpu"), || {
            eca.cancel(
                black_box(&reference),
                black_box(&surveillance),
                &mut residual,
            );
            black_box(&residual);
        });
        for dopplers in [33, 129] {
            let mut caf = Caf::new(cpi, 256, dopplers, 2_000_000.0);
            let mut surface = Surface::default();
            measure(&format!("radar/{cpi}/{dopplers}/caf_cpu"), || {
                caf.compute(
                    black_box(&reference),
                    black_box(&surveillance),
                    &mut surface,
                );
                black_box(&surface);
            });
        }
    }
    let plan = SubbandPlan::new(20_000_000.0).unwrap();
    for size in [2048, 4096, 8192, 32_768] {
        let input = samples(size, 0x1234567);
        let mut bank = plan.filter_bank(size);
        measure(&format!("wideband/{size}/cpu"), || {
            bank.process(black_box(&input));
            black_box(bank.samples(0));
        });
    }
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_candidates() {
    let context = Arc::new(Context::new().expect("hardware GPU required"));
    println!("adapter={:?}", context.device.adapter_info());
    for size in [4096, 16_384, 65_536] {
        for batches in [1, 4, 16] {
            let input = samples(size * batches, 0x1234567);
            let mut expected = vec![0.0; input.len()];
            let mut actual = expected.clone();
            let mut cpu = SpectrumAnalyzer::new(size);
            let mut gpu = spectrum::Spectrum::new(context.clone(), size, batches);
            for (input, out) in input.chunks(size).zip(expected.chunks_mut(size)) {
                cpu.power_db(input, out);
            }
            gpu.compute(&input, &mut actual);
            let error = expected
                .iter()
                .zip(&actual)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(error < 0.1, "spectrum error={error}");
            measure(&format!("spectrum/{size}/{batches}/cpu"), || {
                for (input, out) in input.chunks(size).zip(expected.chunks_mut(size)) {
                    cpu.power_db(black_box(input), out);
                }
                black_box(&expected);
            });
            measure(&format!("spectrum/{size}/{batches}/gpu_batched"), || {
                gpu.compute(black_box(&input), &mut actual);
                black_box(&actual);
            });
        }
    }
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_wideband() {
    let context = Arc::new(Context::new().expect("hardware GPU required"));
    let plan = SubbandPlan::new(20_000_000.0).unwrap();
    for size in [2048, 4096, 8192, 32_768] {
        let input = samples(size, 0x1234567);
        let mut cpu = plan.filter_bank(size);
        let mut gpu = wideband::Wideband::new(context.clone(), size);
        let mut actual = vec![0.0; size.div_ceil(5) * 13 * 2];
        for _ in 0..5 {
            cpu.process(&input);
            let frames = gpu.process(&input, &mut actual);
            for band in 0..13 {
                assert_eq!(cpu.samples(band).len(), frames);
                for (sample, expected) in cpu.samples(band).iter().enumerate() {
                    let at = (band * frames + sample) * 2;
                    let actual = Complex::new(actual[at], actual[at + 1]);
                    assert!((actual - expected).norm() < 2e-6, "wideband mismatch");
                }
            }
        }
        measure(&format!("wideband/{size}/cpu"), || {
            cpu.process(black_box(&input));
            black_box(cpu.samples(0));
        });
        measure(&format!("wideband/{size}/gpu"), || {
            gpu.process(black_box(&input), &mut actual);
            black_box(&actual);
        });
    }
}

#[test]
#[ignore = "hardware benchmark"]
fn benchmark_radar_tiled() {
    let context = Arc::new(Context::new().expect("hardware GPU required"));
    for (cpi, dopplers) in [
        (16_384, 33),
        (16_384, 129),
        (65_536, 33),
        (65_536, 129),
        (400_000, 41),
    ] {
        let reference = samples(cpi, 0x1234567);
        let surveillance = samples(cpi, 0x7654321);
        let mut cpu = Caf::new(cpi, 256, dopplers, 2_000_000.0);
        let mut gpu = super::caf::GpuCaf::new(context.clone(), &cpu).unwrap();
        let mut output = Surface::default();
        measure(&format!("radar/{cpi}/{dopplers}/caf_cpu"), || {
            cpu.compute(black_box(&reference), black_box(&surveillance), &mut output);
            black_box(&output);
        });
        measure(&format!("radar/{cpi}/{dopplers}/caf_gpu_tiled"), || {
            gpu.compute(black_box(&reference), black_box(&surveillance), &mut output)
                .unwrap();
            black_box(&output);
        });
    }
}
