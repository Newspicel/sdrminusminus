#![allow(clippy::expect_used)]

use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use sdrmm_dsp::{
    caf::{Caf, Surface},
    cfar::{CfarParams, cluster, detect},
    combine::Combiner,
    covariance::Covariance,
    eca::{Eca, EcaParams},
    fft::FftPair,
    music::{Music, correlative, peak},
    steering::{SteeringGrid, uca},
    xcorr::XCorr,
};

const RATE: f64 = 2_000_000.0;
const FREQ_HZ: f64 = 300e6;

fn resampling(c: &mut Criterion) {
    let input = pseudo(2048, 0xF12);
    let mut output = Vec::new();
    let mut group = c.benchmark_group("resampling");
    group.throughput(Throughput::Elements(input.len() as u64));
    for (label, ratio) in [
        ("62500_to_48000", 48_000.0 / 62_500.0),
        ("250000_to_240000", 240_000.0 / 250_000.0),
        ("240000_to_48000", 48_000.0 / 240_000.0),
        ("44100_to_48000", 48_000.0 / 44_100.0),
    ] {
        let mut resampler = sdrmm_dsp::FracResampler::new(ratio);
        resampler.process(&input, &mut output);
        group.bench_function(label, |b| {
            b.iter(|| {
                resampler.process(black_box(&input), &mut output);
                black_box(&output);
            });
        });
    }
    group.finish();
}

fn tuning(c: &mut Criterion) {
    let input = pseudo(2_048, 0xDDC);
    let mut out = vec![Complex::new(0.0, 0.0); input.len()];
    let mut nco = sdrmm_dsp::Nco::new(187_500.0, 20_000_000.0);
    let mut group = c.benchmark_group("tuning");
    group.throughput(Throughput::Elements(input.len() as u64));
    group.bench_function("mix", |b| {
        b.iter(|| {
            nco.mix_into(black_box(&input), &mut out);
            black_box(&out);
        });
    });
    for output_rate in [48_000.0, 240_000.0] {
        let mut ddc = sdrmm_dsp::Ddc::new(20_000_000.0, output_rate, 187_500.0).expect("rates");
        ddc.process(&input, &mut out);
        group.bench_function(format!("ddc_{output_rate}"), |b| {
            b.iter(|| {
                ddc.process(black_box(&input), &mut out);
                black_box(&out);
            });
        });
    }
    for channels in [16, 32] {
        let mut downconverters: Vec<_> = (0..channels)
            .map(|index| {
                let rate = if index % 4 == 1 { 240_000.0 } else { 48_000.0 };
                let offset = 100_000.0 + index as f64 * 25_000.0;
                let mut ddc = sdrmm_dsp::Ddc::new(20_000_000.0, rate, offset).expect("rates");
                ddc.process(&input, &mut out);
                ddc
            })
            .collect();
        group.throughput(Throughput::Elements(input.len() as u64));
        group.bench_function(format!("mixed_{channels}_channels"), |b| {
            b.iter(|| {
                for ddc in &mut downconverters {
                    ddc.process(black_box(&input), &mut out);
                    black_box(&out);
                }
            });
        });
    }
    group.finish();
}

struct SharedBand {
    index: usize,
    decimator: sdrmm_dsp::subband::SubbandDecimator,
    output: Vec<Complex<f32>>,
}

fn shared_tuning(c: &mut Criterion) {
    shared_tuning_at_rate(c, 20_000_000.0, 2048, "shared_tuning");
    shared_tuning_at_rate(c, 5_000_000.0, 4096, "shared_tuning_5msps");
    shared_tuning_at_rate(c, 6_000_000.0, 4096, "shared_tuning_6msps");
    shared_tuning_at_rate(c, 10_000_000.0, 8192, "shared_tuning_10msps");
    shared_tuning_at_rate(c, 12_000_000.0, 8192, "shared_tuning_12msps");
    shared_tuning_at_rate(c, 16_000_000.0, 8192, "shared_tuning_16msps");
    shared_tuning_at_rate(c, 20_000_000.0, 8192, "shared_tuning_20msps");
    shared_tuning_at_rate(c, 8_000_000.0, 4096, "shared_tuning_8msps");
}

fn shared_tuning_at_rate(c: &mut Criterion, input_rate: f64, block_len: usize, name: &str) {
    let input = pseudo(block_len, 0x5B);
    let plan = sdrmm_dsp::subband::SubbandPlan::new(input_rate).expect("rate");
    let mut output = Vec::new();
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Elements(input.len() as u64));
    for (layout, start, spread) in [
        ("clustered", 100_000.0, false),
        ("shifted", input_rate * 0.085, false),
        ("spread", 0.0, true),
    ] {
        for count in [1, 2, 4, 8, 10, 12, 16, 32] {
            let settings: Vec<_> = (0..count)
                .map(|index| {
                    let offset = if spread && count > 1 {
                        input_rate * (0.88 * index as f64 / (count - 1) as f64 - 0.44)
                    } else {
                        start + index as f64 * 25_000.0
                    };
                    let rate = if index % 4 == 1 { 240_000.0 } else { 48_000.0 };
                    let offset = protected_offset(plan, offset, rate);
                    (offset, rate)
                })
                .collect();
            let mut direct: Vec<_> = settings
                .iter()
                .map(|&(offset, rate)| {
                    sdrmm_dsp::Ddc::new(input_rate, rate, offset).expect("rates")
                })
                .collect();
            group.bench_function(format!("{layout}/{count}/independent"), |b| {
                b.iter(|| {
                    for ddc in &mut direct {
                        ddc.process(black_box(&input), &mut output);
                        black_box(&output);
                    }
                });
            });
            let mut bands: Vec<SharedBand> = Vec::new();
            let mut channels = Vec::new();
            for (offset, rate) in settings {
                let index = plan.select(offset, rate).expect("protected subband");
                let center = plan.center(index);
                let band = bands
                    .iter()
                    .position(|band| band.index == index)
                    .unwrap_or_else(|| {
                        bands.push(SharedBand {
                            index,
                            decimator: plan.decimator(index, input.len()),
                            output: Vec::new(),
                        });
                        bands.len() - 1
                    });
                channels.push((
                    band,
                    sdrmm_dsp::Ddc::new(plan.output_rate(), rate, offset - center).expect("rates"),
                ));
            }
            group.bench_function(format!("{layout}/{count}/shared"), |b| {
                b.iter(|| {
                    for band in &mut bands {
                        band.decimator.process(black_box(&input), &mut band.output);
                    }
                    for (band, ddc) in &mut channels {
                        ddc.process(&bands[*band].output, &mut output);
                        black_box(&output);
                    }
                });
            });
            let mut bank = plan.filter_bank(input.len());
            group.bench_function(format!("{layout}/{count}/filter_bank"), |b| {
                b.iter(|| {
                    bank.process(black_box(&input));
                    for (band, ddc) in &mut channels {
                        ddc.process(bank.samples(bands[*band].index), &mut output);
                        black_box(&output);
                    }
                });
            });
        }
    }
    group.finish();
}

fn protected_offset(plan: sdrmm_dsp::subband::SubbandPlan, offset: f64, rate: f64) -> f64 {
    if plan.select(offset, rate).is_some() {
        return offset;
    }
    let center = (0..sdrmm_dsp::subband::SUBBANDS)
        .map(|band| plan.center(band))
        .min_by(|a, b| (a - offset).abs().total_cmp(&(b - offset).abs()))
        .expect("subbands");
    let margin = 0.49 * (plan.bandwidth() - rate);
    center + (offset - center).clamp(-margin, margin)
}

fn pseudo(len: usize, seed: u64) -> Vec<Complex<f32>> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            let mut next = || {
                state ^= state >> 12;
                state ^= state << 25;
                state ^= state >> 27;
                (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32 - 1.0
            };
            Complex::new(next(), next())
        })
        .collect()
}

fn lanes(count: usize, len: usize) -> Vec<Vec<Complex<f32>>> {
    (0..count)
        .map(|lane| {
            let base = pseudo(len, 0x1000 + lane as u64);
            let rotation = Complex::from_polar(1.0f32, 0.4 * lane as f32);
            base.iter().map(|s| s * rotation).collect()
        })
        .collect()
}

fn fft_4096(c: &mut Criterion) {
    let mut fft = FftPair::new(4_096);
    let mut buf = pseudo(4_096, 0xF17);
    let mut group = c.benchmark_group("fft_4096");
    group.throughput(Throughput::Elements(4_096));
    group.bench_function("forward", |b| {
        b.iter(|| {
            fft.forward(black_box(&mut buf));
            black_box(buf[0])
        });
    });
    group.finish();
}

fn xcorr_8192(c: &mut Criterion) {
    let mut xcorr = XCorr::new(8_192);
    let a = pseudo(8_192, 0xAA);
    let b_lane = pseudo(8_192, 0xBB);
    let mut group = c.benchmark_group("xcorr_8192");
    group.throughput(Throughput::Elements(8_192));
    group.bench_function("estimate", |bench| {
        bench.iter(|| black_box(xcorr.estimate(black_box(&a), black_box(&b_lane))));
    });
    group.finish();
}

fn covariance_and_eigen(c: &mut Criterion) {
    let built = lanes(4, 8_192);
    let borrowed: Vec<&[Complex<f32>]> = built.iter().map(Vec::as_slice).collect();
    let mut covariance = Covariance::new(4);
    let mut matrix = Vec::new();
    let mut music = Music::new(4).expect("order");
    let grid = SteeringGrid::new(&uca(0.35, 4), FREQ_HZ, 1.0);
    let mut surface = Vec::new();
    let mut group = c.benchmark_group("covariance_eig_4x8192");
    group.throughput(Throughput::Elements(8_192));
    group.bench_function("accumulate_and_solve", |b| {
        b.iter(|| {
            covariance.reset();
            covariance.accumulate(black_box(&borrowed));
            covariance.matrix(&mut matrix);
            music.pseudospectrum(&matrix, &grid, 1, &mut surface);
            black_box(surface.len())
        });
    });
    group.finish();
}

fn music_grid_360(c: &mut Criterion) {
    let built = lanes(4, 4_096);
    let borrowed: Vec<&[Complex<f32>]> = built.iter().map(Vec::as_slice).collect();
    let mut covariance = Covariance::new(4);
    covariance.accumulate(&borrowed);
    let mut matrix = Vec::new();
    covariance.matrix(&mut matrix);
    let grid = SteeringGrid::new(&uca(0.35, 4), FREQ_HZ, 1.0);
    let mut music = Music::new(4).expect("order");
    let mut surface = Vec::new();
    let mut scratch = Vec::new();
    let mut group = c.benchmark_group("music_grid_360");
    group.throughput(Throughput::Elements(360));
    group.bench_function("pseudospectrum", |b| {
        b.iter(|| {
            music.pseudospectrum(black_box(&matrix), &grid, 1, &mut surface);
            black_box(peak(&surface, &grid, &mut scratch).bearing_deg)
        });
    });
    group.bench_function("correlative", |b| {
        b.iter(|| {
            correlative(black_box(&matrix), &grid, &mut surface);
            black_box(surface.len())
        });
    });
    group.finish();
}

fn eca_32_taps(c: &mut Criterion) {
    let reference = pseudo(32_768, 0xEC1);
    let surveillance = pseudo(32_768, 0xEC2);
    let mut eca = Eca::new(
        EcaParams {
            delay_taps: 32,
            doppler_bins: 0,
            batch: 16_384,
            loading: 1e-4,
        },
        RATE,
    )
    .expect("sized");
    let mut residual = Vec::new();
    let mut group = c.benchmark_group("eca_32taps_32768");
    group.throughput(Throughput::Elements(32_768));
    group.bench_function("cancel", |b| {
        b.iter(|| {
            eca.cancel(
                black_box(&reference),
                black_box(&surveillance),
                &mut residual,
            );
            black_box(residual.len())
        });
    });
    group.finish();
}

fn caf_surface(c: &mut Criterion) {
    let cpi = 16_384;
    let reference = pseudo(cpi, 0xCA1);
    let surveillance = pseudo(cpi, 0xCA2);
    let mut caf = Caf::new(cpi, 256, 33, RATE);
    let mut surface = Surface::default();
    let mut group = c.benchmark_group("caf_256x33");
    group.throughput(Throughput::Elements(256 * 33));
    group.bench_function("compute", |b| {
        b.iter(|| {
            caf.compute(
                black_box(&reference),
                black_box(&surveillance),
                &mut surface,
            );
            black_box(surface.power.len())
        });
    });
    group.finish();
}

fn cfar_surface(c: &mut Criterion) {
    let (ranges, dopplers) = (256usize, 33usize);
    let surface: Vec<f32> = pseudo(ranges * dopplers, 0xCFA)
        .iter()
        .map(|s| s.norm_sqr() + 0.1)
        .collect();
    let params = CfarParams::default();
    let mut detections = Vec::new();
    let mut group = c.benchmark_group("cfar_256x33");
    group.throughput(Throughput::Elements((ranges * dopplers) as u64));
    group.bench_function("detect", |b| {
        b.iter(|| {
            detect(
                black_box(&surface),
                ranges,
                dopplers,
                &params,
                &mut detections,
            );
            black_box(detections.len())
        });
    });
    group.finish();
}

fn combiner_weights(c: &mut Criterion) {
    let lanes = lanes(4, 16_384);
    let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
    let mut covariance = Covariance::new(4);
    covariance.set_forward_backward(false);
    covariance.accumulate(&views);
    let mut matrix = Vec::new();
    covariance.matrix(&mut matrix);
    let mut combiner = Combiner::new(4).expect("four lanes");
    let mut weights = Vec::new();

    let mut group = c.benchmark_group("combiner_weights");
    group.bench_function("diversity", |b| {
        b.iter(|| {
            combiner.diversity(black_box(&matrix), &mut weights);
            black_box(weights[0])
        });
    });
    group.bench_function("cancel", |b| {
        b.iter(|| {
            combiner
                .cancel(black_box(&matrix), &mut weights)
                .expect("the auxiliary lanes carry power");
            black_box(weights[0])
        });
    });
    group.finish();
}

fn cfar_cluster(c: &mut Criterion) {
    let detections: Vec<sdrmm_dsp::cfar::Detection> = (0..256)
        .map(|index| sdrmm_dsp::cfar::Detection {
            range_bin: index / 4,
            doppler_bin: index % 8,
            snr_db: (index % 17) as f32,
        })
        .collect();
    let mut group = c.benchmark_group("cfar_cluster");
    group.throughput(Throughput::Elements(detections.len() as u64));
    group.bench_function("256_hits", |b| {
        b.iter_batched_ref(
            || detections.clone(),
            |working| {
                cluster(black_box(working));
                black_box(working.len())
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group!(
    benches,
    resampling,
    tuning,
    shared_tuning,
    fft_4096,
    xcorr_8192,
    covariance_and_eigen,
    music_grid_360,
    eca_32_taps,
    caf_surface,
    cfar_surface,
    cfar_cluster,
    combiner_weights
);
criterion_main!(benches);
