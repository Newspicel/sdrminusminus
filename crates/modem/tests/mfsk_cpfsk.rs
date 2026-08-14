//! §5 measurement bundle for the M-ary CPFSK catalog entry ( §7 phase 3 accept):
//! committed BER curves for M ∈ {2, 4, 8}, the §4.3 limits table at the M = 4 reference
//! configuration, and the level-1 E2E loopbacks. The chains under measurement live in
//! `ber::catalog::mfsk`; the committed artifacts live in `baselines/cpm/` and regress here:
//!
//! - `mfsk2_cpfsk_awgn.json` — 2FSK h = ½ through discriminator + slicer. Reference policy:
//!   noncoherent orthogonal 2-FSK theory **plus the documented offset** measured below
//!   ([`M2_THEORY_OFFSET_DB`]) — the honest closed-form mapping for this tier, since the
//!   discriminator is neither the coherent nor exactly the noncoherent detector, and the
//!   chain carries its stated framing overhead in Eb.
//! - `mfsk4_cpfsk_awgn.json` — the DMR-like reference configuration, commit-and-guard (no
//!   closed form for partial-response CPM through a discriminator, §4.1). Reviewed at commit
//!   time: monotone waterfall, 1e-2 sensitivity ~7 dB inside the phase-0 chain's 16.9 dB,
//!   and — the headline — the 1e-3 and 1e-4 crossings *exist* on a continuous stream: the
//!   old chain's wander floor is gone at the continuous timing bandwidth (residual ~1e-5).
//! - `mfsk8_cpfsk_awgn.json` — the 8-ary generality gate, commit-and-guard, level scale on
//!   the known-symbol hook.
//! - `mfsk4_limits.json` — §4.3 table at the reference configuration: sensitivities, CFO,
//!   drift, sample clock, static timing, the three burst-survival rows on the TDMA chain,
//!   and the static-indoor composite profile, all under the *default* §4.3 criterion — the
//!   phase-0 chain needed a documented override because its floor sat above 1e-3; this
//!   engine does not.

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

/// The committed artifacts, resolved from this crate's manifest — the registry states them
/// workspace-relative, which is what `cargo xtask ber` and the docs-row rule read.
fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

// --- Always-run harness gates ----------------------------------------------------------------

/// A chain defect (alignment, sign, level scale, hook plumbing) is loud before statistics:
/// with nearly no noise, one trial of every chain sits at or near zero errors. Bound 5e-3 —
/// a mis-slice or misalignment reads tens of percent.
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

/// Smoke tier of a committed curve: the first three grid points re-measured with the
/// committed budgets. (seed, index) names each point's realisation, so a grid prefix
/// reproduces the committed points exactly on one host; 0.5 dB absorbs cross-platform float
/// drift only.
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

/// The M = 2 reference gate, smoke tier: the committed curve itself must sit at theory + the
/// documented offset. Reading the committed artifact costs nothing, and the full writer
/// re-asserts the same bound on fresh measurement.
#[test]
fn mfsk2_committed_curve_sits_at_theory_plus_documented_offset() {
    let committed = load_curve(M2_AWGN);
    let offset = sweep::penalty_db(&committed, |db| theory::mfsk_noncoherent_ber(2, db), 1e-3);
    assert!(
        (offset - M2_THEORY_OFFSET_DB).abs() < M2_OFFSET_TOL_DB,
        "measured offset {offset} dB vs documented {M2_THEORY_OFFSET_DB} dB"
    );
}

// --- Level-1 E2E ( §4.4) -----------------------------------------------------------

/// The §5 item-7 property: payloads survive bit-for-bit at a stated margin over each
/// entry's own measured 1e-3 sensitivity (read off the committed curve, so the margin
/// tightens if the detector improves). The payload budget honours the e2e module's rule that
/// residual BER × total bits ≪ 1: unlike the phase-0 BPSK link, whose residual at +6 dB is
/// ~3e-10, this entry's discriminator tier carries a real continuous-mode residual (the
/// engine's measured ~1e-5 timing self-noise floor at M = 4, ~1e-4 at M = 8's 14 % margins)
/// that no margin buys back — so perfection is demanded over few, short payloads at a wide
/// margin, and the committed curves carry the statistics the loopback cannot.
fn loopback_at_margin(mut link: Link, curve_name: &str, margin_db: f64, seed: u64) {
    let sensitivity = limits::ebn0_at_ber(&load_curve(curve_name), 1e-3)
        .expect("committed curve must bracket BER 1e-3");
    let payloads = Payloads::new(seed, 2, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, margin_db);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

/// M = 2 has no measurable floor (0 errors in 2e5 bits from 16 dB up), so +6 dB over the
/// ~12.4 dB sensitivity leaves ~zero expected errors across the 2048 trial bits.
#[test]
fn mfsk2_loops_back_clean_at_6db_margin() {
    loopback_at_margin(mfsk2_link_sized(1_024), M2_AWGN, 6.0, 0x2e2e);
}

/// +10 dB over the 13.7 dB sensitivity operates near the ~1e-5 clean residual: ~0.06
/// expected errors over 2 × 2048 bits.
#[test]
fn mfsk4_loops_back_clean_at_10db_margin() {
    loopback_at_margin(mfsk4_link_sized(1_024), M4_AWGN, 10.0, 0x4e2e);
}

/// +10 dB over the 18.1 dB sensitivity; the 8-level residual is ~1e-4, so the budget is 2
/// payloads of 2 hook-framed blocks (2 × 672 bits): ~0.13 expected errors.
#[test]
fn mfsk8_loops_back_clean_at_10db_margin() {
    loopback_at_margin(mfsk8_link_sized(2), M8_AWGN, 10.0, 0x8e2e);
}

// --- Limits table (§4.3, M = 4 reference configuration) --------------------------------------

/// One seeded probe at the operating point. 150 errors separates a passing probe (clean BER
/// ~1e-4 at sensitivity + 3 dB) from the 1e-2 limit unambiguously; the cap bounds a probe
/// that fails hard.
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

/// The §4.3 burst-survival axes on the TDMA chain: each probe rebuilds the link, because the
/// searched value must reshape the transmitter and the Eb accounting together.
fn burst_axis_rows(op_db: f64) -> Vec<LimitRow> {
    vec![
        axis_row("dead time", "symbols", 1_024.0, 16.0, |off| {
            let mut recipe = BurstRecipe::reference(BURST_FRAMES);
            recipe.off_symbols = (off.round() as usize).max(16);
            let link = recipe.link("dead-time probe");
            probe(&link, &recipe.channel(), op_db)
        }),
        // "Minimum burst length" spelled so higher stays better for the comparator: payload
        // symbols removable from the 108-symbol burst; min burst = 24-sym sync + remainder.
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
        // Attenuation of alternate bursts: the decay-limited direction of the level tracker,
        // recovered (or not) within each burst's own sync via the known-symbol hook.
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

/// Grid and budget of the composite-profile degradation row, shared by the full writer and
/// the smoke re-measurement so the two measure the same quantity. The grid brackets the 1e-3
/// crossing for the clean chain and leaves headroom for the profile's shift.
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

/// Smoke tier of the limits table: every committed row re-measured with the committed
/// budgets must sit within 20% of its committed threshold, one-sided — moving better is
/// never a failure. The operating point comes from the committed table, so the smoke run
/// pays for no sensitivity resweep; the curve smoke test guards that number.
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
        // Degradation rows grow when things worsen; every axis threshold shrinks.
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

// --- Full re-measurement (nightly; regenerates the committed artifacts) ----------------------

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
        // Point-by-point in rate: same seeds and budgets make each point a reproduction of
        // the committed one, so the ratio allowance is for cross-host float drift only.
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

/// Run in release:
/// `cargo test -p sdrmm-modem --release --test mfsk_cpfsk -- --ignored measure_mfsk`.
/// The M = 2 writer is also the full reference gate: theory + documented offset.
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

/// The full §4.3 table. The sensitivity sweep is parameter-identical to the committed M = 4
/// curve (same link, grid, seed, budgets), so the smoke tier reads the operating point off
/// the committed table while the curve smoke test guards the underlying number.
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

// --- Exploration (never asserted; kept ignored for grid bracketing) --------------------------

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
