//! Criterion throughput benches for the phase-0 perf scaffold (MODEM-PLAN §4.2). The numbers
//! that gate CI are the committed baselines the ignored tests in `ber::perf` write through
//! `measure_throughput`; these benches are the developer's magnifier — statistical, per
//! change, never committed. Both consume the same pre-generated signals from `ber::perf`, so
//! a criterion run and the committed number measure exactly the same work.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use sdrmm_dsp::{Fsk4Demod, SymbolSync, design_rrc};
use sdrmm_modem::ber::perf::{c4fm_iq, shaped_bpsk_iq, test_dibits};

/// Half a second of DMR-rate C4FM through the four-level front end — the §2.2 census's
/// highest-leverage chain, so its throughput is the first number worth watching.
fn fsk4_dmr_48k(c: &mut Criterion) {
    let iq = c4fm_iq(&test_dibits(2_400, 0x5eed), 48_000.0, 4_800.0, 1_944.0);
    let mut demod = Fsk4Demod::new(48_000.0, 4_800.0, 1_944.0, 0.2);
    let mut soft: Vec<f32> = Vec::with_capacity(iq.len());
    let mut group = c.benchmark_group("fsk4_dmr_48k");
    group.throughput(Throughput::Elements(iq.len() as u64));
    group.bench_function("process", |b| {
        b.iter(|| {
            soft.clear();
            demod.process(black_box(&iq), &mut soft);
            black_box(soft.len())
        });
    });
    group.finish();
}

/// The shared Gardner/Farrow loop alone, on the antipodal signal that drives its detector at
/// full rate — every engine composes it, so its cost is everyone's floor.
fn symbol_sync_8sps(c: &mut Criterion) {
    let iq = shaped_bpsk_iq(4_096, 8.0, 0x0dd5);
    let mut sync = SymbolSync::new(8.0, 0.01);
    let mut symbols: Vec<Complex<f32>> = Vec::with_capacity(iq.len());
    let mut group = c.benchmark_group("symbol_sync_8sps");
    group.throughput(Throughput::Elements(iq.len() as u64));
    group.bench_function("process", |b| {
        b.iter(|| {
            symbols.clear();
            sync.process(black_box(&iq), &mut symbols);
            black_box(symbols.len())
        });
    });
    group.finish();
}

/// A cheap canary: pure filter design, no streaming state. If this moves, the toolchain or
/// the measurement moved — not an engine.
fn design_rrc_canary(c: &mut Criterion) {
    let taps = design_rrc(10.0, 0.2, 8);
    let mut group = c.benchmark_group("design_rrc");
    group.throughput(Throughput::Elements(taps.len() as u64));
    group.bench_function("sps10_alpha02_span8", |b| {
        b.iter(|| black_box(design_rrc(black_box(10.0), 0.2, 8)));
    });
    group.finish();
}

criterion_group!(benches, fsk4_dmr_48k, symbol_sync_8sps, design_rrc_canary);
criterion_main!(benches);
