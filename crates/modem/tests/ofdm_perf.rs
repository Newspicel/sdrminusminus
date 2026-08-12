//! §4.2 performance baselines for the phase-6 OFDM entry: the per-symbol receive path and the
//! whole-frame path a burst receiver actually pays, each measured on the waveform the entry's own
//! catalog chain transmits, plus the steady-state zero-allocation gates.
//!
//! Two benches rather than one, because an OFDM receiver's cost splits in two and the split is
//! the interesting part: the per-symbol path is a transform and a table of one-tap divisions,
//! while acquisition is a repetition search plus a correlation over the whole preamble — a fixed
//! cost per burst that a long frame amortises and a short one does not.
//!
//! Real-time factors divide by the reference configuration's 20 MHz, so the numbers answer the
//! same "how many channels of this per core" question every other entry's do.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        catalog::ofdm::{self, RATE, SEARCH, SYMBOLS},
        perf::{
            CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf,
            host_id, load_baselines, measure_throughput, save_baselines,
        },
    },
    constellation::tables,
    ofdm::{ChannelEstimator, OfdmDemod, OfdmMod, OfdmParams},
};

/// This test binary's allocation counter — `#[global_allocator]` binds per binary, so the library
/// cannot install it on anyone's behalf (see `ber::perf`).
#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

/// One QPSK frame at the reference geometry, with the same lead-in the catalog chain transmits so
/// the acquisition bench searches for a burst it has to actually find.
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

/// Samples the per-symbol bench consumes: the data part of a frame, preamble excluded, since that
/// is what the symbol path reads.
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

// --- Zero-allocation gates (§4.2) ------------------------------------------------------------

/// The per-symbol path: de-rotate, transform, equalise, track, write. Warmed twice per the §4.2
/// convention, then one steady-state call must acquire no memory.
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

/// Acquisition too, on both estimator tiers. It is not the per-sample hot path — it runs once per
/// burst — but a receiver scanning for bursts runs it *constantly* on noise, and the channel
/// estimate's interpolation is the one place here that would naturally have reached for a `Vec`.
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

/// The soft path: one symbol's points through the crate's demapper at the per-bin noise variance.
#[test]
fn the_soft_output_path_allocates_nothing() {
    let (params, mut wave) = frame();
    // Noise, because the soft path is about believing a symbol *by how noisy its bin is*: a
    // noiseless frame would measure a variance the demapper is right to refuse.
    let mut rng = sdrmm_modem::ber::rng::Rng::new(0x0f_06);
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

// --- Baseline writer and gate (§4.2 protocol) ------------------------------------------------

/// Rewrites the committed baseline. Run deliberately, on the reference machine:
/// `cargo test -p sdrmm-modem --release --test ofdm_perf write_ -- --ignored`.
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

/// The nightly perf gate: measured against committed, failing past [`REGRESSION_FRACTION`].
/// Compared only in release and only on the host that wrote the baseline.
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
