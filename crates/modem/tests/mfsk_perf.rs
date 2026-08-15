#![allow(clippy::unwrap_used, clippy::expect_used)]

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        catalog::mfsk::{PERF, RATE, mfsk4_burst, modulate},
        perf::{
            PerfBaseline, REGRESSION_FRACTION, compare_perf, host_id, load_baselines,
            measure_throughput, save_baselines, test_dibits,
        },
    },
    cpm::CpmDemod,
};

fn reference_waveform() -> Vec<Complex<f32>> {
    modulate(&mfsk4_burst(), &test_dibits(2_400, 0x5eed))
}

fn measured_baselines() -> Vec<PerfBaseline> {
    let entry = mfsk4_burst();
    let iq = reference_waveform();
    let mut demod = CpmDemod::new(&entry.params, &entry.receive_filter, entry.timing_bw);
    let mut soft = Vec::with_capacity(iq.len());
    demod.process(&iq, &mut soft);
    soft.clear();
    demod.process(&iq, &mut soft);
    let msamples_per_s = measure_throughput(300, iq.len() as u64, || {
        soft.clear();
        demod.process(&iq, &mut soft);
    });
    vec![PerfBaseline {
        bench: "cpm_demod_m4_48k".into(),
        msamples_per_s,
        realtime_factor: msamples_per_s * 1e6 / RATE,
        config: "48 kHz, 4800 baud, ETSI 4-level ±1944 Hz (h=0.27), RRC α=0.2 span 8, \
                 timing bw 0.015"
            .into(),
        host: host_id(),
    }]
}

fn committed_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{PERF}.json"))
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_cpm_perf_baseline() {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = committed_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    save_baselines(&path, &measured_baselines()).unwrap();
}

#[test]
#[ignore = "nightly perf gate; run in release: cargo test -p sdrmm-modem --release --test mfsk_perf compare_cpm_perf_baseline -- --ignored"]
fn compare_cpm_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    let committed = load_baselines(&committed_path()).unwrap();
    if committed.iter().any(|b| b.host != host_id()) {
        eprintln!("skipping the perf gate: baseline host is not {}", host_id());
        return;
    }
    match compare_perf(&measured_baselines(), &committed, REGRESSION_FRACTION) {
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
