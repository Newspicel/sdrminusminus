use std::hint::black_box;

use criterion::{Criterion, Throughput};
use num_complex::Complex;
use sdrmm_dsp::{
    Ddc, IqDcBlocker,
    array_sync::{FastConvolver, design_correction},
    beamform::{Adaptation, BlockSolver, Constraints, TdlCanceller, WeightSet},
    correlator::FxCorrelator,
    covariance::{CovarianceBank, SampleCovariance},
    doa::{Doa, DoaConfig, DoaReport, Estimator},
    linalg::{CMat, Eigen, HermitianEigen},
    manifold::{Direction, Geometry, Manifold, Winding},
};

use super::pseudo;

const FREQ: f64 = 433.92e6;
const RATE: f64 = 2_400_000.0;
const LANES: usize = 5;
const BLOCK: usize = 16_384;
const CORR_FFT: usize = 4_096;
const CORR_TAPS: usize = 129;
const SNAPSHOTS: f64 = 4_096.0;
const DDC_OFFSET_HZ: f64 = 25_000.0;

fn lanes(count: usize, len: usize) -> Vec<Vec<Complex<f32>>> {
    (0..count)
        .map(|lane| {
            let base = pseudo(len, 0x1000 + lane as u64);
            let turn = Complex::from_polar(1.0f32, 0.4 * lane as f32);
            base.iter().map(|sample| sample * turn).collect()
        })
        .collect()
}

fn views(lanes: &[Vec<Complex<f32>>]) -> Vec<&[Complex<f32>]> {
    lanes.iter().map(Vec::as_slice).collect()
}

fn kraken() -> Manifold {
    Manifold::ideal(Geometry::uca(0.35, LANES, 0.0, Winding::Clockwise).expect("a kraken circle"))
}

fn line8() -> Manifold {
    Manifold::ideal(Geometry::ula(0.3, 8, 180.0).expect("an 8 element line"))
}

fn scene(manifold: &Manifold, sources: &[(f64, f32)]) -> CMat {
    let n = manifold.len();
    let mut r = CMat::identity(n).expect("an order");
    let mut a = [Complex::new(0.0f32, 0.0); 16];
    for &(azimuth, power) in sources {
        manifold.steer(FREQ, Direction::horizon(azimuth), &mut a[..n]);
        for i in 0..n {
            for j in 0..n {
                r.add(i, j, a[i] * a[j].conj() * power);
            }
        }
    }
    r
}

fn steering(manifold: &Manifold, azimuth: f64) -> [Complex<f32>; LANES] {
    let mut out = [Complex::new(0.0, 0.0); LANES];
    manifold.steer(FREQ, Direction::horizon(azimuth), &mut out);
    out
}

fn covariance(c: &mut Criterion) {
    let lanes = lanes(LANES, BLOCK);
    let views = views(&lanes);
    let mut covariance = SampleCovariance::new(LANES).expect("an order");
    let mut r = CMat::zeros(LANES).expect("an order");
    let mut group = c.benchmark_group("array");
    group.throughput(Throughput::Elements(BLOCK as u64));
    group.bench_function("covariance_5x16384", |b| {
        b.iter(|| {
            covariance.reset();
            covariance.accumulate(black_box(&views));
            black_box(covariance.matrix(&mut r))
        });
    });
    let r = scene(&kraken(), &[(40.0, 10.0), (200.0, 3.0)]);
    let mut eigen = HermitianEigen::new(LANES).expect("an order");
    let mut values = Eigen::new();
    group.throughput(Throughput::Elements(1));
    group.bench_function("eigen_5x5", |b| {
        b.iter(|| {
            eigen.solve(black_box(&r), &mut values).expect("a solve");
            black_box(values.values()[0])
        });
    });
    let mut bank = CovarianceBank::new(LANES, 1_024, 512).expect("a bank");
    group.throughput(Throughput::Elements(BLOCK as u64));
    group.bench_function("bank_1024", |b| {
        b.iter(|| black_box(bank.push(black_box(&views), 0.95).expect("a push")));
    });
    group.finish();
}

fn estimator(c: &mut Criterion, name: &str, manifold: &Manifold, estimator: Estimator) {
    let r = scene(manifold, &[(60.0, 10.0), (110.0, 5.0)]);
    let config = DoaConfig {
        estimator,
        ..DoaConfig::default()
    };
    let mut doa = Doa::new(manifold, &config, FREQ).expect("a doa");
    let mut report = DoaReport::default();
    let mut group = c.benchmark_group("array");
    group.throughput(Throughput::Elements(1));
    group.bench_function(name, |b| {
        b.iter(|| {
            doa.estimate(manifold, black_box(&r), SNAPSHOTS, 3.0, &mut report)
                .expect("an estimate");
            black_box(report.peaks[0].azimuth_deg)
        });
    });
    group.finish();
}

fn estimators(c: &mut Criterion) {
    let kraken = kraken();
    estimator(c, "music_360", &kraken, Estimator::Music);
    estimator(c, "capon_360", &kraken, Estimator::Capon);
    let line = line8();
    estimator(c, "root_music_8", &line, Estimator::RootMusic);
    estimator(c, "esprit_8", &line, Estimator::Esprit);
}

fn beamform(c: &mut Criterion) {
    let manifold = kraken();
    let r = scene(&manifold, &[(137.0, 10.0), (20.0, 5.0)]);
    let mut solver = BlockSolver::new(LANES).expect("a solver");
    let mut weights = WeightSet::zeros(LANES);
    let mut constraints = Constraints::new(LANES);
    constraints
        .push(&steering(&manifold, 137.0), Complex::new(1.0, 0.0))
        .expect("a look");
    for null in [20.0, 250.0] {
        constraints
            .push(&steering(&manifold, null), Complex::new(0.0, 0.0))
            .expect("a null");
    }
    let mut group = c.benchmark_group("array");
    group.throughput(Throughput::Elements(1));
    group.bench_function("lcmv_5", |b| {
        b.iter(|| {
            black_box(
                solver
                    .lcmv(Some(black_box(&r)), &constraints, 0.1, &mut weights)
                    .expect("a solve"),
            )
        });
    });
    let lanes = lanes(LANES, BLOCK);
    let views = views(&lanes);
    let mut canceller =
        TdlCanceller::new(0, &[1, 2, 3, 4], 8, Adaptation::Nlms { step: 0.05 }).expect("a tdl");
    let mut out = Vec::with_capacity(BLOCK);
    group.throughput(Throughput::Elements(BLOCK as u64));
    group.bench_function("tdl_8_taps", |b| {
        b.iter(|| {
            out.clear();
            canceller
                .process(black_box(&views), &mut out)
                .expect("a block");
            black_box(out.len())
        });
    });
    let mut correlator = FxCorrelator::new(LANES, 1_024, false).expect("a correlator");
    group.bench_function("correlator_1024", |b| {
        b.iter(|| black_box(correlator.push(black_box(&views)).expect("a push")));
    });
    group.finish();
}

fn correction(fft: usize, frac: f32, weight: Complex<f32>) -> Vec<Complex<f32>> {
    let mut spectrum = Vec::with_capacity(fft);
    design_correction(fft, CORR_TAPS, 8.0, frac, weight, None, &mut spectrum);
    spectrum
}

fn correct(c: &mut Criterion, lanes: &[Vec<Complex<f32>>], fft: usize) {
    let mut convolvers: Vec<FastConvolver> = (0..LANES)
        .map(|lane| {
            let mut convolver = FastConvolver::new(fft, CORR_TAPS);
            let weight = Complex::from_polar(1.0, lane as f32);
            let spectrum = correction(fft, 0.1 + 0.1 * lane as f32, weight);
            convolver.set_response(&spectrum).expect("a response");
            convolver
        })
        .collect();
    let mut out: Vec<Vec<Complex<f32>>> = (0..LANES)
        .map(|_| Vec::with_capacity(BLOCK + fft))
        .collect();
    let mut group = c.benchmark_group("array");
    group.throughput(Throughput::Elements((LANES * BLOCK) as u64));
    group.bench_function("correct_5x16384", |b| {
        b.iter(|| {
            for ((convolver, lane), out) in convolvers.iter_mut().zip(lanes).zip(&mut out) {
                out.clear();
                convolver.push(black_box(lane), out);
            }
            black_box(out[0].len())
        });
    });
    group.finish();
}

fn front(c: &mut Criterion) {
    let mut lanes = lanes(LANES, BLOCK);
    correct(c, &lanes, CORR_FFT);
    let mut group = c.benchmark_group("array");
    group.throughput(Throughput::Elements((LANES * BLOCK) as u64));
    let mut blockers: Vec<IqDcBlocker> =
        (0..LANES).map(|_| IqDcBlocker::new(RATE, 146.0)).collect();
    group.bench_function("dc_5x16384", |b| {
        b.iter(|| {
            for (blocker, lane) in blockers.iter_mut().zip(&mut lanes) {
                blocker.process(black_box(lane));
            }
            black_box(lanes[0][0])
        });
    });
    let mut ddcs: Vec<Ddc> = (0..LANES)
        .map(|_| Ddc::new(RATE, 20_000.0, DDC_OFFSET_HZ).expect("rates"))
        .collect();
    let mut narrow = Vec::with_capacity(BLOCK);
    group.bench_function("ddc_5x16384_to_20k", |b| {
        b.iter(|| {
            for (ddc, lane) in ddcs.iter_mut().zip(&lanes) {
                ddc.process(black_box(lane), &mut narrow);
            }
            black_box(narrow.len())
        });
    });
    group.finish();
}

pub(crate) fn benches(c: &mut Criterion) {
    covariance(c);
    estimators(c);
    beamform(c);
    front(c);
}
