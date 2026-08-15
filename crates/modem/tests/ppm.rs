#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        Curve,
        catalog::{
            FULL_ERRORS, orthogonal,
            ppm::{
                ENVELOPE_GRID, ENVELOPE_LIMITS, ENVELOPE_SEED, FULL_CAP, M2_ENVELOPE_AWGN, M2_GRID,
                M2_MATCHED_AWGN, M2_SEED, M4_GRID, M4_MATCHED_AWGN, M4_SEED, MATCHED_LIMITS,
                ORACLE_TOLERANCE_DB, RATE, SLOT_SPS, link_sized, ppm2_envelope_link,
                ppm2_matched_link, ppm4_matched_link, unique_word,
            },
        },
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{Cfo, ChannelSpec, ClockError, Drift, TimingOffset},
        limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
        rng::Rng,
        sweep::{self, Link},
        theory,
    },
    ppm::{PpmDemod, PpmMod, SlotDetector},
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

fn oracle(m: u32) -> impl Fn(f64) -> f64 {
    move |db| theory::mfsk_noncoherent_ber(m, db)
}

#[test]
fn every_chain_round_trips_clean_at_high_ebn0() {
    for (link, name) in [
        (ppm2_matched_link(), "ppm2 matched"),
        (ppm4_matched_link(), "ppm4 matched"),
        (ppm2_envelope_link(), "ppm2 envelope"),
    ] {
        let ber = limits::measure_ber(&link, &ChannelSpec::default(), 25.0, 0x0c1e, 1, 1);
        assert!(ber < 1e-3, "{name} floor {ber} at 25 dB Eb/N0");
    }
}

fn assert_curve_prefix(link: &Link, grid: &[f64], seed: u64, name: &str) {
    let committed = load_curve(name);
    let measured = sweep::sweep_ber(
        link,
        &ChannelSpec::default(),
        &grid[..3],
        seed,
        FULL_ERRORS,
        FULL_CAP,
    );
    let worst = sweep::worst_penalty_db_vs_curve(&measured, &committed, grid[0], grid[2]);
    assert!(worst.abs() < 0.5, "{name} drift vs committed: {worst} dB");
}

#[test]
fn ppm2_matched_curve_matches_committed_baseline() {
    assert_curve_prefix(&ppm2_matched_link(), M2_GRID, M2_SEED, M2_MATCHED_AWGN);
}

#[test]
fn ppm4_matched_curve_matches_committed_baseline() {
    assert_curve_prefix(&ppm4_matched_link(), M4_GRID, M4_SEED, M4_MATCHED_AWGN);
}

#[test]
fn ppm2_envelope_curve_matches_committed_baseline() {
    assert_curve_prefix(
        &ppm2_envelope_link(),
        ENVELOPE_GRID,
        ENVELOPE_SEED,
        M2_ENVELOPE_AWGN,
    );
}

#[test]
fn the_matched_tier_sits_on_the_exact_closed_form() {
    for (m, stem, grid) in [
        (2u32, M2_MATCHED_AWGN, M2_GRID),
        (4, M4_MATCHED_AWGN, M4_GRID),
    ] {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, oracle(m), grid[0], *grid.last().unwrap());
        assert!(
            worst.abs() < ORACLE_TOLERANCE_DB,
            "M = {m}: worst penalty {worst} dB vs exact noncoherent {m}-ary"
        );
    }
}

#[test]
fn the_envelope_tier_sits_the_recorded_margin_behind_the_matched_one() {
    let sensitivity = |stem: &str| {
        limits::ebn0_at_ber(&load_curve(stem), 1e-3).expect("grid must bracket BER 1e-3")
    };
    let margin = sensitivity(M2_ENVELOPE_AWGN) - sensitivity(M2_MATCHED_AWGN);
    assert!(
        (1.0..3.0).contains(&margin),
        "envelope tier is {margin} dB behind the matched one"
    );
}

#[test]
fn ppm_and_mfsk_measure_the_same_sensitivity_at_equal_alphabets() {
    let sensitivity = |stem: &str| {
        limits::ebn0_at_ber(&load_curve(stem), 1e-3).expect("grid must bracket BER 1e-3")
    };
    for (m, ppm_stem, mfsk_stem) in [
        (2, M2_MATCHED_AWGN, orthogonal::M2_AWGN),
        (4, M4_MATCHED_AWGN, orthogonal::M4_AWGN),
    ] {
        let (ppm, mfsk) = (sensitivity(ppm_stem), sensitivity(mfsk_stem));
        assert!(
            (ppm - mfsk).abs() < 0.15,
            "M = {m}: {m}-PPM reads {ppm:.2} dB and {m}-FSK {mfsk:.2} dB at BER 1e-3"
        );
    }
}

fn loopback_at_margin(mut link: Link, curve_name: &str, margin_db: f64, seed: u64) {
    let sensitivity = limits::ebn0_at_ber(&load_curve(curve_name), 1e-3)
        .expect("committed curve must bracket BER 1e-3");
    let payloads = Payloads::new(seed, 4, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, margin_db);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn ppm2_matched_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        link_sized(2, 256, SlotDetector::MatchedFilter),
        M2_MATCHED_AWGN,
        6.0,
        0x2bb2,
    );
}

#[test]
fn ppm4_matched_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        link_sized(4, 256, SlotDetector::MatchedFilter),
        M4_MATCHED_AWGN,
        6.0,
        0x4bb4,
    );
}

#[test]
fn ppm2_envelope_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        link_sized(2, 256, SlotDetector::Envelope),
        M2_ENVELOPE_AWGN,
        6.0,
        0x2bbe,
    );
}

const PROBE_SYMBOLS: usize = 112;
const PROBE_NOISE_VAR: f64 = 0.001;

fn probe_symbols(m: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..PROBE_SYMBOLS)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as usize % m) as u8
        })
        .collect()
}

fn noisy(wave: &mut [Complex<f32>], seed: u64) {
    let mut rng = Rng::new(seed);
    let sigma = (PROBE_NOISE_VAR / 2.0).sqrt();
    for s in wave.iter_mut() {
        *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
    }
}

#[test]
fn every_fractional_rate_and_phase_decodes_through_some_phase_table() {
    for m in [2usize, 4] {
        let symbols = probe_symbols(m);
        for &(sps, allowed) in &[(1.024, 1usize), (1.2, 0), (1.28, 0), (2.5, 0)] {
            for (index, phase) in [0.0, 0.19, 0.37, 0.5, 0.71, 0.93].into_iter().enumerate() {
                let mut wave = Vec::new();
                let mut sent = symbols.clone();
                sent.push(0);
                PpmMod::new(m, sps, phase, 1.0).modulate(&sent, &mut wave);
                noisy(&mut wave, 0x9b0 + index as u64);
                for detector in [SlotDetector::MatchedFilter, SlotDetector::Envelope] {
                    let errors = best_phase_table_errors(m, sps, detector, &wave, &symbols);
                    assert!(
                        errors <= allowed,
                        "M = {m}, {sps} samples/slot, phase {phase}, {detector:?}: the best of \
                         eight phase tables left {errors} of {} symbols wrong (allowed {allowed})",
                        symbols.len()
                    );
                }
            }
        }
    }
}

fn best_phase_table_errors(
    m: usize,
    sps: f64,
    detector: SlotDetector,
    wave: &[Complex<f32>],
    sent: &[u8],
) -> usize {
    PpmDemod::phases(m, sps, 0, sent.len() + 1, 8, detector)
        .iter()
        .map(|receiver| {
            let mut decoded = Vec::with_capacity(sent.len());
            receiver.demodulate(wave, 0, sent.len(), &mut decoded);
            decoded.iter().zip(sent).filter(|(a, b)| a != b).count()
        })
        .min()
        .unwrap_or(usize::MAX)
}

#[test]
fn the_phase_tables_are_not_interchangeable() {
    let m = 2;
    let symbols = probe_symbols(m);
    let mut wave = Vec::new();
    PpmMod::new(m, 1.024, 0.5, 1.0).modulate(&symbols, &mut wave);
    let aligned = PpmDemod::new(m, 1.024, 0, symbols.len(), 0.5, SlotDetector::MatchedFilter);
    let mismatched = PpmDemod::new(m, 1.024, 0, symbols.len(), 0.0, SlotDetector::MatchedFilter);
    let decode = |receiver: &PpmDemod| {
        let mut out = Vec::new();
        receiver.demodulate(&wave, 0, symbols.len(), &mut out);
        out.iter().zip(&symbols).filter(|(a, b)| a != b).count()
    };
    assert_eq!(decode(&aligned), 0);
    assert!(
        decode(&mismatched) > symbols.len() / 10,
        "the phase-0 table read a phase-0.5 burst too well to be a different table"
    );
}

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, M2_SEED ^ 0xbe5, 150, 40_000)
}

fn axis_row(
    axis: &str,
    unit: &str,
    max_axis: f64,
    tolerance: f64,
    ber_at: impl Fn(f64) -> f64,
) -> LimitRow {
    limits::measure_axis_row(
        axis,
        unit,
        Criterion::FailureBer,
        max_axis,
        tolerance,
        ber_at,
    )
}

const PROFILE_ERRORS: u64 = 250;
const PROFILE_CAP: u64 = 600_000;

const CFO_AXIS_HZ: f64 = 1_000_000.0;
const DRIFT_AXIS_HZ_S: f64 = 1e9;

fn measure_rows(link: &Link, op_db: f64, profile_grid: &[f64]) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", CFO_AXIS_HZ, 2_000.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, RATE)),
                op_db,
            )
        }),
        axis_row("frequency drift", "Hz/s", DRIFT_AXIS_HZ_S, 5e6, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, RATE)),
                op_db,
            )
        }),
        axis_row("sample clock", "ppm", 10_000.0, 5.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
            )
        }),
        axis_row("static timing offset", "samples", 32.0, 0.5, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
            )
        }),
        limits::measure_profile_degradation(
            link,
            &ChannelSpec::default(),
            CompositeProfile::StaticIndoor,
            profile_grid,
            M2_SEED ^ 0x51de,
            PROFILE_ERRORS,
            PROFILE_CAP,
        ),
    ]
}

fn assert_table_matches(stem: &str, link: &Link, profile_grid: &[f64]) {
    let committed = limits::load_json(&baseline_path(stem)).unwrap();
    let op_db = committed.operating_point_db().unwrap();
    let measured = measure_rows(link, op_db, profile_grid);
    let mut faults = Vec::new();
    for row in &committed.rows {
        let Some(m) = measured.iter().find(|m| m.axis == row.axis) else {
            faults.push(format!("row '{}' vanished", row.axis));
            continue;
        };
        assert_eq!(
            m.criterion, row.criterion,
            "criterion changed on '{}'",
            row.axis
        );
        assert_eq!(m.unit, row.unit, "unit changed on '{}'", row.axis);
        let worse_by = if row.criterion == limits::DEGRADATION_CRITERION {
            m.threshold - row.threshold
        } else {
            row.threshold - m.threshold
        };
        if m.threshold.is_nan() || worse_by > 0.2 * row.threshold.abs() {
            faults.push(format!(
                "row '{}': committed {} -> measured {} {}",
                row.axis, row.threshold, m.threshold, m.unit
            ));
        }
    }
    assert!(faults.is_empty(), "{stem} regressions: {faults:#?}");
}

const MATCHED_PROFILE_GRID: [f64; 4] = [10.0, 11.0, 12.0, 13.0];
const ENVELOPE_PROFILE_GRID: [f64; 4] = [12.0, 13.0, 14.0, 15.0];

#[test]
fn matched_tier_limits_rows_match_committed_table() {
    assert_table_matches(MATCHED_LIMITS, &ppm2_matched_link(), &MATCHED_PROFILE_GRID);
}

#[test]
fn envelope_tier_limits_rows_match_committed_table() {
    assert_table_matches(
        ENVELOPE_LIMITS,
        &ppm2_envelope_link(),
        &ENVELOPE_PROFILE_GRID,
    );
}

#[test]
fn the_envelope_tier_does_not_fail_on_the_carrier_axes_at_all() {
    let row = |stem: &str, axis: &str| {
        limits::load_json(&baseline_path(stem))
            .unwrap()
            .rows
            .iter()
            .find(|r| r.axis == axis)
            .unwrap_or_else(|| panic!("{stem} carries no '{axis}' row"))
            .threshold
    };
    for (axis, bracket) in [
        ("static CFO", CFO_AXIS_HZ),
        ("frequency drift", DRIFT_AXIS_HZ_S),
    ] {
        let matched = row(MATCHED_LIMITS, axis);
        let envelope = row(ENVELOPE_LIMITS, axis);
        assert!(
            (envelope - bracket).abs() < 1e-9,
            "{axis}: the envelope tier failed at {envelope} inside the {bracket} bracket"
        );
        assert!(
            matched < 0.75 * bracket,
            "{axis}: the matched tier reached {matched}, so the bracket bounded it, not the tier"
        );
    }
}

fn remeasure_curve(link: &Link, grid: &[f64], seed: u64, name: &str) -> Curve {
    let curve = sweep::sweep_ber(
        link,
        &ChannelSpec::default(),
        grid,
        seed,
        FULL_ERRORS,
        FULL_CAP,
    );
    for p in &curve.points {
        println!(
            "{:>5.1} dB  {:>8} / {:<10} BER {:.3e}",
            p.ebn0_db,
            p.errors,
            p.trials,
            p.rate()
        );
    }
    let path = baseline_path(name);
    if path.exists() {
        let committed: Curve = sweep::load_json(&path).unwrap();
        assert_eq!(
            curve.points.len(),
            committed.points.len(),
            "{name}: grid changed"
        );
        for (m, c) in curve.points.iter().zip(&committed.points) {
            assert!((m.ebn0_db - c.ebn0_db).abs() < 1e-9, "{name}: grid changed");
            let ratio = (m.rate().max(1e-12) / c.rate().max(1e-12)).log10().abs();
            assert!(
                ratio < 0.1,
                "{name} at {} dB: committed BER {:.3e}, measured {:.3e}",
                c.ebn0_db,
                c.rate(),
                m.rate()
            );
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        sweep::save_json(&curve, &path).unwrap();
        println!("baseline created at {}", path.display());
    }
    curve
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_ppm2_matched_full() {
    let curve = remeasure_curve(&ppm2_matched_link(), M2_GRID, M2_SEED, M2_MATCHED_AWGN);
    let worst = sweep::worst_penalty_db(&curve, oracle(2), M2_GRID[0], *M2_GRID.last().unwrap());
    println!("worst penalty vs exact noncoherent 2-ary: {worst:+.3} dB");
    assert!(worst.abs() < ORACLE_TOLERANCE_DB, "worst {worst} dB");
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_ppm4_matched_full() {
    let curve = remeasure_curve(&ppm4_matched_link(), M4_GRID, M4_SEED, M4_MATCHED_AWGN);
    let worst = sweep::worst_penalty_db(&curve, oracle(4), M4_GRID[0], *M4_GRID.last().unwrap());
    println!("worst penalty vs exact noncoherent 4-ary: {worst:+.3} dB");
    assert!(worst.abs() < ORACLE_TOLERANCE_DB, "worst {worst} dB");
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_ppm2_envelope_full() {
    remeasure_curve(
        &ppm2_envelope_link(),
        ENVELOPE_GRID,
        ENVELOPE_SEED,
        M2_ENVELOPE_AWGN,
    );
}

fn measure_table_full(
    stem: &str,
    entry: &str,
    link: &Link,
    grid: &[f64],
    seed: u64,
    profile: &[f64],
) {
    let sensitivity = limits::measure_sensitivity(
        link,
        &ChannelSpec::default(),
        grid,
        seed,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new(entry, seed, &sensitivity);
    let op_db = table
        .operating_point_db()
        .expect("grid must bracket BER 1e-3");
    table.rows = measure_rows(link, op_db, profile);
    println!(
        "{entry}: sensitivity 1e-2 {:?}  1e-3 {:?}  1e-4 {:?}",
        table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
    );
    for row in &table.rows {
        println!("{:<24} {:>14.4} {}", row.axis, row.threshold, row.unit);
    }
    let path = baseline_path(stem);
    if path.exists() {
        let committed = limits::load_json(&path).unwrap();
        if let Err(faults) = limits::compare_tables(&table, &committed, 0.2) {
            panic!("limits regressions: {faults:#?}");
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(&table, &path).unwrap();
        println!("baseline created at {}", path.display());
    }
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_matched_limits_full() {
    measure_table_full(
        MATCHED_LIMITS,
        "ppm2-matched-reference",
        &ppm2_matched_link(),
        M2_GRID,
        M2_SEED,
        &MATCHED_PROFILE_GRID,
    );
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_envelope_limits_full() {
    measure_table_full(
        ENVELOPE_LIMITS,
        "ppm2-envelope-reference",
        &ppm2_envelope_link(),
        ENVELOPE_GRID,
        ENVELOPE_SEED,
        &ENVELOPE_PROFILE_GRID,
    );
}

#[test]
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (name, m, link) in [
        ("2-PPM matched", 2u32, ppm2_matched_link()),
        ("4-PPM matched", 4, ppm4_matched_link()),
        ("2-PPM envelope", 2, ppm2_envelope_link()),
    ] {
        let grid: Vec<f64> = (4..=17).map(f64::from).collect();
        let curve = sweep::sweep_ber(&link, &ChannelSpec::default(), &grid, 0x9999, 500, 400_000);
        println!(
            "--- {name} ({SLOT_SPS} samples/slot, word {})",
            unique_word(m as usize).len()
        );
        for p in &curve.points {
            println!(
                "{:>5.1} dB  BER {:.3e}  (orthogonal theory {:.3e})",
                p.ebn0_db,
                p.rate(),
                theory::mfsk_noncoherent_ber(m, p.ebn0_db)
            );
        }
    }
}
