#![allow(clippy::expect_used)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use sdrmm_dsp::{
    caf::{Caf, Surface},
    cfar::{CfarParams, detect},
    covariance::Covariance,
    eca::{Eca, EcaParams},
    fft::FftPair,
    music::{Music, correlative, peak},
    steering::{SteeringGrid, uca},
    xcorr::XCorr,
};

const RATE: f64 = 2_000_000.0;
const FREQ_HZ: f64 = 300e6;

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

criterion_group!(
    benches,
    fft_4096,
    xcorr_8192,
    covariance_and_eigen,
    music_grid_360,
    eca_32_taps,
    caf_surface,
    cfar_surface
);
criterion_main!(benches);
