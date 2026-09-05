#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    constellation::tables,
    ofdm::{ChannelEstimator, OfdmDemod, OfdmMod, OfdmParams},
};
use sdrmm_modem_test_support::ber::{
    catalog::ofdm::{self, RATE, SEARCH, SYMBOLS},
    perf::{
        CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf, host_id,
        load_baselines, measure_throughput, save_baselines,
    },
};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn frame() -> (OfdmParams, Vec<Complex<f32>>) {
    let params = OfdmParams::wifi_like();
    let table = tables::qam_square(4).unwrap();
    let mut state = 0x0f_06u32;
    let points: Vec<Complex<f32>> = (0..params.data_subcarriers() * SYMBOLS)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            table.points()[(state % 4) as usize]
        })
        .collect();
    let mut wave = vec![Complex::new(0.0, 0.0); ofdm::LEAD];
    OfdmMod::new(params.clone()).frame(&points, &mut wave);
    (params, wave)
}

fn data_samples(params: &OfdmParams) -> u64 {
    (SYMBOLS * params.symbol_samples()) as u64
}

fn measured() -> Vec<PerfBaseline> {
    let (params, wave) = frame();
    let mut demod = OfdmDemod::new(params.clone());
    let mut points = Vec::with_capacity(SYMBOLS * params.data_subcarriers());
    demod.acquire(&wave, SEARCH).unwrap();
    demod.demodulate(&wave, SYMBOLS, &mut points);
    points.clear();
    let symbol_msps = measure_throughput(400, data_samples(&params), || {
        points.clear();
        demod.demodulate(&wave, SYMBOLS, &mut points);
    });

    let frame_msps = measure_throughput(400, wave.len() as u64, || {
        points.clear();
        demod.acquire(&wave, SEARCH).unwrap();
        demod.demodulate(&wave, SYMBOLS, &mut points);
    });

    let geometry = format!(
        "{}-point/{}-prefix, {}+{} subcarriers, {SYMBOLS} data symbols, QPSK, 20 MHz",
        params.fft(),
        params.cp(),
        params.data_subcarriers(),
        params.map().pilots().len()
    );
    vec![
        PerfBaseline {
            bench: "ofdm64_symbols_20m".into(),
            msamples_per_s: symbol_msps,
            realtime_factor: symbol_msps * 1e6 / RATE,
            config: format!("{geometry}, per-symbol path (long-training estimate held)"),
            host: host_id(),
        },
        PerfBaseline {
            bench: "ofdm64_frame_20m".into(),
            msamples_per_s: frame_msps,
            realtime_factor: frame_msps * 1e6 / RATE,
            config: format!(
                "{geometry}, whole frame including preamble search over {SEARCH} samples"
            ),
            host: host_id(),
        },
    ]
}

fn path(stem: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

#[test]
fn the_symbol_path_allocates_nothing() {
    let (params, wave) = frame();
    let mut demod = OfdmDemod::new(params.clone());
    demod.acquire(&wave, SEARCH).unwrap();
    let mut points = vec![Complex::new(0.0, 0.0); params.data_subcarriers()];
    for _ in 0..2 {
        for symbol in 0..SYMBOLS {
            demod.symbol(&wave, symbol, &mut points);
        }
    }
    assert_no_alloc("OfdmDemod::symbol", || {
        for symbol in 0..SYMBOLS {
            demod.symbol(&wave, symbol, &mut points);
        }
    });

    let mut sink = Vec::with_capacity(SYMBOLS * params.data_subcarriers());
    for _ in 0..2 {
        demod.demodulate(&wave, SYMBOLS, &mut sink);
        sink.clear();
    }
    assert_no_alloc("OfdmDemod::demodulate", || {
        demod.demodulate(&wave, SYMBOLS, &mut sink);
    });
    assert_eq!(sink.len(), SYMBOLS * params.data_subcarriers());
}

#[test]
fn acquisition_allocates_nothing_on_either_tier() {
    let (params, wave) = frame();
    for estimator in [ChannelEstimator::LongTraining, ChannelEstimator::ShortComb] {
        let mut demod = OfdmDemod::new(params.clone()).with_estimator(estimator);
        for _ in 0..2 {
            demod.acquire(&wave, SEARCH).unwrap();
        }
        assert_no_alloc("OfdmDemod::acquire", || {
            demod.acquire(&wave, SEARCH).unwrap();
        });
    }
}

#[test]
fn the_soft_output_path_allocates_nothing() {
    let (params, mut wave) = frame();
    let mut rng = sdrmm_modem_test_support::ber::rng::Rng::new(0x0f_06);
    for s in &mut wave {
        *s += Complex::new((rng.normal() * 0.1) as f32, (rng.normal() * 0.1) as f32);
    }
    let table = tables::qam_square(16).unwrap();
    let mut demod = OfdmDemod::new(params.clone());
    demod.acquire(&wave, SEARCH).unwrap();
    let mut points = vec![Complex::new(0.0, 0.0); params.data_subcarriers()];
    demod.symbol(&wave, 0, &mut points);
    let mut llrs = vec![sdrmm_modem::soft::Llr(0.0); points.len() * table.bits_per_symbol()];
    demod.llrs(&points, &table, &mut llrs);
    assert_no_alloc("OfdmDemod::llrs", || {
        demod.llrs(&points, &table, &mut llrs);
    });
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_ofdm_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let rows = measured();
    let path = path(ofdm::PERF);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    save_baselines(&path, &rows).unwrap();
    for row in &rows {
        println!(
            "{}: {:.1} Msamples/s, {:.1}x real time",
            row.bench, row.msamples_per_s, row.realtime_factor
        );
    }
}

#[test]
#[ignore = "nightly perf gate; run in release: cargo test -p sdrmm-modem --release --test ofdm_perf compare_ -- --ignored"]
fn compare_ofdm_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    let committed = load_baselines(&path(ofdm::PERF)).unwrap();
    if committed.iter().any(|b| b.host != host_id()) {
        eprintln!("skipping the perf gate: baseline host is not {}", host_id());
        return;
    }
    match compare_perf(&measured(), &committed, REGRESSION_FRACTION) {
        Ok(changes) => {
            for c in changes {
                eprintln!(
                    "{}: {:+.1}% vs baseline ({:.1} -> {:.1} Msamples/s)",
                    c.bench,
                    100.0 * c.change_fraction,
                    c.committed_msamples_per_s,
                    c.measured_msamples_per_s
                );
            }
        }
        Err(regressions) => panic!(
            "throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * REGRESSION_FRACTION
        ),
    }
}
