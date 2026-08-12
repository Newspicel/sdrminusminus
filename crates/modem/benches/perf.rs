//! Criterion throughput benches for the phase-0 perf scaffold (MODEM-PLAN §4.2). The numbers
//! that gate CI are the committed baselines the ignored tests in `ber::perf` write through
//! `measure_throughput`; these benches are the developer's magnifier — statistical, per
//! change, never committed. Both consume the same pre-generated signals from `ber::perf`, so
//! a criterion run and the committed number measure exactly the same work.

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use num_complex::Complex;
use sdrmm_dsp::{SymbolSync, design_rrc};
use sdrmm_modem::ber::perf::{shaped_bpsk_iq, test_dibits};

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

/// The phase-3 CPM engine at the M = 4 reference configuration (48 kHz, 4800 baud, ETSI
/// dibit table, ±1944 Hz, RRC α = 0.2, timing bw 0.015) — the committed number lives in
/// `baselines/cpm/mfsk_perf.json`, written by the ignored test in `tests/mfsk_perf.rs` from
/// the identical waveform and construction, so the bench and the gate measure the same work.
fn cpm_demod_m4_48k(c: &mut Criterion) {
    use sdrmm_modem::{
        cpm::{CpmDemod, CpmMod, CpmParams, Mapping, TIMING_BW_BURST},
        pulse::{self, Norm},
    };
    let params = CpmParams::from_deviation(
        Mapping::new(vec![1.0, 3.0, -1.0, -3.0]),
        1_944.0,
        4_800.0,
        pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area),
        10.0,
    );
    let mut modulator = CpmMod::new(params.clone());
    let mut iq: Vec<Complex<f32>> = Vec::new();
    modulator.modulate(&test_dibits(2_400, 0x5eed), &mut iq);
    modulator.flush(&mut iq);
    let rx = pulse::root_raised_cosine(10.0, 0.2, 8, Norm::Area);
    let mut demod = CpmDemod::new(&params, &rx, TIMING_BW_BURST);
    let mut soft: Vec<f32> = Vec::with_capacity(iq.len());
    let mut group = c.benchmark_group("cpm_demod_m4_48k");
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

/// GMSK BT = 0.5 (D-STAR/Bluetooth-BR shape) at 10 sps through its pulse-matched receive
/// filter — the committed number lives in `baselines/cpm/gmsk_perf.json`, written by the
/// ignored test in `tests/gmsk_msk_afsk_bundles.rs` from the identical waveform and
/// construction, so the bench and the gate measure the same work (likewise the two below).
fn gmsk_bt05_demod(c: &mut Criterion) {
    use sdrmm_modem::{
        cpm::{CpmDemod, CpmMod, CpmParams, Mapping, TIMING_BW_BURST},
        pulse::{self, Norm},
    };
    let params = CpmParams::from_h(
        Mapping::natural(2),
        0.5,
        pulse::gaussian_freq(10.0, 0.5, 3, Norm::Area),
        10.0,
    );
    let mut modulator = CpmMod::new(params.clone());
    let mut iq: Vec<Complex<f32>> = Vec::new();
    modulator.modulate(&test_bits(2_400, 0x5eed), &mut iq);
    modulator.flush(&mut iq);
    let rx = pulse::gaussian_freq(10.0, 0.5, 3, Norm::Area);
    let mut demod = CpmDemod::new(&params, &rx, TIMING_BW_BURST);
    let mut soft: Vec<f32> = Vec::with_capacity(iq.len());
    let mut group = c.benchmark_group("gmsk_bt05_demod");
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

/// MSK (1REC h = ½) at 10 sps with the integrate-and-dump receive filter.
fn msk_demod(c: &mut Criterion) {
    use sdrmm_modem::{
        cpm::{CpmDemod, CpmMod, CpmParams, Mapping, TIMING_BW_BURST},
        pulse::{self, Norm},
    };
    let params = CpmParams::from_h(
        Mapping::natural(2),
        0.5,
        pulse::rect(10.0, Norm::Area),
        10.0,
    );
    let mut modulator = CpmMod::new(params.clone());
    let mut iq: Vec<Complex<f32>> = Vec::new();
    modulator.modulate(&test_bits(2_400, 0x5eed), &mut iq);
    modulator.flush(&mut iq);
    let rx = pulse::rect(10.0, Norm::Area);
    let mut demod = CpmDemod::new(&params, &rx, TIMING_BW_BURST);
    let mut soft: Vec<f32> = Vec::with_capacity(iq.len());
    let mut group = c.benchmark_group("msk_demod");
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

/// Bell-202-shaped AFSK (1200/2200 Hz, 1200 baud) through the tier-1 tone-filterbank
/// detector on real 12 kHz audio — the real-input hot path.
fn afsk_filterbank_12k(c: &mut Criterion) {
    use sdrmm_modem::{
        cpm::{CpmDemod, CpmMod, CpmParams, Mapping, RealDetector, TIMING_BW_BURST},
        pulse::{self, Norm},
    };
    let params = CpmParams::from_deviation(
        Mapping::new(vec![1.0, -1.0]),
        500.0,
        1_200.0,
        pulse::rect(10.0, Norm::Area),
        10.0,
    );
    let mut modulator = CpmMod::new(params.clone());
    let mut baseband: Vec<Complex<f32>> = Vec::new();
    modulator.modulate(&test_bits(2_400, 0x5eed), &mut baseband);
    modulator.flush(&mut baseband);
    let mut carrier = sdrmm_dsp::Nco::new(1_700.0, 12_000.0);
    let audio: Vec<f32> = baseband
        .iter()
        .map(|&s| (s * carrier.next_sample()).re)
        .collect();
    let detector = RealDetector::ToneFilterbank {
        plus_hz: 2_200.0,
        minus_hz: 1_200.0,
    };
    let rx = pulse::rect(5.0, Norm::Area);
    let mut demod = CpmDemod::real(&params, &rx, TIMING_BW_BURST, 12_000.0, detector);
    let mut soft: Vec<f32> = Vec::with_capacity(audio.len());
    let mut group = c.benchmark_group("afsk_filterbank_12k");
    group.throughput(Throughput::Elements(audio.len() as u64));
    group.bench_function("process_real", |b| {
        b.iter(|| {
            soft.clear();
            demod.process_real(black_box(&audio), &mut soft);
            black_box(soft.len())
        });
    });
    group.finish();
}

/// The phase-5 orthogonal M-FSK entry at its reference configuration (48 kHz, 4800 baud, M = 4,
/// spacing one cycle per symbol) — the committed number lives in
/// `baselines/orthogonal/mfsk_perf.json`, written by the ignored test in
/// `tests/orthogonal_ppm_perf.rs` from the identical waveform, so bench and gate measure the
/// same work (likewise the PPM bench below).
fn mfsk4_filterbank_48k(c: &mut Criterion) {
    use sdrmm_modem::{ber::catalog::orthogonal, orthogonal::MfskDemod};
    let symbols = orthogonal::filler(4, 1_200);
    let iq = orthogonal::modulate(4, &symbols);
    let demod = MfskDemod::new(orthogonal::params(4));
    let mut out: Vec<u8> = Vec::with_capacity(symbols.len());
    let mut group = c.benchmark_group("mfsk4_filterbank_48k");
    group.throughput(Throughput::Elements(iq.len() as u64));
    group.bench_function("demodulate", |b| {
        b.iter(|| {
            out.clear();
            demod.demodulate(black_box(&iq), 0, symbols.len(), &mut out);
            black_box(out.len())
        });
    });
    group.finish();
}

/// The phase-5 M-PPM entry's tier-1 detector at 1 Mslot/s on 8 Msps — the Mode S alphabet at the
/// catalog's reference oversampling.
fn ppm2_matched_8m(c: &mut Criterion) {
    use sdrmm_modem::{ber::catalog::ppm, ppm::SlotDetector};
    let symbols = ppm::filler(2, 2_048);
    let iq = ppm::modulate(2, &symbols);
    let demod = ppm::demod(2, symbols.len(), SlotDetector::MatchedFilter);
    let mut out: Vec<u8> = Vec::with_capacity(symbols.len());
    let mut group = c.benchmark_group("ppm2_matched_8m");
    group.throughput(Throughput::Elements(iq.len() as u64));
    group.bench_function("demodulate", |b| {
        b.iter(|| {
            out.clear();
            demod.demodulate(black_box(&iq), 0, symbols.len(), &mut out);
            black_box(out.len())
        });
    });
    group.finish();
}

/// 2-level bench symbols from the shared dibit generator, so every CPM bench draws from one
/// deterministic source.
fn test_bits(len: usize, seed: u32) -> Vec<u8> {
    test_dibits(len, seed).into_iter().map(|d| d & 1).collect()
}

criterion_group!(
    benches,
    symbol_sync_8sps,
    design_rrc_canary,
    cpm_demod_m4_48k,
    gmsk_bt05_demod,
    msk_demod,
    afsk_filterbank_12k,
    mfsk4_filterbank_48k,
    ppm2_matched_8m
);
criterion_main!(benches);
