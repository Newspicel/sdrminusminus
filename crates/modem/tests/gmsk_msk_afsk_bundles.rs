#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sdrmm_dsp::Nco;
use sdrmm_modem::cpm::{CpmDemod, MlseDetector, TIMING_BW_BURST};
use sdrmm_modem_test_support::ber::{
    Curve,
    catalog::{
        self, DRIFT_TOLERANCE_DB, FULL_ERRORS, afsk, framing,
        framing::{FULL_CAP, RATE},
        gmsk, msk,
    },
    e2e::{Payloads, channel_at_margin, loopback},
    impair::{Awgn, Cfo, ChannelSpec, ClockError, Drift, Impairment, TimingOffset},
    limits::{self, Criterion, LimitRow, LimitsTable},
    perf::{self, PerfBaseline},
    rng::Rng,
    sweep::{self, Link},
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

const PROBE_ERRORS: u64 = 100;
const PROBE_CAP: u64 = 30_000;

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

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64, seed: u64) -> f64 {
    limits::measure_ber(link, spec, op_db, seed, PROBE_ERRORS, PROBE_CAP)
}

fn steady_axis_rows(link: &Link, rate: f64, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 3_000.0, 25.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, rate)),
                op_db,
                seed,
            )
        }),
        axis_row("frequency drift", "Hz/s", 20_000.0, 200.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, rate)),
                op_db,
                seed,
            )
        }),
        axis_row("sample clock", "ppm", 50_000.0, 500.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        axis_row("static timing offset", "samples", 10.0, 0.25, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
                seed,
            )
        }),
    ]
}

fn gmsk_burst_axis_rows(op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("dead time", "symbols", 1_024.0, 16.0, |off| {
            let mut recipe = gmsk::BurstRecipe::reference(gmsk::BURST_FRAMES);
            recipe.off_symbols = (off.round() as usize).max(16);
            let link = recipe.link("dead-time probe");
            probe(&link, &recipe.channel(), op_db, seed)
        }),
        axis_row(
            "burst shortening",
            "payload symbols removed (of 128)",
            112.0,
            2.0,
            |removed| {
                let mut recipe = gmsk::BurstRecipe::reference(gmsk::BURST_FRAMES);
                recipe.payload_symbols = 128 - (removed.round() as usize).min(112);
                let link = recipe.link("burst-length probe");
                probe(&link, &recipe.channel(), op_db, seed)
            },
        ),
        axis_row(
            "level step",
            "dB attenuation of alternate bursts",
            12.0,
            0.25,
            |db| {
                let mut recipe = gmsk::BurstRecipe::reference(gmsk::BURST_FRAMES);
                recipe.level_step_db = -db;
                let link = recipe.link("level-step probe");
                probe(&link, &recipe.channel(), op_db, seed)
            },
        ),
    ]
}

fn operating_point(curve: &Curve) -> f64 {
    limits::ebn0_at_ber(curve, 1e-3).expect("the grid must bracket BER 1e-3") + 3.0
}

#[test]
fn every_chain_round_trips_near_clean_at_high_ebn0() {
    for (link, template, name) in [
        (gmsk::link(0.3), ChannelSpec::default(), "gmsk bt=0.3"),
        (gmsk::link(0.5), ChannelSpec::default(), "gmsk bt=0.5"),
        (
            gmsk::mlse_link(0.3),
            ChannelSpec::default(),
            "gmsk bt=0.3 mlse",
        ),
        (
            gmsk::mlse_link(0.5),
            ChannelSpec::default(),
            "gmsk bt=0.5 mlse",
        ),
        (msk::link(), ChannelSpec::default(), "msk"),
        (afsk::filterbank_link(), ChannelSpec::default(), "afsk fb"),
        (
            afsk::discriminator_link(),
            ChannelSpec::default(),
            "afsk disc",
        ),
        (
            gmsk::BurstRecipe::reference(gmsk::BURST_FRAMES).link("gmsk burst"),
            gmsk::BurstRecipe::reference(gmsk::BURST_FRAMES).channel(),
            "gmsk burst",
        ),
    ] {
        let ber = limits::measure_ber(&link, &template, 30.0, 0x0c1e, 1, 1);
        assert!(ber < 3e-3, "{name} floor {ber} at 30 dB Eb/N0");
    }
}

fn smoke(stem: &str) {
    let m = catalog::measurement(stem).expect("every guarded artifact must be registered");
    let tier = m.tier(false);
    let measured = sweep::sweep_ber(
        &(m.link)(),
        &ChannelSpec::default(),
        tier.grid,
        tier.seed,
        tier.min_errors,
        tier.max_trial_bits,
    );
    let committed = sweep::load_json(&baseline_path(stem)).unwrap();
    let drift = m.drift_db(&measured, &committed).unwrap();
    assert!(
        drift.abs() < DRIFT_TOLERANCE_DB,
        "{stem} drift vs committed: {drift} dB"
    );
}

#[test]
fn gmsk_curves_match_committed_baselines() {
    smoke(gmsk::BT03_AWGN);
    smoke(gmsk::BT05_AWGN);
}

#[test]
fn gmsk_mlse_curves_match_committed_baselines() {
    smoke(gmsk::BT03_MLSE_AWGN);
    smoke(gmsk::BT05_MLSE_AWGN);
}

#[test]
fn msk_curve_matches_committed_baseline() {
    smoke(msk::AWGN);
}

#[test]
fn afsk_curves_match_committed_baselines() {
    smoke(afsk::FILTERBANK_AWGN);
    smoke(afsk::DISCRIMINATOR_AWGN);
}

#[test]
fn gmsk_alternating_curves_still_reproduce_the_historical_baselines() {
    for (bt, grid, seed, stem) in [
        (
            0.3,
            gmsk::BT03_GRID,
            gmsk::BT03_SEED,
            gmsk::BT03_AWGN_ALTERNATING,
        ),
        (
            0.5,
            gmsk::BT05_GRID,
            gmsk::BT05_SEED,
            gmsk::BT05_AWGN_ALTERNATING,
        ),
    ] {
        let measured = sweep::sweep_ber(
            &gmsk::alternating_link(bt),
            &ChannelSpec::default(),
            &grid[..3],
            seed,
            FULL_ERRORS,
            FULL_CAP,
        );
        let committed = sweep::load_json(&baseline_path(stem)).unwrap();
        let drift = sweep::worst_penalty_db_vs_curve(&measured, &committed, grid[0], grid[2]);
        assert!(
            drift.abs() < DRIFT_TOLERANCE_DB,
            "{stem} drift vs the historical baseline: {drift} dB"
        );
    }
}

#[test]
fn gmsk_bt05_sits_near_msk_at_1e3() {
    let gmsk = sweep::load_json(&baseline_path(gmsk::BT05_AWGN)).unwrap();
    let msk = sweep::load_json(&baseline_path(msk::AWGN)).unwrap();
    let penalty = sweep::penalty_db_vs_curve(&gmsk, &msk, 1e-3);
    println!("GMSK BT=0.5 vs MSK at BER 1e-3: {penalty:+.3} dB");
    assert!(
        (0.0..1.6).contains(&penalty),
        "GMSK BT=0.5 is {penalty} dB from MSK at 1e-3 (committed: +1.37 dB)"
    );
}

#[test]
fn the_mlse_tier_beats_the_discriminator_tier_it_merges_against() {
    for (bt, mlse_curve, disc_curve, floor) in [
        (0.3, gmsk::BT03_MLSE_AWGN, gmsk::BT03_AWGN, 7.5),
        (0.5, gmsk::BT05_MLSE_AWGN, gmsk::BT05_AWGN, 1.6),
    ] {
        let mlse = sweep::load_json(&baseline_path(mlse_curve)).unwrap();
        let disc = sweep::load_json(&baseline_path(disc_curve)).unwrap();
        let gain = -sweep::penalty_db_vs_curve(&mlse, &disc, 1e-3);
        println!("GMSK BT={bt}: MLSE gains {gain:+.3} dB over the discriminator at BER 1e-3");
        assert!(
            gain > floor,
            "BT={bt}: the MLSE tier gains only {gain} dB over the discriminator tier; a tier \
             that does not beat the one it merges against does not merge (§5 item 2)"
        );
    }
}

#[test]
fn the_acquisition_framing_moved_no_sensitivity() {
    for (bt, current, historical) in [
        (0.3, gmsk::BT03_AWGN, gmsk::BT03_AWGN_ALTERNATING),
        (0.5, gmsk::BT05_AWGN, gmsk::BT05_AWGN_ALTERNATING),
    ] {
        let now = sweep::load_json(&baseline_path(current)).unwrap();
        let then = sweep::load_json(&baseline_path(historical)).unwrap();
        let shift = sweep::penalty_db_vs_curve(&now, &then, 1e-3);
        println!("GMSK BT={bt}: data-like vs alternating framing at BER 1e-3: {shift:+.3} dB");
        assert!(
            shift.abs() < CROSSING_NOISE_DB,
            "BT={bt}: the framing moved the 1e-3 crossing by {shift} dB — the two generations \
             are supposed to differ in acquisition only"
        );
    }
}

const CROSSING_NOISE_DB: f64 = 0.25;

#[test]
fn gmsk_committed_curves_fall_monotonically() {
    for stem in [
        gmsk::BT03_AWGN,
        gmsk::BT05_AWGN,
        gmsk::BT03_MLSE_AWGN,
        gmsk::BT05_MLSE_AWGN,
    ] {
        let curve = sweep::load_json(&baseline_path(stem)).unwrap();
        let waterfall: Vec<_> = curve.points.iter().filter(|p| p.rate() >= 1e-4).collect();
        for pair in waterfall.windows(2) {
            assert!(
                pair[1].rate() < pair[0].rate(),
                "{stem}: BER rises from {:.3e} at {} dB to {:.3e} at {} dB",
                pair[0].rate(),
                pair[0].ebn0_db,
                pair[1].rate(),
                pair[1].ebn0_db
            );
        }
    }
}

#[test]
fn afsk_filterbank_is_the_tier_one_reference() {
    let fb = sweep::load_json(&baseline_path(afsk::FILTERBANK_AWGN)).unwrap();
    let disc = sweep::load_json(&baseline_path(afsk::DISCRIMINATOR_AWGN)).unwrap();
    let penalty = sweep::penalty_db_vs_curve(&fb, &disc, 1e-3);
    println!("AFSK filterbank vs discriminator at BER 1e-3: {penalty:+.3} dB");
    assert!(
        penalty < 0.25,
        "the filterbank fell {penalty} dB behind the discriminator; the tier-1 choice no \
         longer holds"
    );
}

fn compare_rows(measured: &[LimitRow], committed: &LimitsTable, name: &str) {
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
        let worse_by = row.threshold - m.threshold;
        if m.threshold.is_nan() || worse_by > 0.2 * row.threshold.abs() {
            faults.push(format!(
                "row '{}': committed {} -> measured {} {}",
                row.axis, row.threshold, m.threshold, m.unit
            ));
        }
    }
    assert!(faults.is_empty(), "{name} limits regressions: {faults:#?}");
}

#[test]
fn gmsk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path(gmsk::LIMITS)).unwrap();
    let curve = sweep::load_json(&baseline_path(gmsk::BT05_AWGN)).unwrap();
    let op_db = operating_point(&curve);
    let link = gmsk::link(0.5);
    let mut measured = steady_axis_rows(&link, RATE, op_db, gmsk::BT05_SEED ^ 0xbe5);
    measured.extend(gmsk_burst_axis_rows(op_db, gmsk::BT05_SEED ^ 0xbe5));
    compare_rows(&measured, &committed, "gmsk");
}

#[test]
fn gmsk_alternating_steady_limits_still_reproduce_the_historical_table() {
    let committed = limits::load_json(&baseline_path(gmsk::LIMITS_ALTERNATING)).unwrap();
    let curve = sweep::load_json(&baseline_path(gmsk::BT05_AWGN_ALTERNATING)).unwrap();
    let op_db = operating_point(&curve);
    let link = gmsk::alternating_link(0.5);
    let measured = steady_axis_rows(&link, RATE, op_db, gmsk::BT05_SEED ^ 0xbe5);
    let steady: Vec<LimitRow> = committed
        .rows
        .iter()
        .filter(|row| measured.iter().any(|m| m.axis == row.axis))
        .cloned()
        .collect();
    compare_rows(
        &measured,
        &LimitsTable {
            rows: steady,
            ..committed
        },
        "gmsk alternating (historical)",
    );
}

#[test]
fn gmsk_mlse_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path(gmsk::MLSE_LIMITS)).unwrap();
    let curve = sweep::load_json(&baseline_path(gmsk::BT03_MLSE_AWGN)).unwrap();
    let op_db = operating_point(&curve);
    let link = gmsk::mlse_link(0.3);
    let measured = steady_axis_rows(&link, RATE, op_db, gmsk::BT03_MLSE_SEED ^ 0xbe5);
    compare_rows(&measured, &committed, "gmsk mlse");
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_gmsk_mlse_limits_full() {
    let link = gmsk::mlse_link(0.3);
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        gmsk::BT03_MLSE_GRID,
        gmsk::BT03_MLSE_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("gmsk-bt03-mlse", gmsk::BT03_MLSE_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&link, RATE, op_db, gmsk::BT03_MLSE_SEED ^ 0xbe5);
    write_or_check_limits(&table, gmsk::MLSE_LIMITS);
}

#[test]
fn msk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path(msk::LIMITS)).unwrap();
    let curve = sweep::load_json(&baseline_path(msk::AWGN)).unwrap();
    let op_db = operating_point(&curve);
    let link = msk::link();
    let measured = steady_axis_rows(&link, RATE, op_db, msk::SEED ^ 0xbe5);
    compare_rows(&measured, &committed, "msk");
}

#[test]
fn afsk_limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path(afsk::LIMITS)).unwrap();
    let curve = sweep::load_json(&baseline_path(afsk::FILTERBANK_AWGN)).unwrap();
    let op_db = operating_point(&curve);
    let link = afsk::filterbank_link();
    let measured = afsk_axis_rows(&link, op_db, afsk::FILTERBANK_SEED ^ 0xbe5);
    compare_rows(&measured, &committed, "afsk");
}

fn afsk_axis_rows(link: &Link, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", 500.0, 5.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, afsk::RATE)),
                op_db,
                seed,
            )
        }),
        axis_row("frequency drift", "Hz/s", 2_000.0, 25.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, afsk::RATE)),
                op_db,
                seed,
            )
        }),
        axis_row("sample clock", "ppm", 50_000.0, 500.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        axis_row("static timing offset", "samples", 10.0, 0.25, |d| {
            probe(
                link,
                &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                op_db,
                seed,
            )
        }),
    ]
}

const E2E_MARGIN_DB: f64 = 6.0;
const E2E_PAYLOADS: usize = 6;

fn e2e(mut link: Link, curve_name: &str, seed: u64) {
    let committed = sweep::load_json(&baseline_path(curve_name)).unwrap();
    let sensitivity = limits::ebn0_at_ber(&committed, 1e-3).unwrap();
    let payloads = Payloads::new(seed, E2E_PAYLOADS, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, E2E_MARGIN_DB);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn gmsk_loops_back_clean_at_margin() {
    e2e(gmsk::link(0.5), gmsk::BT05_AWGN, 0x0e2e_63a5);
    e2e(gmsk::link(0.3), gmsk::BT03_AWGN, 0x0e2e_63a3);
}

const MLSE_BT03_E2E_MARGIN_DB: f64 = 12.0;

#[test]
fn gmsk_mlse_loops_back_clean_at_margin() {
    e2e(gmsk::mlse_link(0.5), gmsk::BT05_MLSE_AWGN, 0x0e2e_63a5_11e5);
    let mut link = gmsk::mlse_link(0.3);
    let committed = sweep::load_json(&baseline_path(gmsk::BT03_MLSE_AWGN)).unwrap();
    let sensitivity = limits::ebn0_at_ber(&committed, 1e-3).unwrap();
    let payloads = Payloads::new(0x0e2e_63a3_11e5, E2E_PAYLOADS, link.bits_per_trial);
    let mut channel = channel_at_margin(
        &ChannelSpec::default(),
        &link,
        sensitivity,
        MLSE_BT03_E2E_MARGIN_DB,
    );
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

#[test]
fn msk_loops_back_clean_at_margin() {
    e2e(msk::link(), msk::AWGN, 0x0e2e_635b);
}

#[test]
fn afsk_loops_back_clean_at_margin_through_both_detectors() {
    e2e(afsk::filterbank_link(), afsk::FILTERBANK_AWGN, 0x0e2e_afb1);
    e2e(
        afsk::discriminator_link(),
        afsk::DISCRIMINATOR_AWGN,
        0x0e2e_afd1,
    );
}

fn bench_bits(len: usize, seed: u32) -> Vec<u8> {
    perf::test_dibits(len, seed)
        .into_iter()
        .map(|d| d & 1)
        .collect()
}

fn measured_gmsk_perf() -> Vec<PerfBaseline> {
    let params = gmsk::params(0.5);
    let iq = framing::cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut demod = CpmDemod::new(&params, &gmsk::rx(0.5), TIMING_BW_BURST);
    let mut soft = Vec::with_capacity(iq.len());
    demod.process(&iq, &mut soft);
    soft.clear();
    demod.process(&iq, &mut soft);
    let msps = perf::measure_throughput(300, iq.len() as u64, || {
        soft.clear();
        demod.process(&iq, &mut soft);
    });
    vec![PerfBaseline {
        bench: "gmsk_bt05_demod".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / RATE,
        config: "GMSK BT=0.5 h=0.5, 10 sps, pulse-matched rx, timing bw 0.015".into(),
        host: perf::host_id(),
    }]
}

fn measured_gmsk_mlse_perf() -> Vec<PerfBaseline> {
    let mut out = Vec::new();
    for bt in [0.3, 0.5] {
        let params = gmsk::params(bt);
        let rx = gmsk::mlse_rx(bt);
        let iq = framing::cpm_wave(&params, &bench_bits(2_400, 0x5eed));
        let mut demod = CpmDemod::new(&params, &rx, TIMING_BW_BURST);
        let mut detector = MlseDetector::new(&params, &rx);
        let (mut soft, mut decided, mut bits) = (
            Vec::with_capacity(iq.len()),
            Vec::with_capacity(iq.len()),
            Vec::with_capacity(iq.len()),
        );
        let mut run = |demod: &mut CpmDemod, detector: &mut MlseDetector| {
            soft.clear();
            decided.clear();
            bits.clear();
            demod.process(&iq, &mut soft);
            detector.process(&soft, &mut decided, &mut bits);
        };
        run(&mut demod, &mut detector);
        run(&mut demod, &mut detector);
        let msps =
            perf::measure_throughput(300, iq.len() as u64, || run(&mut demod, &mut detector));
        out.push(PerfBaseline {
            bench: format!("gmsk_bt{:02}_mlse", (bt * 10.0) as u32),
            msamples_per_s: msps,
            realtime_factor: msps * 1e6 / RATE,
            config: format!(
                "GMSK BT={bt} h=0.5, 10 sps, pulse-matched rx, CpmDemod + MlseDetector \
                 ({} trellis states)",
                MlseDetector::new(&params, &rx).states()
            ),
            host: perf::host_id(),
        });
    }
    out
}

fn measured_msk_perf() -> Vec<PerfBaseline> {
    let params = msk::params();
    let iq = framing::cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut demod = CpmDemod::new(&params, &msk::rx(), TIMING_BW_BURST);
    let mut soft = Vec::with_capacity(iq.len());
    demod.process(&iq, &mut soft);
    soft.clear();
    demod.process(&iq, &mut soft);
    let msps = perf::measure_throughput(300, iq.len() as u64, || {
        soft.clear();
        demod.process(&iq, &mut soft);
    });
    vec![PerfBaseline {
        bench: "msk_demod".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / RATE,
        config: "MSK (1REC h=0.5), 10 sps, integrate-and-dump rx, timing bw 0.015".into(),
        host: perf::host_id(),
    }]
}

fn measured_afsk_perf() -> Vec<PerfBaseline> {
    let params = afsk::params();
    let baseband = framing::cpm_wave(&params, &bench_bits(2_400, 0x5eed));
    let mut carrier = Nco::new(afsk::CENTRE_HZ as f32, afsk::RATE as f32);
    let audio: Vec<f32> = baseband
        .iter()
        .map(|&s| (s * carrier.next_sample()).re)
        .collect();
    let detector = afsk::filterbank();
    let mut demod = CpmDemod::real(
        &params,
        &afsk::rx(detector),
        TIMING_BW_BURST,
        afsk::RATE,
        detector,
    );
    let mut soft = Vec::with_capacity(audio.len());
    demod.process_real(&audio, &mut soft);
    soft.clear();
    demod.process_real(&audio, &mut soft);
    let msps = perf::measure_throughput(300, audio.len() as u64, || {
        soft.clear();
        demod.process_real(&audio, &mut soft);
    });
    vec![PerfBaseline {
        bench: "afsk_filterbank_12k".into(),
        msamples_per_s: msps,
        realtime_factor: msps * 1e6 / afsk::RATE,
        config: "AFSK 1200/2200 Hz tone filterbank, 12 kHz, 10 sps, half-symbol rx rect".into(),
        host: perf::host_id(),
    }]
}

fn write_perf(name: &str, measured: &[PerfBaseline]) {
    if cfg!(debug_assertions) {
        panic!("a debug-profile number must never become the committed baseline");
    }
    let path = baseline_path(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    perf::save_baselines(&path, measured).unwrap();
}

fn compare_perf(name: &str, measured: &[PerfBaseline]) {
    let committed = perf::load_baselines(&baseline_path(name)).unwrap();
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
            "{name} throughput regressions past {:.0}%: {regressions:#?}",
            100.0 * perf::REGRESSION_FRACTION
        ),
    }
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_gmsk_perf_baseline() {
    write_perf(gmsk::PERF, &measured_gmsk_perf());
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_gmsk_mlse_perf_baseline() {
    write_perf(gmsk::MLSE_PERF, &measured_gmsk_mlse_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_gmsk_mlse_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf(gmsk::MLSE_PERF, &measured_gmsk_mlse_perf());
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_msk_perf_baseline() {
    write_perf(msk::PERF, &measured_msk_perf());
}

#[test]
#[ignore = "rewrites the committed baseline; run explicitly in release on the reference host"]
fn write_afsk_perf_baseline() {
    write_perf(afsk::PERF, &measured_afsk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_gmsk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf(gmsk::PERF, &measured_gmsk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_msk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf(msk::PERF, &measured_msk_perf());
}

#[test]
#[ignore = "nightly perf gate; run alone in release (wall-clock: parallel sweeps starve it)"]
fn compare_afsk_perf_baseline() {
    if cfg!(debug_assertions) {
        eprintln!("skipping the perf gate: throughput is only comparable in release");
        return;
    }
    compare_perf(afsk::PERF, &measured_afsk_perf());
}

fn remeasure_curve(link: &Link, template: &ChannelSpec, grid: &[f64], seed: u64, name: &str) {
    let curve = sweep::sweep_ber(link, template, grid, seed, FULL_ERRORS, FULL_CAP);
    for p in &curve.points {
        println!(
            "{:>5.1} dB  {:>7} / {:<9} BER {:.3e}",
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
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_gmsk_curves_full() {
    remeasure_curve(
        &gmsk::link(0.3),
        &ChannelSpec::default(),
        gmsk::BT03_GRID,
        gmsk::BT03_SEED,
        gmsk::BT03_AWGN,
    );
    remeasure_curve(
        &gmsk::link(0.5),
        &ChannelSpec::default(),
        gmsk::BT05_GRID,
        gmsk::BT05_SEED,
        gmsk::BT05_AWGN,
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_gmsk_mlse_curves_full() {
    remeasure_curve(
        &gmsk::mlse_link(0.3),
        &ChannelSpec::default(),
        gmsk::BT03_MLSE_GRID,
        gmsk::BT03_MLSE_SEED,
        gmsk::BT03_MLSE_AWGN,
    );
    remeasure_curve(
        &gmsk::mlse_link(0.5),
        &ChannelSpec::default(),
        gmsk::BT05_MLSE_GRID,
        gmsk::BT05_MLSE_SEED,
        gmsk::BT05_MLSE_AWGN,
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curve"]
fn measure_msk_curve_full() {
    remeasure_curve(
        &msk::link(),
        &ChannelSpec::default(),
        msk::GRID,
        msk::SEED,
        msk::AWGN,
    );
}

#[test]
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_afsk_curves_full() {
    remeasure_curve(
        &afsk::filterbank_link(),
        &ChannelSpec::default(),
        afsk::GRID,
        afsk::FILTERBANK_SEED,
        afsk::FILTERBANK_AWGN,
    );
    remeasure_curve(
        &afsk::discriminator_link(),
        &ChannelSpec::default(),
        afsk::GRID,
        afsk::DISCRIMINATOR_SEED,
        afsk::DISCRIMINATOR_AWGN,
    );
}

fn write_or_check_limits(table: &LimitsTable, name: &str) {
    println!(
        "{name}: sensitivity 1e-2 {:?}  1e-3 {:?}  1e-4 {:?}",
        table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
    );
    for row in &table.rows {
        println!("{:<24} {:>12.4} {}", row.axis, row.threshold, row.unit);
    }
    let path = baseline_path(name);
    if path.exists() {
        let committed = limits::load_json(&path).unwrap();
        if let Err(faults) = limits::compare_tables(table, &committed, 0.2) {
            panic!("{name} limits regressions: {faults:#?}");
        }
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(table, &path).unwrap();
        println!("baseline created at {}", path.display());
    }
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_gmsk_limits_full() {
    let link = gmsk::link(0.5);
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        gmsk::BT05_GRID,
        gmsk::BT05_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("gmsk-bt05-discriminator", gmsk::BT05_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&link, RATE, op_db, gmsk::BT05_SEED ^ 0xbe5);
    table
        .rows
        .extend(gmsk_burst_axis_rows(op_db, gmsk::BT05_SEED ^ 0xbe5));
    write_or_check_limits(&table, gmsk::LIMITS);
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_msk_limits_full() {
    let link = msk::link();
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        msk::GRID,
        msk::SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("msk-discriminator", msk::SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&link, RATE, op_db, msk::SEED ^ 0xbe5);
    write_or_check_limits(&table, msk::LIMITS);
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_afsk_limits_full() {
    let link = afsk::filterbank_link();
    let sensitivity = limits::measure_sensitivity(
        &link,
        &ChannelSpec::default(),
        afsk::GRID,
        afsk::FILTERBANK_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("afsk-tone-filterbank", afsk::FILTERBANK_SEED, &sensitivity);
    let op_db = operating_point(&sensitivity.curve);
    table.rows = afsk_axis_rows(&link, op_db, afsk::FILTERBANK_SEED ^ 0xbe5);
    write_or_check_limits(&table, afsk::LIMITS);
}

#[test]
#[ignore = "diagnostic behind the committed tail; prints error positions, asserts nothing"]
fn probe_mlse_error_positions() {
    for ebn0_db in [26.0, 40.0] {
        let link = gmsk::mlse_link(0.3);
        let mut rng = Rng::new(0x5eed);
        let channel = ChannelSpec::default()
            .awgn(Awgn::for_ebn0(ebn0_db, link.bits_per_trial as u64))
            .build();
        let mut positions: Vec<usize> = Vec::new();
        for _ in 0..400 {
            let payload: Vec<bool> = (0..link.bits_per_trial)
                .map(|_| rng.uniform() > 0.5)
                .collect();
            let mut wave = (link.modulate)(&payload);
            channel.apply(&mut wave, &mut rng);
            let decoded = (link.demodulate)(&wave);
            for (i, &sent) in payload.iter().enumerate() {
                if decoded.get(i) != Some(&sent) {
                    positions.push(i);
                }
            }
        }
        println!(
            "{ebn0_db} dB: {} errors in 400 x {} bits; positions {positions:?}",
            positions.len(),
            link.bits_per_trial,
        );
    }
}

#[test]
#[ignore = "prints coarse curves to choose the MLSE grids; asserts nothing"]
fn probe_mlse_grids() {
    for (link, name) in [
        (gmsk::mlse_link(0.3), "gmsk bt=0.3 MLSE"),
        (gmsk::mlse_link(0.5), "gmsk bt=0.5 MLSE"),
        (gmsk::link(0.3), "gmsk bt=0.3 discriminator"),
        (gmsk::link(0.5), "gmsk bt=0.5 discriminator"),
    ] {
        let grid: Vec<f64> = (4..=13).map(|d| f64::from(d) * 2.0).collect();
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

#[test]
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (link, name) in [
        (gmsk::link(0.3), "gmsk bt=0.3"),
        (gmsk::link(0.5), "gmsk bt=0.5"),
        (msk::link(), "msk"),
        (afsk::filterbank_link(), "afsk filterbank"),
        (afsk::discriminator_link(), "afsk discriminator"),
    ] {
        let grid: Vec<f64> = (3..=15).map(|d| f64::from(d) * 2.0).collect();
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
