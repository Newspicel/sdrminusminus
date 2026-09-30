#![allow(clippy::expect_used)]

#[path = "perf/array.rs"]
mod array;
#[path = "perf/radar.rs"]
mod radar;

use std::hint::black_box;

use criterion::{Criterion, Throughput};
use num_complex::Complex;
use sdrmm_dsp::{fft::FftPair, xcorr::XCorr};

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

fn fir(c: &mut Criterion) {
    let input = pseudo(2_048, 0xF1);
    let real: Vec<f32> = input.iter().map(|sample| sample.re).collect();
    let taps = sdrmm_dsp::design_lowpass(127, 0.11);
    let complex_taps: Vec<_> = pseudo(127, 0xC7).iter().map(|tap| tap * 0.01).collect();
    let mut group = c.benchmark_group("fir");
    group.throughput(Throughput::Elements(input.len() as u64));
    for factor in [1, 4] {
        let mut decimator = sdrmm_dsp::Decimator::new(&taps, factor);
        let mut output = Vec::new();
        group.bench_function(format!("complex_127_by_{factor}"), |b| {
            b.iter(|| {
                decimator.process(black_box(&input), &mut output);
                black_box(&output);
            });
        });
        let mut decimator = sdrmm_dsp::RealDecimator::new(&taps, factor);
        let mut output = Vec::new();
        group.bench_function(format!("real_127_by_{factor}"), |b| {
            b.iter(|| {
                decimator.process(black_box(&real), &mut output);
                black_box(&output);
            });
        });
    }
    let mut filter = sdrmm_dsp::FirC::new(&complex_taps);
    let mut output = Vec::new();
    group.bench_function("complex_taps_127", |b| {
        b.iter(|| {
            filter.process(black_box(&input), &mut output);
            black_box(&output);
        });
    });
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

fn real_to_iq(c: &mut Criterion) {
    let input: Vec<f32> = pseudo(65_536, 0xA15).iter().map(|s| s.re).collect();
    let mut output = Vec::new();
    let mut converter = sdrmm_dsp::RealToIq::default();
    let mut group = c.benchmark_group("real_to_iq");
    group.throughput(Throughput::Elements(input.len() as u64));
    group.bench_function("half_band", |b| {
        b.iter(|| {
            converter.process(black_box(&input), &mut output);
            black_box(&output);
        });
    });
    group.finish();
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

pub(crate) fn pseudo(len: usize, seed: u64) -> Vec<Complex<f32>> {
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

fn spectrum(c: &mut Criterion) {
    let mut analyzer = sdrmm_dsp::SpectrumAnalyzer::new(4_096);
    let input = pseudo(4_096, 0x5EC);
    let mut out = vec![0.0f32; 4_096];
    let mut group = c.benchmark_group("spectrum");
    group.throughput(Throughput::Elements(4_096));
    group.bench_function("power_db_4096", |b| {
        b.iter(|| {
            analyzer.power_db(black_box(&input), &mut out);
            black_box(&out);
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

fn fm_demod(c: &mut Criterion) {
    let input = pseudo(4_096, 0xF3);
    let mut output = Vec::with_capacity(input.len());
    let mut demod = sdrmm_dsp::FmDemod::new(240_000.0, 75_000.0);
    let mut group = c.benchmark_group("fm_demod");
    group.throughput(Throughput::Elements(input.len() as u64));
    group.bench_function("240k_4096", |b| {
        b.iter(|| {
            demod.process(black_box(&input), &mut output);
            black_box(&output);
        });
    });
    group.finish();
}

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    resampling(&mut criterion);
    fir(&mut criterion);
    tuning(&mut criterion);
    real_to_iq(&mut criterion);
    shared_tuning(&mut criterion);
    fft_4096(&mut criterion);
    spectrum(&mut criterion);
    xcorr_8192(&mut criterion);
    fm_demod(&mut criterion);
    array::benches(&mut criterion);
    radar::benches(&mut criterion);
    criterion.final_summary();
}
