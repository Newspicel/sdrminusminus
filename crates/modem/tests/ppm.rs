//! §5 measurement bundle for the M-PPM entry ( §7 phase 5): committed BER curves on
//! both detector tiers, the two §4.3 limits tables that make the tier trade a measured pair
//! rather than a claim, the level-1 E2E loopbacks, and the fractional-rate / sub-sample-phase
//! properties the ADS-B attachment stands on.
//!
//! The chains live in `ber::catalog::ppm`; the committed artifacts live in `baselines/ppm/`.
//! The matched-filter tier is held to the *exact* noncoherent orthogonal closed form — M slots
//! are M orthogonal equal-energy signals, which is the same statement the M-FSK entry's tones
//! make — and the envelope tier is committed-and-guarded, with its distance behind tier 1 gated
//! as a number.

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

// --- Always-run harness gates ----------------------------------------------------------------

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

/// The matched tier's acceptance: M-PPM through a matched filter and an argmax is noncoherent
/// orthogonal signalling, so its committed curves sit on that exact closed form — the same
/// oracle the M-FSK entry answers to, which is the two entries' shared claim made checkable.
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

/// The §5 item-2 tier comparison, as a number: summing magnitudes instead of samples costs the
/// envelope tier a measured margin at 1e-3. It is what Mode S pays for a statistic that needs
/// no phase coherence within a slot and comes free with the magnitudes a burst scan computes
/// anyway — and it is gated in *both* directions, because a tier that stopped costing anything
/// would mean the matched filter had quietly stopped integrating.
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

/// The claim that makes both phase-5 entries one entry's worth of theory: M orthogonal
/// equal-energy signals detected noncoherently perform identically whether the M signals are M
/// *tones in one interval* or M *intervals at one tone*. Measured across the two entries'
/// committed curves, which share nothing but that closed form — different engines, different
/// sample rates (48 kHz against 8 MHz), different framing, different seeds.
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

// --- Level-1 E2E ( §4.4) -----------------------------------------------------------

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

// --- The attachment's properties: fractional rates and sub-sample phases ---------------------

/// Payload symbols the fractional-rate probes carry, and the noise they carry it under.
///
/// 112 symbols is Mode S's long frame, and the length is part of the property rather than a
/// convenience: at ~1 sample per slot the fraction of a sample each boundary falls at *cycles*
/// through the frame (`frac(j·1.024)`), so some slots sit at the geometry's own worst case,
/// where a band-limited pulse is split near-evenly between two samples that neighbouring slots
/// share. A frame long enough will always contain one. That is the rate's blind spot, not the
/// receiver's — `channels::adsb` meets it the same way, with short frames and a CRC that
/// rejects the tables which lost — which is why the property below is "*some* table reads the
/// whole frame", not "the table that looked best on its first symbols did". The noise sits
/// 30 dB above one slot's energy: enough that a boundary with no margin left still shows, far
/// enough from sensitivity that the probe measures the sampling and not the SNR. Sensitivity is
/// the committed curves' job, at the 8-samples-per-slot reference geometry.
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

/// The property `channels::adsb` runs on and no curve can state: at a slot width that is not a
/// whole number of samples, and at a transmitter phase the receiver was never told, one of the
/// phase tables reads the burst. Both tiers, the rates a 1090 MHz receiver actually offers
/// (1.024 samples per slot at 2.048 Msps, 1.2 at 2.4, 1.28 at 2.56) and one comfortable rate,
/// with the Mode S CRC's job — deciding which table won — done here by comparing whole frames.
///
/// The tolerance is the *rate's*, and it is the finding this test exists to pin. From 1.2
/// samples per slot up, every rate and phase reads a 112-symbol frame exactly, at every table
/// mismatch eight tables can leave. At 1.024 it does not: eight tables bound the mismatch to a
/// sixteenth of a sample, and on a slot that wide a sixteenth is 6 % of the boundary — enough,
/// on the slots where a band-limited pulse already splits near-evenly between two samples that
/// neighbouring slots share, to leave a symbol with no margin at all. Measured at 30 dB: at
/// most one symbol in 112, against none at every wider slot. That is the weakness dump1090 has
/// at 2.0–2.048 Msps, it is why `channels::adsb` checks a CRC rather than trusting a table, and
/// it is why the committed curves are measured at 8 samples per slot instead.
#[test]
fn every_fractional_rate_and_phase_decodes_through_some_phase_table() {
    for m in [2usize, 4] {
        let symbols = probe_symbols(m);
        for &(sps, allowed) in &[(1.024, 1usize), (1.2, 0), (1.28, 0), (2.5, 0)] {
            for (index, phase) in [0.0, 0.19, 0.37, 0.5, 0.71, 0.93].into_iter().enumerate() {
                let mut wave = Vec::new();
                // One guard symbol past the payload: at a fractional slot width the burst's
                // last slot ends inside a sample the transmitter would otherwise not emit, and
                // a receiver reading a truncated slot is measuring the generator, not the rate.
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

/// Symbol errors the *best* phase table leaves — the Mode S CRC's job, which is to say which
/// table read the frame, done here by comparing whole frames rather than by scoring a prefix: at
/// ~1 sample per slot the fraction of a sample each boundary falls at walks through the frame,
/// so the table that reads the first symbols best is not always the table that reads all of them.
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

/// A burst at the *wrong* phase table must actually be wrong somewhere — otherwise the test
/// above would pass on a receiver that ignored the phase entirely, and the eight tables would
/// be eight copies of one.
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

// --- Limits tables (§4.3, M = 2 reference configuration, both tiers) -------------------------

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

/// The carrier axes' brackets, named because the tier comparison below reads them: a row that
/// reaches its bracket is a tier that does not fail on that axis at all, and saying so takes
/// the bracket's value.
const CFO_AXIS_HZ: f64 = 1_000_000.0;
const DRIFT_AXIS_HZ_S: f64 = 1e9;

fn measure_rows(link: &Link, op_db: f64, profile_grid: &[f64]) -> Vec<LimitRow> {
    vec![
        // Nothing here tracks a carrier: what a frequency offset costs is the correlation lost
        // *inside* one slot, so the axis runs to a whole cycle per slot (1 MHz at 1 Mslot/s)
        // — which the envelope tier, reading no phase at all, is expected to shrug off entirely.
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
        // The frame's alignment is one searched offset for the whole burst, so a clock error
        // walks the slot boundaries across it: half a slot of walk over 8600 samples is ~460 ppm.
        axis_row("sample clock", "ppm", 10_000.0, 5.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
            )
        }),
        // Two symbols of static delay — inside the searched lead-in, so this row is expected to
        // be bracket-bound and says so only because the bracket reaches the end of the search.
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

/// The profile row's grid per tier: it has to bracket that tier's own 1e-3 crossing, and the
/// two tiers sit a measured margin apart.
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

/// The tier trade the two tables exist to record, read off them: on the carrier axes the
/// envelope tier does not fail *at all* — both rows sit at their bracket, because a magnitude
/// has no phase for an offset or a drift to spoil — while the matched tier has a real limit
/// inside the same bracket, where the offset has turned enough phase within one slot to eat the
/// coherent sum. That is the whole reason Mode S pays the envelope tier's 2.4 dB.
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
