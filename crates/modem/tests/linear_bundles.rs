//! §5 measurement bundle for every linear catalog entry (MODEM-PLAN §7 phase 4 accept): the
//! committed BER curves, the §4.3 limits tables, the perf baselines and the level-1 E2E loopbacks.
//! The chains under measurement live in `ber::catalog::{ask, psk, qam}` on the shared
//! `ber::catalog::linear` substrate; the committed artifacts live in `baselines/linear/`.
//!
//! **What each class of gate here is for.**
//!
//! - *Always-run:* a noiseless round trip of every registered chain (a defect in alignment, sign,
//!   rotation schedule or differential pairing is loud before any statistics), and the smoke tier
//!   of every committed curve — its first three grid points, re-measured at the committed seed and
//!   budget, so a grid *prefix* reproduces the committed points exactly.
//! - *Reference gates:* every closed-form row is held to its oracle across its whole grid; the
//!   exotic geometries are held to the table-driven union bound. These run on the full sweep.
//! - *Tier gates (§5 item 2):* the second detector of an entry is measured against the first, and
//!   the margin is committed. Three of them: coherent OOK over envelope OOK, feedforward timing
//!   over tracking timing on 16-QAM, and π/4-DQPSK coherent over π/4-DQPSK differential.
//! - *Regeneration:* `#[ignore]`d, run in release to rewrite the committed artifacts.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        Curve, MIN_ERRORS_PER_POINT,
        catalog::{self, DRIFT_TOLERANCE_DB, Entry, Measurement, linear},
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{Cfo, ChannelSpec, ClockError, Drift, IqImbalance, TimingOffset},
        limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
        perf::{self, PerfBaseline},
        rng::Rng,
        sweep::{self, Link},
    },
    constellation::tables,
    linear::{
        CarrierLoop, EnvelopeDemod, EnvelopeTiming, LinearBurstDemod, LinearDemod, LinearMod,
        LinearParams, LinearTiming, PhaseDetector, TIMING_BW_CONTINUOUS,
    },
};

/// The committed artifacts, resolved from this crate's manifest — the registry states them
/// workspace-relative, which is what `cargo xtask ber` and the docs-row rule read.
fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

/// Every registered entry whose artifacts live under `baselines/linear/`.
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

// --- Always-run harness gates ----------------------------------------------------------------

/// The phase's headline structural claim, asserted as a count: every linear row of §6 has a
/// registered, committed measurement. A row that quietly lost its runner would still pass every
/// other gate in this file.
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

/// A chain defect is loud before statistics: with no noise at all, every registered chain returns
/// its payload bit for bit. This is the gate that caught the receiver correlating against rotated
/// word points, the differential reference symbol, and the star table's difference rule.
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

/// Smoke tier of every committed curve: the first [`SMOKE_POINTS`](catalog::SMOKE_POINTS) grid
/// points re-measured with the committed budgets. `(seed, index)` names each point's realisation,
/// so a grid prefix reproduces the committed points exactly on one host;
/// [`DRIFT_TOLERANCE_DB`] absorbs cross-platform float drift only.
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

/// Every committed curve descends. A waterfall that rises inside itself is a chain that fell over
/// somewhere on the grid, and it is the one shape a horizontal-distance gate can miss.
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

/// Every committed point carries enough errors to mean something. The budget is 2000; the floor
/// asserted is 100, which is where a point's vertical confidence interval is ±20 %.
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

// --- Reference gates (§4.1) --------------------------------------------------------------------

/// Every committed curve against its §4.1 reference: the closed forms for the regular families,
/// the table-driven union bound for the geometries that have none. Read off the *committed*
/// artifact rather than a fresh sweep, so this gate is fast and states what was reviewed.
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

// --- Tier gates (§5 item 2) --------------------------------------------------------------------

/// Eb/N0 a committed curve crosses `ber` at.
fn crossing(stem: &str, ber: f64) -> f64 {
    limits::ebn0_at_ber(&load_curve(stem), ber)
        .unwrap_or_else(|| panic!("{stem} never crosses {ber:e}"))
}

/// Coherent OOK against the noncoherent envelope tier. The gap is what a carrier reference is
/// worth to a keyed carrier — and the reason the multilevel amplitude rows are measured coherently
/// (see `catalog::ask`).
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

/// Feedforward timing against the tracking loop, on the identical 16-QAM chain. This is the
/// measurement that put the high-order rows on the feedforward tier: the tracking loop's residual
/// jitter is an error floor, not a shift, so the margin at 1e-3 understates it.
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

/// π/4-DQPSK coherent against π/4-DQPSK differential — the acceptance MODEM-PLAN §7 phase 4 states
/// in as many words ("coherent beats differential by the measured, recorded margin"). Measured
/// 1.76 dB at 1e-3, not the asymptotic 3: differential detection's penalty grows toward 3 dB only
/// deep in the tail, and this tier pays a little of it back for the 8th-power detector its
/// rotation forces (see `catalog::psk`'s `pi2_bpsk_link` docs for why the order is 8).
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

/// The offset axis must cost nothing: OQPSK on QPSK's curve and π/2-BPSK on BPSK's, both within
/// the counting noise of a 2000-error point. What the stagger and the rotation buy is envelope
/// (asserted in `linear::modulator`), and a sensitivity that moved would be a defect.
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

/// The differential family's ~3 dB against its coherent counterparts, read off the committed
/// curves. Not a tier comparison of one entry — DPSK's data *is* the difference, so it has no
/// coherent tier — but the family relationship the catalog states, and worth holding to a number.
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

// --- Level-1 E2E (§4.4) -------------------------------------------------------------------------

/// Margin above each entry's own measured 1e-3 sensitivity at which its loopback must be perfect.
/// 8 dB rather than the CPM rows' 6: these payloads are 4096 symbols, so a trial carries up to
/// 40 960 bits and the residual BER has to be that much smaller for `residual × bits ≪ 1`.
const LOOPBACK_MARGIN_DB: f64 = 8.0;

/// The one row with no clean loopback at any margin, and the reason: the tracking-timing
/// comparison chain has an *error floor*, not a shifted waterfall — its residual timing jitter
/// does not shrink with SNR — so "perfect at a stated margin" is not a property it has. That is
/// precisely the finding the row exists to record, and exempting it here is cheaper than pretending
/// a margin exists.
const NO_CLEAN_LOOPBACK: [&str; 1] = [catalog::qam::QAM16_TRACKED_AWGN];

/// Level-1 E2E for every entry: 5 payloads survive the channel bit for bit at
/// [`LOOPBACK_MARGIN_DB`] above the committed sensitivity. Because `tx.rs` drives the same
/// modulators, a green loopback is also the transmit path's correctness test.
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

// --- §4.3 limits tables ---------------------------------------------------------------------------

/// Probe budget per axis point: enough to separate the failure floor from the operating BER,
/// cheap enough that a ~16-probe bisection stays fast.
const PROBE_ERRORS: u64 = 200;
const PROBE_BITS: u64 = 1_500_000;

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, 0x11c5, PROBE_ERRORS, PROBE_BITS)
}

/// One axis of a limits table: what it is called, the bracket and resolution its bisection uses,
/// and how a value becomes a channel. Bundled into a struct because the alternative is an
/// eight-argument function, and at eight arguments a call site stops saying which number is which.
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

/// The §4.3 axis rows every linear entry reports, at the entry's own operating point.
///
/// Two axes here that the CPM tables do not carry, because they are the *linear* failure modes: IQ
/// gain and phase imbalance distort a constellation without touching a discriminator's output at
/// all.
///
/// Search tolerances are tight — 0.2 Hz on CFO, 1 ppm on the clock — for a reason found by
/// measurement: at 20 ppm resolution the 16-QAM clock row bisected to exactly 0, which reads as
/// "tolerates nothing" when the truth was "tolerates less than the search could see". A limits
/// table's zeros have to be real.
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

/// The three committed limits tables and the chains behind them: the coherent reference
/// configuration (QPSK), the densest table a coherent loop carries (16-QAM), and the noncoherent
/// tier (envelope OOK). One per *tier*, not one per row: the axes measure the receiver, and every
/// row on a tier runs the same receiver with a different table.
/// Sensitivity grids for the limits runs, *wider* than the committed curves' by 3 dB at the top.
/// The reason is structural: every axis row is measured at sensitivity(1e-3) + 3 dB, and the ≤1 dB
/// penalty criterion needs the clean curve read one further dB below that — so the grid has to
/// reach past the operating point, which a curve grid chosen to end where its points still carry
/// 100 errors does not.
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

// --- §4.2 perf baselines -------------------------------------------------------------------------

/// Warmed-up throughput of one chain's steady-state `process` path, per the `ber::perf`
/// convention: two warm-up calls so the buffers hold their steady capacity, then the measured
/// iterations. The signal is the entry's own modulator output, so the number is what a channel
/// running this entry actually pays.
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

/// The coherent tier's throughput at three table densities and on both timing tiers. Three
/// densities because the demapper and the decision-directed detector both scan the table, so cost
/// grows with M and a single row would say nothing about the catalog's range.
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
    // The tracking tier at one density, so the two timing tiers' costs subtract directly.
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

/// The noncoherent envelope tier: no carrier loop, one magnitude and two moments per symbol, so
/// this is the catalog's cheapest linear receiver and the floor the coherent rows are read against.
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

// --- Regeneration -------------------------------------------------------------------------------

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
