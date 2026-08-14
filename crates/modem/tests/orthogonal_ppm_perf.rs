#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        catalog::{
            orthogonal::{self, SPS as MFSK_SPS},
            ppm::{self as ppm_catalog, RATE as PPM_RATE, SLOT_SPS},
        },
        perf::{
            CountingAlloc, PerfBaseline, REGRESSION_FRACTION, assert_no_alloc, compare_perf,
            host_id, load_baselines, measure_throughput, save_baselines,
        },
    },
    orthogonal::MfskDemod,
    ppm::{PpmDemod, SlotDetector},
};

/// This test binary's allocation counter — `#[global_allocator]` binds per binary, so the
/// library cannot install it on anyone's behalf (see `ber::perf`).
#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

const MFSK_RATE: f64 = orthogonal::RATE;
/// Symbols per measured block: a quarter second at 4800 baud, and the same block the M-FSK
/// entry's own trials are shaped from.
const MFSK_SYMBOLS: usize = 1_200;
/// Symbols per measured PPM block: 4096 slots at 8 samples each, ~4 ms of 1 Mslot/s traffic.
const PPM_SYMBOLS: usize = 2_048;

fn mfsk_signal() -> Vec<Complex<f32>> {
    orthogonal::modulate(4, &orthogonal::filler(4, MFSK_SYMBOLS))
}

fn ppm_signal() -> Vec<Complex<f32>> {
    ppm_catalog::modulate(2, &ppm_catalog::filler(2, PPM_SYMBOLS))
}

fn measured_mfsk() -> Vec<PerfBaseline> {
    let iq = mfsk_signal();
    let demod = MfskDemod::new(orthogonal::params(4));
    let mut symbols = Vec::with_capacity(MFSK_SYMBOLS);
    demod.demodulate(&iq, 0, MFSK_SYMBOLS, &mut symbols);
    symbols.clear();
    let msamples_per_s = measure_throughput(200, iq.len() as u64, || {
        symbols.clear();
        demod.demodulate(&iq, 0, MFSK_SYMBOLS, &mut symbols);
    });
    vec![PerfBaseline {
        bench: "mfsk4_filterbank_48k".into(),
        msamples_per_s,
        realtime_factor: msamples_per_s * 1e6 / MFSK_RATE,
        config: format!(
            "48 kHz, 4800 baud, {MFSK_SPS} sps, M=4 orthogonal tone plan (spacing 1 cycle/symbol)"
        ),
        host: host_id(),
    }]
}

fn measured_ppm() -> Vec<PerfBaseline> {
    let iq = ppm_signal();
    [
        ("ppm2_matched_8m", SlotDetector::MatchedFilter),
        ("ppm2_envelope_8m", SlotDetector::Envelope),
    ]
    .into_iter()
    .map(|(bench, detector)| {
        let demod = ppm_catalog::demod(2, PPM_SYMBOLS, detector);
        let mut symbols = Vec::with_capacity(PPM_SYMBOLS);
        demod.demodulate(&iq, 0, PPM_SYMBOLS, &mut symbols);
        symbols.clear();
        let msamples_per_s = measure_throughput(200, iq.len() as u64, || {
            symbols.clear();
            demod.demodulate(&iq, 0, PPM_SYMBOLS, &mut symbols);
        });
        PerfBaseline {
            bench: bench.into(),
            msamples_per_s,
            realtime_factor: msamples_per_s * 1e6 / PPM_RATE,
            config: format!(
                "1 Mslot/s at 8 Msps ({SLOT_SPS} samples/slot), M=2, {}",
                match detector {
                    SlotDetector::MatchedFilter => "matched filter",
                    SlotDetector::Envelope => "envelope",
                }
            ),
            host: host_id(),
        }
    })
    .collect()
}

fn path(stem: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

#[test]
fn the_filterbank_hot_path_allocates_nothing() {
    let iq = mfsk_signal();
    let demod = MfskDemod::new(orthogonal::params(4));
    let mut energies = [0.0f32; 4];
    for _ in 0..2 {
        for symbol in 0..MFSK_SYMBOLS {
            demod.energies(&iq, 0, symbol, &mut energies);
        }
    }
    assert_no_alloc("MfskDemod::energies", || {
        for symbol in 0..MFSK_SYMBOLS {
            demod.energies(&iq, 0, symbol, &mut energies);
        }
    });
    let mut symbols = Vec::with_capacity(MFSK_SYMBOLS);
    demod.demodulate(&iq, 0, MFSK_SYMBOLS, &mut symbols);
    symbols.clear();
    assert_no_alloc("MfskDemod::estimate_offset + demodulate", || {
        let offset = demod.estimate_offset(&iq, 64);
        demod.demodulate(&iq, offset, MFSK_SYMBOLS, &mut symbols);
    });
}

/// The PPM hot path, both tiers: slot statistics into a caller's slice, and the argmax loop
/// over a reserved sink.
#[test]
fn the_slot_detectors_allocate_nothing() {
    let iq = ppm_signal();
    for detector in [SlotDetector::MatchedFilter, SlotDetector::Envelope] {
        let demod = ppm_catalog::demod(2, PPM_SYMBOLS, detector);
        let mut stats = [0.0f32; 2];
        let mut symbols = Vec::with_capacity(PPM_SYMBOLS);
        for _ in 0..2 {
            demod.demodulate(&iq, 0, PPM_SYMBOLS, &mut symbols);
            symbols.clear();
        }
        assert_no_alloc("PpmDemod::statistics_at", || {
            for symbol in 0..PPM_SYMBOLS {
                demod.statistics_at(&iq, symbol * 2, &mut stats);
            }
        });
        assert_no_alloc("PpmDemod::demodulate", || {
            demod.demodulate(&iq, 0, PPM_SYMBOLS, &mut symbols);
        });
        symbols.clear();
    }
}

/// The attachment's scan path: pre-computed magnitudes through the envelope tier, which is what
/// `channels::adsb` runs per candidate sample of a wideband stream.
#[test]
fn the_magnitude_scan_path_allocates_nothing() {
    let iq = ppm_signal();
    let mut mag = Vec::with_capacity(iq.len());
    sdrmm_modem::ppm::magnitudes(&iq, &mut mag);
    let demod = PpmDemod::new(2, SLOT_SPS, 0, PPM_SYMBOLS, 0.0, SlotDetector::Envelope);
    let mut stats = [0.0f32; 2];
    for symbol in 0..PPM_SYMBOLS {
        demod.envelope_at(&mag, symbol * 2, &mut stats);
    }
    assert_no_alloc("PpmDemod::envelope_at", || {
        for symbol in 0..PPM_SYMBOLS {
            demod.envelope_at(&mag, symbol * 2, &mut stats);
        }
    });
}

/// Rewrites both committed baselines. Run deliberately, on the reference machine:
/// `cargo test -p sdrmm-modem --release --test orthogonal_ppm_perf write_ -- --ignored`.
#[test]
#[ignore = "rewrites the committed baselines; run explicitly in release on the reference host"]
fn write_phase5_perf_baselines() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    for (stem, rows) in [
        (orthogonal::PERF, measured_mfsk()),
        (ppm_catalog::PERF, measured_ppm()),
    ] {
        let path = path(stem);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        save_baselines(&path, &rows).unwrap();
        for row in &rows {
            println!(
                "{}: {:.1} Msamples/s, {:.0}x real time",
                row.bench, row.msamples_per_s, row.realtime_factor
            );
        }
    }
}

/// The nightly perf gate: measured against committed, failing past [`REGRESSION_FRACTION`].
/// Compared only in release and only on the host that wrote the baseline.
#[test]
#[ignore = "nightly perf gate; run in release: cargo test -p sdrmm-modem --release --test orthogonal_ppm_perf compare_ -- --ignored"]
fn compare_phase5_perf_baselines() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    for (stem, measured) in [
        (orthogonal::PERF, measured_mfsk()),
        (ppm_catalog::PERF, measured_ppm()),
    ] {
        let committed = load_baselines(&path(stem)).unwrap();
        if committed.iter().any(|b| b.host != host_id()) {
            eprintln!("skipping {stem}: baseline host is not {}", host_id());
            continue;
        }
        match compare_perf(&measured, &committed, REGRESSION_FRACTION) {
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
}
