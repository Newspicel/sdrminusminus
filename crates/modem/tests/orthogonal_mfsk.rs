#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sdrmm_modem_test_support::ber::{
    Curve,
    catalog::{
        FULL_ERRORS,
        orthogonal::{
            FULL_CAP, M2_AWGN, M2_GRID, M2_SEED, M4_AWGN, M4_GRID, M4_LIMITS, M4_SEED, M8_AWGN,
            M8_GRID, M8_SEED, ORACLE_TOLERANCE_DB, RATE, link_sized, mfsk2_link, mfsk4_link,
            mfsk8_link,
        },
    },
    e2e::{Payloads, channel_at_margin, loopback},
    impair::{Cfo, ChannelSpec, ClockError, Drift, TimingOffset},
    limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
    sweep::{self, Link},
    theory,
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
fn all_three_chains_round_trip_clean_at_high_ebn0() {
    for (link, name) in [
        (mfsk2_link(), "mfsk2"),
        (mfsk4_link(), "mfsk4"),
        (mfsk8_link(), "mfsk8"),
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
fn mfsk2_curve_matches_committed_baseline() {
    assert_curve_prefix(&mfsk2_link(), M2_GRID, M2_SEED, M2_AWGN);
}

#[test]
fn mfsk4_curve_matches_committed_baseline() {
    assert_curve_prefix(&mfsk4_link(), M4_GRID, M4_SEED, M4_AWGN);
}

#[test]
fn mfsk8_curve_matches_committed_baseline() {
    assert_curve_prefix(&mfsk8_link(), M8_GRID, M8_SEED, M8_AWGN);
}

#[test]
fn every_committed_curve_sits_on_the_exact_closed_form() {
    for (m, stem, grid) in [
        (2u32, M2_AWGN, M2_GRID),
        (4, M4_AWGN, M4_GRID),
        (8, M8_AWGN, M8_GRID),
    ] {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, oracle(m), grid[0], *grid.last().unwrap());
        assert!(
            worst.abs() < ORACLE_TOLERANCE_DB,
            "M = {m}: worst penalty {worst} dB vs exact noncoherent {m}-FSK"
        );
    }
}

#[test]
fn sensitivity_improves_with_the_alphabet() {
    let sensitivity = |stem: &str| {
        limits::ebn0_at_ber(&load_curve(stem), 1e-3).expect("grid must bracket BER 1e-3")
    };
    let (m2, m4, m8) = (
        sensitivity(M2_AWGN),
        sensitivity(M4_AWGN),
        sensitivity(M8_AWGN),
    );
    assert!(m4 < m2 - 0.5, "M=2 {m2} dB, M=4 {m4} dB");
    assert!(m8 < m4 - 0.5, "M=4 {m4} dB, M=8 {m8} dB");
}

fn loopback_at_margin(mut link: Link, curve_name: &str, margin_db: f64, seed: u64) {
    let sensitivity = limits::ebn0_at_ber(&load_curve(curve_name), 1e-3)
        .expect("committed curve must bracket BER 1e-3");
    let payloads = Payloads::new(seed, 4, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, margin_db);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn mfsk2_loops_back_clean_at_6db_margin() {
    loopback_at_margin(link_sized(2, 256), M2_AWGN, 6.0, 0x2e5c);
}

#[test]
fn mfsk4_loops_back_clean_at_6db_margin() {
    loopback_at_margin(link_sized(4, 256), M4_AWGN, 6.0, 0x4e5c);
}

#[test]
fn mfsk8_loops_back_clean_at_6db_margin() {
    loopback_at_margin(link_sized(8, 256), M8_AWGN, 6.0, 0x8e5c);
}

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, M4_SEED ^ 0xbe5, 150, 40_000)
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

const PROFILE_GRID: [f64; 4] = [8.0, 9.0, 10.0, 11.0];
const PROFILE_ERRORS: u64 = 250;
const PROFILE_CAP: u64 = 600_000;

fn measure_rows(link: &Link, op_db: f64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 4_800.0, 25.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, RATE)),
                op_db,
            )
        }),
        axis_row("frequency drift", "Hz/s", 50_000.0, 250.0, |hz_s| {
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
        axis_row("static timing offset", "samples", 50.0, 0.5, |d| {
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
            &PROFILE_GRID,
            M4_SEED ^ 0x51de,
            PROFILE_ERRORS,
            PROFILE_CAP,
        ),
    ]
}

#[test]
fn mfsk4_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path(M4_LIMITS)).unwrap();
    let op_db = committed.operating_point_db().unwrap();
    let link = mfsk4_link();
    let measured = measure_rows(&link, op_db);
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
    assert!(faults.is_empty(), "limits regressions: {faults:#?}");
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

fn measure_full(link: &Link, grid: &[f64], seed: u64, name: &str, m: u32) {
    let curve = remeasure_curve(link, grid, seed, name);
    let worst = sweep::worst_penalty_db(&curve, oracle(m), grid[0], *grid.last().unwrap());
    println!("worst penalty vs exact noncoherent {m}-FSK: {worst:+.3} dB");
    assert!(
        worst.abs() < ORACLE_TOLERANCE_DB,
        "M = {m}: worst penalty {worst} dB"
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk2_full() {
    measure_full(&mfsk2_link(), M2_GRID, M2_SEED, M2_AWGN, 2);
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk4_full() {
    measure_full(&mfsk4_link(), M4_GRID, M4_SEED, M4_AWGN, 4);
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk8_full() {
    measure_full(&mfsk8_link(), M8_GRID, M8_SEED, M8_AWGN, 8);
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_mfsk4_limits_full() {
    let link = mfsk4_link();
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        M4_GRID,
        M4_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("mfsk4-orthogonal-reference", M4_SEED, &sensitivity);
    let op_db = table
        .operating_point_db()
        .expect("grid must bracket BER 1e-3");
    table.rows = measure_rows(&link, op_db);
    println!(
        "sensitivity 1e-2 {:?}  1e-3 {:?}  1e-4 {:?}",
        table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
    );
    for row in &table.rows {
        println!("{:<24} {:>12.4} {}", row.axis, row.threshold, row.unit);
    }
    let path = baseline_path(M4_LIMITS);
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
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (m, link) in [(2u32, mfsk2_link()), (4, mfsk4_link()), (8, mfsk8_link())] {
        let grid: Vec<f64> = (3..=15).map(f64::from).collect();
        let curve = sweep::sweep_ber(&link, &ChannelSpec::default(), &grid, 0x9999, 500, 800_000);
        println!("--- M = {m}");
        for p in &curve.points {
            println!(
                "{:>5.1} dB  BER {:.3e}  (theory {:.3e})",
                p.ebn0_db,
                p.rate(),
                theory::mfsk_noncoherent_ber(m, p.ebn0_db)
            );
        }
    }
}
