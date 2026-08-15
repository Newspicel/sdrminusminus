#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sdrmm_modem::ber::{
    Curve,
    catalog::{
        FULL_ERRORS,
        mfsk::{
            BURST_FRAMES, BurstRecipe, FULL_CAP, M2_AWGN, M2_GRID, M2_OFFSET_TOL_DB, M2_SEED,
            M2_THEORY_OFFSET_DB, M4_AWGN, M4_GRID, M4_LIMITS, M4_SEED, M8_AWGN, M8_GRID, M8_SEED,
            RATE, mfsk2_link, mfsk2_link_sized, mfsk4_link, mfsk4_link_sized, mfsk8_link,
            mfsk8_link_sized,
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

#[test]
fn all_three_chains_round_trip_clean_at_high_ebn0() {
    for (link, name) in [
        (mfsk2_link(), "mfsk2"),
        (mfsk4_link(), "mfsk4"),
        (mfsk8_link(), "mfsk8"),
    ] {
        let ber = limits::measure_ber(&link, &ChannelSpec::default(), 30.0, 0x0c1e, 1, 1);
        assert!(ber < 5e-3, "{name} floor {ber} at 30 dB Eb/N0");
    }
    let recipe = BurstRecipe::reference(BURST_FRAMES);
    let link = recipe.link("mfsk4 burst floor probe");
    let ber = limits::measure_ber(&link, &recipe.channel(), 30.0, 0x0c1e, 1, 1);
    assert!(ber < 5e-3, "mfsk4 burst floor {ber} at 30 dB Eb/N0");
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
fn mfsk2_committed_curve_sits_at_theory_plus_documented_offset() {
    let committed = load_curve(M2_AWGN);
    let offset = sweep::penalty_db(&committed, |db| theory::mfsk_noncoherent_ber(2, db), 1e-3);
    assert!(
        (offset - M2_THEORY_OFFSET_DB).abs() < M2_OFFSET_TOL_DB,
        "measured offset {offset} dB vs documented {M2_THEORY_OFFSET_DB} dB"
    );
}

fn loopback_at_margin(mut link: Link, curve_name: &str, margin_db: f64, seed: u64) {
    let sensitivity = limits::ebn0_at_ber(&load_curve(curve_name), 1e-3)
        .expect("committed curve must bracket BER 1e-3");
    let payloads = Payloads::new(seed, 2, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, margin_db);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn mfsk2_loops_back_clean_at_6db_margin() {
    loopback_at_margin(mfsk2_link_sized(1_024), M2_AWGN, 6.0, 0x2e2e);
}

#[test]
fn mfsk4_loops_back_clean_at_10db_margin() {
    loopback_at_margin(mfsk4_link_sized(1_024), M4_AWGN, 10.0, 0x4e2e);
}

#[test]
fn mfsk8_loops_back_clean_at_10db_margin() {
    loopback_at_margin(mfsk8_link_sized(2), M8_AWGN, 10.0, 0x8e2e);
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

fn steady_axis_rows(link: &Link, op_db: f64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 6_000.0, 25.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, RATE)),
                op_db,
            )
        }),
        axis_row("frequency drift", "Hz/s", 20_000.0, 100.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, RATE)),
                op_db,
            )
        }),
        axis_row("sample clock", "ppm", 50_000.0, 250.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
            )
        }),
        axis_row("static timing offset", "samples", 10.0, 0.25, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
            )
        }),
    ]
}

fn burst_axis_rows(op_db: f64) -> Vec<LimitRow> {
    vec![
        axis_row("dead time", "symbols", 1_024.0, 16.0, |off| {
            let mut recipe = BurstRecipe::reference(BURST_FRAMES);
            recipe.off_symbols = (off.round() as usize).max(16);
            let link = recipe.link("dead-time probe");
            probe(&link, &recipe.channel(), op_db)
        }),
        axis_row(
            "burst shortening",
            "payload symbols removed (of 108)",
            96.0,
            2.0,
            |removed| {
                let mut recipe = BurstRecipe::reference(BURST_FRAMES);
                recipe.payload_symbols = 108 - (removed.round() as usize).min(96);
                let link = recipe.link("burst-length probe");
                probe(&link, &recipe.channel(), op_db)
            },
        ),
        axis_row(
            "level step",
            "dB attenuation of alternate bursts",
            12.0,
            0.25,
            |db| {
                let mut recipe = BurstRecipe::reference(BURST_FRAMES);
                recipe.level_step_db = -db;
                let link = recipe.link("level-step probe");
                probe(&link, &recipe.channel(), op_db)
            },
        ),
    ]
}

const PROFILE_GRID: [f64; 4] = [12.0, 13.0, 14.0, 15.0];
const PROFILE_ERRORS: u64 = 250;
const PROFILE_CAP: u64 = 1_000_000;

fn profile_row(link: &Link) -> LimitRow {
    limits::measure_profile_degradation(
        link,
        &ChannelSpec::default(),
        CompositeProfile::StaticIndoor,
        &PROFILE_GRID,
        M4_SEED ^ 0x51de,
        PROFILE_ERRORS,
        PROFILE_CAP,
    )
}

fn measure_rows(link: &Link, op_db: f64) -> Vec<LimitRow> {
    let mut rows = steady_axis_rows(link, op_db);
    rows.extend(burst_axis_rows(op_db));
    rows.push(profile_row(link));
    rows
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

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk2_full() {
    let curve = remeasure_curve(&mfsk2_link(), M2_GRID, M2_SEED, M2_AWGN);
    let offset = sweep::penalty_db(&curve, |db| theory::mfsk_noncoherent_ber(2, db), 1e-3);
    println!("offset vs noncoherent 2-FSK theory at 1e-3: {offset:+.3} dB");
    assert!(
        (offset - M2_THEORY_OFFSET_DB).abs() < M2_OFFSET_TOL_DB,
        "measured offset {offset} dB vs documented {M2_THEORY_OFFSET_DB} dB"
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk4_full() {
    remeasure_curve(&mfsk4_link(), M4_GRID, M4_SEED, M4_AWGN);
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_mfsk8_full() {
    remeasure_curve(&mfsk8_link(), M8_GRID, M8_SEED, M8_AWGN);
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
    let mut table = LimitsTable::new("mfsk4-cpfsk-reference", M4_SEED, &sensitivity);
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
    for (link, name) in [
        (mfsk2_link(), "mfsk2"),
        (mfsk4_link(), "mfsk4"),
        (mfsk8_link(), "mfsk8"),
    ] {
        let grid: Vec<f64> = (2..=13).map(|d| f64::from(d) * 2.0).collect();
        let curve = sweep::sweep_ber(&link, &ChannelSpec::default(), &grid, 0x9999, 100, 200_000);
        println!("--- {name}");
        for p in &curve.points {
            println!(
                "{:>5.1} dB  BER {:.3e}  ({}/{})",
                p.ebn0_db,
                p.rate(),
                p.errors,
                p.trials
            );
        }
    }
}
