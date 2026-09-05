#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_modem::{
    constellation::tables,
    linear::{
        CarrierLoop, EnvelopeDemod, EnvelopeTiming, LinearBurstDemod, LinearDemod, LinearMod,
        LinearParams, LinearTiming, PhaseDetector, TIMING_BW_CONTINUOUS,
    },
};
use sdrmm_modem_test_support::ber::{
    Curve, MIN_ERRORS_PER_POINT,
    catalog::{self, DRIFT_TOLERANCE_DB, Entry, Measurement, linear},
    e2e::{Payloads, channel_at_margin, loopback},
    impair::{Cfo, ChannelSpec, ClockError, Drift, IqImbalance, TimingOffset},
    limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
    perf::{self, PerfBaseline},
    rng::Rng,
    sweep::{self, Link},
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

fn linear_entries() -> Vec<&'static Entry> {
    catalog::ENTRIES
        .iter()
        .filter(|e| e.stem_prefix_is_linear())
        .collect()
}

fn linear_measurements() -> Vec<&'static Measurement> {
    linear_entries()
        .into_iter()
        .flat_map(|e| e.measurements.iter())
        .collect()
}

#[test]
fn every_linear_row_is_registered_and_committed() {
    let entries = linear_entries();
    let names: Vec<&str> = entries.iter().map(|e| e.name).collect();
    assert_eq!(
        names,
        [
            "ask",
            "psk",
            "dpsk",
            "oqpsk",
            "pi4-dqpsk",
            "qam",
            "qam-cross",
            "qam-star",
            "qam-nonuniform",
            "apsk",
        ],
        "the §6 linear rows and the registry have drifted apart"
    );
    for m in linear_measurements() {
        assert!(
            baseline_path(m.stem).is_file(),
            "{}: committed artifact missing",
            m.stem
        );
    }
}

#[test]
fn every_chain_round_trips_a_noiseless_payload() {
    for m in linear_measurements() {
        let mut link = (m.link)();
        let payloads = Payloads::new(0x0c1e, 1, link.bits_per_trial);
        let mut channel = ChannelSpec::default().build();
        assert_eq!(
            loopback(&mut link, &mut channel, payloads),
            Ok(()),
            "{}: noiseless round trip",
            m.stem
        );
    }
}

#[test]
fn every_committed_curve_reproduces_its_smoke_prefix() {
    for m in linear_measurements() {
        let link = (m.link)();
        let tier = m.tier(false);
        let measured = sweep::sweep_ber(
            &link,
            &ChannelSpec::default(),
            tier.grid,
            tier.seed,
            tier.min_errors,
            tier.max_trial_bits,
        );
        let committed = load_curve(m.stem);
        let drift = m.drift_db(&measured, &committed).unwrap();
        assert!(
            drift.abs() < DRIFT_TOLERANCE_DB,
            "{}: smoke prefix drifted {drift:+.3} dB from the committed curve",
            m.stem
        );
    }
}

#[test]
fn every_committed_curve_is_monotone() {
    for m in linear_measurements() {
        let curve = load_curve(m.stem);
        for pair in curve.points.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.errors == 0 || b.errors == 0 {
                continue;
            }
            assert!(
                b.rate() <= a.rate() * 1.05,
                "{}: {:.1} dB reads {:.3e} but {:.1} dB reads {:.3e}",
                m.stem,
                a.ebn0_db,
                a.rate(),
                b.ebn0_db,
                b.rate()
            );
        }
    }
}

#[test]
fn every_committed_point_is_above_the_error_floor() {
    for m in linear_measurements() {
        for p in &load_curve(m.stem).points {
            assert!(
                p.errors >= MIN_ERRORS_PER_POINT,
                "{}: point {p:?} is under the error floor",
                m.stem
            );
        }
    }
}

#[test]
fn every_committed_curve_sits_at_its_reference() {
    let mut checked = 0usize;
    for m in linear_measurements() {
        let curve = load_curve(m.stem);
        if let Some((what, gap, tolerance)) = m.reference_gap(&curve) {
            assert!(
                gap.abs() < tolerance,
                "{}: {gap:+.3} dB from {what} (tolerance {tolerance})",
                m.stem
            );
            checked += 1;
        }
    }
    assert!(
        checked >= 16,
        "only {checked} rows carry a closed-form or table-driven reference"
    );
}

fn crossing(stem: &str, ber: f64) -> f64 {
    limits::ebn0_at_ber(&load_curve(stem), ber)
        .unwrap_or_else(|| panic!("{stem} never crosses {ber:e}"))
}

#[test]
fn coherent_ook_beats_the_envelope_tier_by_its_committed_margin() {
    let coherent = crossing(catalog::ask::OOK_COHERENT_AWGN, 1e-3);
    let envelope = crossing(catalog::ask::OOK_ENVELOPE_AWGN, 1e-3);
    let margin = envelope - coherent;
    assert!(
        margin > 0.7,
        "coherent OOK 1e-3 at {coherent:.2} dB, envelope at {envelope:.2} dB — margin {margin:.2} dB"
    );
    assert!(margin < 4.0, "margin {margin:.2} dB is implausibly large");
}

#[test]
fn feedforward_timing_beats_the_tracking_loop_by_its_committed_margin() {
    let feedforward = crossing(catalog::qam::QAM16_AWGN, 1e-3);
    let tracked = crossing(catalog::qam::QAM16_TRACKED_AWGN, 1e-3);
    let margin = tracked - feedforward;
    assert!(
        margin > 0.8,
        "feedforward 1e-3 at {feedforward:.2} dB, tracked at {tracked:.2} dB — margin {margin:.2} dB"
    );
}

#[test]
fn coherent_pi4_dqpsk_beats_the_differential_tier_by_its_committed_margin() {
    let coherent = crossing(catalog::psk::PI4_DQPSK_COHERENT_AWGN, 1e-3);
    let differential = crossing(catalog::psk::PI4_DQPSK_AWGN, 1e-3);
    let margin = differential - coherent;
    assert!(
        (1.5..3.5).contains(&margin),
        "coherent 1e-3 at {coherent:.2} dB, differential at {differential:.2} dB — margin {margin:.2} dB"
    );
}

#[test]
fn the_offset_rows_sit_on_their_unstaggered_twins() {
    for (offset, plain, name) in [
        (
            catalog::psk::OQPSK_AWGN,
            catalog::psk::QPSK_AWGN,
            "OQPSK vs QPSK",
        ),
        (
            catalog::psk::PI2_BPSK_AWGN,
            catalog::psk::BPSK_AWGN,
            "π/2-BPSK vs BPSK",
        ),
    ] {
        let gap = crossing(offset, 1e-3) - crossing(plain, 1e-3);
        assert!(
            gap.abs() < 0.4,
            "{name}: the offset row sits {gap:+.2} dB from its twin"
        );
    }
}

#[test]
fn the_differential_family_pays_its_documented_penalty() {
    for (differential, coherent, name, range) in [
        (
            catalog::psk::DBPSK_AWGN,
            catalog::psk::BPSK_AWGN,
            "DBPSK vs BPSK",
            0.5..1.6,
        ),
        (
            catalog::psk::DQPSK_AWGN,
            catalog::psk::QPSK_AWGN,
            "DQPSK vs QPSK",
            2.0..3.6,
        ),
    ] {
        let penalty = crossing(differential, 1e-3) - crossing(coherent, 1e-3);
        assert!(
            range.contains(&penalty),
            "{name}: {penalty:+.2} dB, expected {range:?}"
        );
    }
}

const LOOPBACK_MARGIN_DB: f64 = 8.0;

const NO_CLEAN_LOOPBACK: [&str; 1] = [catalog::qam::QAM16_TRACKED_AWGN];

#[test]
fn every_entry_loops_back_clean_at_its_stated_margin() {
    for m in linear_measurements() {
        if NO_CLEAN_LOOPBACK.contains(&m.stem) {
            continue;
        }
        let sensitivity = crossing(m.stem, 1e-3);
        let mut link = (m.link)();
        let payloads = Payloads::new(0x00e2e, 5, link.bits_per_trial);
        let mut channel = channel_at_margin(
            &ChannelSpec::default(),
            &link,
            sensitivity,
            LOOPBACK_MARGIN_DB,
        );
        assert_eq!(
            loopback(&mut link, &mut channel, payloads),
            Ok(()),
            "{}: loopback at +{LOOPBACK_MARGIN_DB} dB over {sensitivity:.2} dB",
            m.stem
        );
    }
}

const PROBE_ERRORS: u64 = 200;
const PROBE_BITS: u64 = 1_500_000;

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, 0x11c5, PROBE_ERRORS, PROBE_BITS)
}

struct Axis<'a> {
    name: &'a str,
    unit: &'a str,
    max: f64,
    tolerance: f64,
}

fn axis_row(
    axis: &Axis<'_>,
    criterion: Criterion,
    link: &Link,
    op_db: f64,
    build: impl Fn(f64) -> ChannelSpec,
) -> LimitRow {
    limits::measure_axis_row(
        axis.name,
        axis.unit,
        criterion,
        axis.max,
        axis.tolerance,
        |value| probe(link, &build(value), op_db),
    )
}

fn axis_rows(link: &Link, op_db: f64, clean: &Curve) -> Vec<LimitRow> {
    let penalty = limits::penalty_criterion(clean, op_db, 1.0)
        .expect("the clean sweep must cover the operating point minus 1 dB");
    let rate = linear::RATE;
    vec![
        axis_row(
            &Axis {
                name: "static CFO",
                unit: "Hz",
                max: 2_000.0,
                tolerance: 0.2,
            },
            penalty,
            link,
            op_db,
            move |hz| ChannelSpec::default().cfo(Cfo::from_hz(hz, rate)),
        ),
        axis_row(
            &Axis {
                name: "frequency drift",
                unit: "Hz/s",
                max: 50_000.0,
                tolerance: 5.0,
            },
            penalty,
            link,
            op_db,
            move |hz_per_s| ChannelSpec::default().drift(Drift::from_hz_per_s(hz_per_s, rate)),
        ),
        axis_row(
            &Axis {
                name: "sample clock",
                unit: "ppm",
                max: 20_000.0,
                tolerance: 1.0,
            },
            penalty,
            link,
            op_db,
            |ppm| ChannelSpec::default().clock(ClockError::new(ppm)),
        ),
        axis_row(
            &Axis {
                name: "static timing offset",
                unit: "fraction of a symbol",
                max: 0.5,
                tolerance: 0.005,
            },
            penalty,
            link,
            op_db,
            |fraction| {
                ChannelSpec::default()
                    .timing_offset(TimingOffset::new(fraction * linear::SPS as f64))
            },
        ),
        axis_row(
            &Axis {
                name: "IQ gain imbalance",
                unit: "dB",
                max: 6.0,
                tolerance: 0.02,
            },
            penalty,
            link,
            op_db,
            |db| ChannelSpec::default().iq_imbalance(IqImbalance::new(db, 0.0)),
        ),
        axis_row(
            &Axis {
                name: "IQ phase imbalance",
                unit: "degrees",
                max: 30.0,
                tolerance: 0.1,
            },
            penalty,
            link,
            op_db,
            |deg| ChannelSpec::default().iq_imbalance(IqImbalance::new(0.0, deg)),
        ),
    ]
}

fn measure_limits(entry: &str, link: &Link, grid: &[f64], seed: u64) -> LimitsTable {
    let sensitivity = limits::measure_sensitivity(
        link,
        &ChannelSpec::default(),
        grid,
        seed,
        catalog::FULL_ERRORS,
        linear::FULL_CAP,
    );
    let mut table = LimitsTable::new(entry, seed, &sensitivity);
    let op_db = table
        .operating_point_db()
        .expect("the grid must bracket the 1e-3 sensitivity");
    table.rows = axis_rows(link, op_db, &sensitivity.curve);
    table.rows.push(limits::measure_profile_degradation(
        link,
        &ChannelSpec::default(),
        CompositeProfile::StaticIndoor,
        grid,
        seed,
        catalog::FULL_ERRORS,
        linear::FULL_CAP,
    ));
    table
}

const QPSK_LIMITS_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
const QAM16_LIMITS_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0];
const OOK_LIMITS_GRID: &[f64] = &[8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0];

fn limits_targets() -> Vec<(&'static str, Link, &'static [f64], u64)> {
    vec![
        (
            catalog::psk::PSK_LIMITS,
            catalog::psk::qpsk_link(),
            QPSK_LIMITS_GRID,
            0x9b53,
        ),
        (
            catalog::qam::QAM_LIMITS,
            catalog::qam::qam16_link(),
            QAM16_LIMITS_GRID,
            0x9a16,
        ),
        (
            catalog::ask::OOK_LIMITS,
            catalog::ask::ook_envelope_link(),
            OOK_LIMITS_GRID,
            0x00e5,
        ),
    ]
}

#[test]
#[ignore = "full limits run; run in release: cargo test -p sdrmm-modem --release --test linear_bundles limits -- --ignored"]
fn limits_tables_match_committed() {
    for (stem, link, grid, seed) in limits_targets() {
        let measured = measure_limits(stem, &link, grid, seed);
        let committed = limits::load_json(&baseline_path(stem)).unwrap();
        if let Err(faults) = limits::compare_tables(&measured, &committed, 0.20) {
            panic!("{stem} regressed:\n{}", faults.join("\n"));
        }
    }
}

fn measure_tier(
    bench: &str,
    config: &str,
    params: &LinearParams,
    mut run: impl FnMut(&[Complex<f32>]),
    symbols: usize,
) -> PerfBaseline {
    let mut rng = Rng::new(0x0dd5);
    let m = params.constellation().len() as u64;
    let labels: Vec<u32> = (0..symbols).map(|_| (rng.next_u64() % m) as u32).collect();
    let iq = LinearMod::transmission(params, &labels);
    run(&iq);
    run(&iq);
    let msps = perf::measure_throughput(200, iq.len() as u64, || run(&iq));
    PerfBaseline {
        bench: bench.into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / linear::RATE,
        config: config.into(),
        host: perf::host_id(),
    }
}

fn measured_coherent_perf() -> Vec<PerfBaseline> {
    let rx = linear::rrc();
    let mut out = Vec::new();
    for (bench, m, loop_bw) in [
        ("linear_qpsk_burst", 4u32, catalog::psk::PSK_LOOP_BW),
        ("linear_qam16_burst", 16, catalog::qam::LOOP_BW_16),
        ("linear_qam1024_burst", 1024, catalog::qam::LOOP_BW_1024),
    ] {
        let params = linear::params(tables::qam_square(m), 0.0, false);
        let mut demod = LinearBurstDemod::new(
            &params,
            &rx,
            linear::POWER_SYMBOLS,
            Some(CarrierLoop::new(PhaseDetector::DecisionDirected, loop_bw)),
        );
        let mut symbols = Vec::with_capacity(4_096);
        out.push(measure_tier(
            bench,
            &format!(
                "{m}-QAM, 8 sps, RRC α=0.35 span 8, feedforward timing, decision-directed \
                 Costas (bw {loop_bw}), held power estimate"
            ),
            &params,
            |iq| {
                symbols.clear();
                demod.process(iq, &mut symbols);
            },
            2_048,
        ));
    }
    let params = linear::params(tables::qam_square(16), 0.0, false);
    let mut demod = LinearDemod::new(
        &params,
        &rx,
        LinearTiming::BURST,
        Some(CarrierLoop::new(
            PhaseDetector::DecisionDirected,
            catalog::qam::LOOP_BW_16,
        )),
    );
    let mut symbols = Vec::with_capacity(4_096);
    out.push(measure_tier(
        "linear_qam16_tracked",
        "16-QAM, 8 sps, RRC α=0.35 span 8, SymbolSync (bw 0.015), decision-directed Costas",
        &params,
        |iq| {
            symbols.clear();
            demod.process(iq, &mut symbols);
        },
        2_048,
    ));
    out
}

fn measured_envelope_perf() -> Vec<PerfBaseline> {
    let rx = linear::rrc();
    let params = linear::params(tables::ook(), 0.0, false);
    let mut demod = EnvelopeDemod::new(
        &params,
        &rx,
        TIMING_BW_CONTINUOUS,
        EnvelopeTiming::CONTINUOUS,
    );
    let mut amplitudes = Vec::with_capacity(4_096);
    vec![measure_tier(
        "linear_ook_envelope",
        "OOK, 8 sps, RRC α=0.35 span 8, magnitude + DC removal + tracking timing",
        &params,
        |iq| {
            amplitudes.clear();
            demod.process(iq, &mut amplitudes);
        },
        2_048,
    )]
}

fn write_perf(stem: &str, measured: &[PerfBaseline]) {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = baseline_path(stem);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    perf::save_baselines(&path, measured).unwrap();
}

fn compare_perf_baseline(stem: &str, measured: &[PerfBaseline]) {
    let committed = perf::load_baselines(&baseline_path(stem)).unwrap();
    if committed.iter().any(|b| b.host != perf::host_id()) {
        eprintln!(
            "skipping the perf gate: baseline host is not {}",
            perf::host_id()
        );
        return;
    }
    match perf::compare_perf(measured, &committed, perf::REGRESSION_FRACTION) {
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
            "{stem} throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * perf::REGRESSION_FRACTION
        ),
    }
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_perf_baselines() {
    write_perf(catalog::psk::PSK_PERF, &measured_coherent_perf());
    write_perf(catalog::ask::ASK_PERF, &measured_envelope_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_perf_baselines() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf_baseline(catalog::psk::PSK_PERF, &measured_coherent_perf());
    compare_perf_baseline(catalog::ask::ASK_PERF, &measured_envelope_perf());
}

fn write_curve(m: &Measurement) {
    let link = (m.link)();
    let tier = m.tier(true);
    let curve = sweep::sweep_ber(
        &link,
        &ChannelSpec::default(),
        tier.grid,
        tier.seed,
        tier.min_errors,
        tier.max_trial_bits,
    );
    for p in &curve.points {
        println!(
            "{:<32} {:>5.1} dB  {:>8} / {:<10} BER {:.3e}",
            m.stem,
            p.ebn0_db,
            p.errors,
            p.trials,
            p.rate()
        );
    }
    if let Some((what, gap, tolerance)) = m.reference_gap(&curve) {
        println!("{:<32} {gap:+.3} dB vs {what} (tol {tolerance})", m.stem);
    }
    let path = baseline_path(m.stem);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    sweep::save_json(&curve, &path).unwrap();
}

#[test]
#[ignore = "full sweeps; run in release to (re)generate every committed linear curve"]
fn measure_every_curve_full() {
    for m in linear_measurements() {
        write_curve(m);
    }
}

#[test]
#[ignore = "diagnostic"]
fn diag_probe_budget() {
    for (stem, link, grid, _seed) in limits_targets() {
        let clean = load_curve(&stem.replace("_limits", "_awgn"));
        let _ = grid;
        let sens = limits::ebn0_at_ber(&clean, 1e-3).unwrap();
        let op = sens + 3.0;
        let max_ber = limits::ber_at_ebn0(&clean, op - 1.0);
        for (bits, errors) in [(200_000u64, 60u64), (1_000_000, 200), (4_000_000, 400)] {
            let ber = limits::measure_ber(&link, &ChannelSpec::default(), op, 0x11c5, errors, bits);
            println!("{stem}: op {op:.2} dB, pass<= {max_ber:?}, probe({bits}) = {ber:.3e}");
        }
    }
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed tables"]
fn measure_limits_full() {
    for (stem, link, grid, seed) in limits_targets() {
        let table = measure_limits(stem, &link, grid, seed);
        println!("{stem}: {table:#?}");
        let path = baseline_path(stem);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(&table, &path).unwrap();
    }
}
