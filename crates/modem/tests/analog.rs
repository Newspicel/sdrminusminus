#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sdrmm_modem::analog::{AmDetector, AmMode};
use sdrmm_modem_test_support::ber::{
    analog::{
        AnalogLink, SinadCurve, save_json, sinad_metric, snr_at_sinad, sweep_sinad, threshold_db,
        worst_shortfall_db, worst_shortfall_db_vs_curve,
    },
    catalog::analog::{
        AM_LIMITS, AnalogMeasurement, ENTRIES, NFM_LIMITS, SSB_LIMITS, TRIALS, VOICE_GRID,
        WFM_LIMITS, WIDE_GRID, am_envelope_link, am_link_at_taps, nfm_discriminator_link,
        ssb_hilbert_link, wfm_link,
    },
    impair::{Cfo, ChannelSpec, ClockError, Drift, IqImbalance, PhaseNoise, TimingOffset},
    limits::{self, ANALOG_SINAD_DB, Criterion, LimitRow, LimitsTable, sinad_penalty_criterion},
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> SinadCurve {
    sdrmm_modem_test_support::ber::analog::load_json(&baseline_path(stem)).unwrap()
}

fn measurements() -> Vec<&'static AnalogMeasurement> {
    ENTRIES.iter().flat_map(|e| e.measurements).collect()
}

fn measure(m: &AnalogMeasurement, full: bool) -> SinadCurve {
    sweep_sinad(
        &(m.link)(),
        &ChannelSpec::default(),
        m.tier(full),
        m.seed,
        m.trials,
    )
}

const DRIFT_TOLERANCE_DB: f64 = 0.5;

#[test]
fn every_committed_curve_reproduces_its_smoke_prefix() {
    for m in measurements() {
        let measured = measure(m, false);
        let committed = load_curve(m.stem);
        let lo = measured.points.first().unwrap().snr_db;
        let hi = measured.points.last().unwrap().snr_db;
        let drift = worst_shortfall_db_vs_curve(&measured, &committed, lo, hi);
        assert!(
            drift.abs() < DRIFT_TOLERANCE_DB,
            "{}: drifted {drift} dB from its committed curve",
            m.stem
        );
    }
}

#[test]
fn every_oracle_row_sits_on_its_figure_of_merit() {
    for m in measurements() {
        let Some((name, oracle, from_db, tolerance)) = m.reference.oracle() else {
            continue;
        };
        let committed = load_curve(m.stem);
        let hi = committed.points.last().unwrap().snr_db;
        let gap = worst_shortfall_db(&committed, oracle, from_db, hi);
        assert!(
            gap.abs() < tolerance,
            "{}: {gap:+.3} dB from {name} over [{from_db}, {hi}] dB (tolerance {tolerance})",
            m.stem
        );
    }
}

#[test]
fn every_committed_curve_is_monotone_above_its_threshold() {
    for m in measurements() {
        let committed = load_curve(m.stem);
        let from = m.reference.oracle().map_or(f64::NEG_INFINITY, |o| o.2);
        let usable: Vec<_> = committed
            .points
            .iter()
            .filter(|p| p.snr_db >= from)
            .collect();
        for pair in usable.windows(2) {
            assert!(
                pair[1].sinad_db > pair[0].sinad_db,
                "{}: SINAD fell from {} dB at {} to {} dB at {}",
                m.stem,
                pair[0].sinad_db,
                pair[0].snr_db,
                pair[1].sinad_db,
                pair[1].snr_db
            );
        }
    }
}

const KNEE_DROP_DB: f64 = 1.0;

#[test]
fn the_am_tiers_agree_above_threshold_and_separate_below_it() {
    let envelope = load_curve("analog/am_envelope_sinad");
    let synchronous = load_curve("analog/am_synchronous_sinad");
    let at = |curve: &SinadCurve, snr: f64| {
        curve
            .points
            .iter()
            .find(|p| (p.snr_db - snr).abs() < 1e-9)
            .unwrap()
            .sinad_db
    };
    for snr in [21.0, 24.0, 27.0, 30.0] {
        let gap = at(&synchronous, snr) - at(&envelope, snr);
        assert!(gap.abs() < 0.5, "at {snr} dB the tiers differ by {gap} dB");
    }
    for snr in [0.0, 3.0] {
        let gap = at(&synchronous, snr) - at(&envelope, snr);
        assert!(
            gap > 0.2,
            "at {snr} dB the synchronous tier is only {gap} dB ahead"
        );
    }
}

#[test]
fn the_two_sideband_methods_measure_the_same_entry() {
    let hilbert = load_curve("analog/ssb_hilbert_sinad");
    let weaver = load_curve("analog/ssb_weaver_sinad");
    for (a, b) in hilbert.points.iter().zip(&weaver.points) {
        assert!(
            (a.sinad_db - b.sinad_db).abs() < 0.5,
            "at {} dB: phasing {} dB, Weaver {} dB",
            a.snr_db,
            a.sinad_db,
            b.sinad_db
        );
    }
}

#[test]
fn the_committed_curves_price_bandwidth_against_sensitivity() {
    let at = |stem: &str, snr: f64| {
        load_curve(stem)
            .points
            .iter()
            .find(|p| (p.snr_db - snr).abs() < 1e-9)
            .unwrap()
            .sinad_db
    };
    let ssb = at("analog/ssb_hilbert_sinad", 30.0);
    let am = at("analog/am_envelope_sinad", 30.0);
    let wfm = at("analog/wfm_discriminator_sinad", 30.0);
    assert!((ssb - 30.0).abs() < 1.0, "SSB {ssb:.2} dB at 30 dB SNR");
    assert!(
        (ssb - am - 6.15).abs() < 1.0,
        "AM {am:.2} dB is {:.2} dB behind SSB's {ssb:.2}",
        ssb - am
    );
    assert!(
        (wfm - ssb - 15.74).abs() < 1.0,
        "WFM {wfm:.2} dB is {:.2} dB ahead of SSB's {ssb:.2}",
        wfm - ssb
    );
    let ssb_low = at("analog/ssb_hilbert_sinad", 15.0);
    let wfm_low = at("analog/wfm_discriminator_sinad", 15.0);
    assert!(
        wfm_low < ssb_low,
        "below threshold WFM reads {wfm_low:.2} dB against SSB's {ssb_low:.2}"
    );
}

#[test]
fn the_fm_loop_tier_buys_sensitivity_rather_than_threshold() {
    let discriminator = load_curve("analog/nfm_discriminator_sinad");
    let pll = load_curve("analog/nfm_pll_sinad");
    let at = |curve: &SinadCurve, snr: f64| {
        curve
            .points
            .iter()
            .find(|p| (p.snr_db - snr).abs() < 1e-9)
            .unwrap()
            .sinad_db
    };
    for snr in [9.0, 15.0, 18.0, 21.0, 24.0, 27.0, 30.0] {
        let gain = at(&pll, snr) - at(&discriminator, snr);
        assert!(
            (0.0..1.0).contains(&gain),
            "at {snr} dB the loop tier is {gain} dB ahead"
        );
    }
    let oracle =
        |snr| sdrmm_modem_test_support::ber::theory::analog_sinad_db(1.041_666_666_666_666_7, snr);
    assert_eq!(
        threshold_db(&discriminator, oracle, KNEE_DROP_DB),
        threshold_db(&pll, oracle, KNEE_DROP_DB),
        "the two tiers' knees moved apart"
    );
}

#[test]
fn the_oracle_gap_is_the_receive_filters_own_transition() {
    let grid = [24.0, 27.0, 30.0];
    let gap_at = |taps: usize| {
        let link = am_link_at_taps(
            AmMode::Suppressed,
            AmDetector::Synchronous { loop_bw: 1e-3 },
            taps,
            &format!("DSB-SC, {taps} taps"),
        );
        let curve = sweep_sinad(&link, &ChannelSpec::default(), &grid, 0xf117e2, TRIALS);
        -worst_shortfall_db(&curve, |snr| snr, 24.0, 30.0)
    };
    let soft = gap_at(127);
    let sharp = gap_at(1_023);
    assert!(
        soft > sharp + 0.5,
        "a 127-tap receiver reads {soft:.2} dB over its oracle and a 1023-tap one {sharp:.2}"
    );
    assert!(
        sharp < 0.8,
        "the committed configuration reads {sharp:.2} dB over its oracle"
    );
}

#[test]
fn every_entry_recovers_its_tone_at_a_stated_margin() {
    for m in measurements() {
        let committed = load_curve(m.stem);
        let sensitivity =
            snr_at_sinad(&committed, ANALOG_SINAD_DB).expect("committed curve brackets 12 dB");
        let link = (m.link)();
        let curve = sweep_sinad(
            &link,
            &ChannelSpec::default(),
            &[sensitivity + 12.0],
            m.seed ^ 0xe2e,
            TRIALS,
        );
        let point = curve.points[0];
        assert!(
            point.sinad_db > ANALOG_SINAD_DB + 6.0,
            "{}: {} dB SINAD at 12 dB over sensitivity",
            m.stem,
            point.sinad_db
        );
        assert!(
            point.thd_percent < 5.0,
            "{}: {} % THD at 12 dB over sensitivity",
            m.stem,
            point.thd_percent
        );
    }
}

const LIMITS_TOLERANCE: f64 = 0.2;

fn axis_rows(link: &AnalogLink, op_db: f64, seed: u64, clean_sinad_db: f64) -> Vec<LimitRow> {
    let penalty = sinad_penalty_criterion(clean_sinad_db, 1.0);
    let floor = Criterion::MinSinad {
        min_sinad_db: ANALOG_SINAD_DB,
    };
    let probe = |spec: ChannelSpec| sinad_metric(link, &spec, op_db, seed, 2);
    vec![
        limits::measure_axis_row("static CFO", "cycles/sample", penalty, 0.02, 1e-7, |cfo| {
            probe(ChannelSpec::default().cfo(Cfo::from_cycles_per_sample(cfo)))
        }),
        limits::measure_axis_row(
            "frequency drift",
            "cycles/sample^2",
            penalty,
            1e-6,
            1e-12,
            |rate| probe(ChannelSpec::default().drift(Drift::from_hz_per_s(rate, 1.0))),
        ),
        limits::measure_axis_row("sample clock", "ppm", penalty, 50_000.0, 1.0, |ppm| {
            probe(ChannelSpec::default().clock(ClockError::new(ppm)))
        }),
        limits::measure_axis_row(
            "static timing offset",
            "samples",
            penalty,
            64.0,
            0.5,
            |offset| probe(ChannelSpec::default().timing_offset(TimingOffset::new(offset))),
        ),
        limits::measure_axis_row("IQ gain imbalance", "dB", penalty, 6.0, 0.01, |db| {
            probe(ChannelSpec::default().iq_imbalance(IqImbalance::new(db, 0.0)))
        }),
        limits::measure_axis_row(
            "IQ phase imbalance",
            "degrees",
            penalty,
            30.0,
            0.05,
            |deg| probe(ChannelSpec::default().iq_imbalance(IqImbalance::new(0.0, deg))),
        ),
        limits::measure_axis_row("phase noise", "degrees RMS", floor, 30.0, 0.05, |deg| {
            probe(ChannelSpec::default().phase_noise(PhaseNoise::new(deg)))
        }),
    ]
}

type Table = (
    &'static str,
    &'static str,
    fn() -> AnalogLink,
    &'static [f64],
    u64,
);

fn table_stems() -> [Table; 4] {
    [
        (
            AM_LIMITS,
            "am-envelope",
            am_envelope_link as fn() -> AnalogLink,
            VOICE_GRID,
            0xa11e,
        ),
        (
            SSB_LIMITS,
            "ssb-hilbert",
            ssb_hilbert_link as fn() -> AnalogLink,
            VOICE_GRID,
            0x55b1,
        ),
        (
            NFM_LIMITS,
            "nfm-discriminator",
            nfm_discriminator_link as fn() -> AnalogLink,
            VOICE_GRID,
            0x8f30,
        ),
        (
            WFM_LIMITS,
            "wfm-discriminator",
            wfm_link as fn() -> AnalogLink,
            WIDE_GRID,
            0x7f30,
        ),
    ]
}

fn measure_table(entry: &str, link: &AnalogLink, grid: &[f64], seed: u64) -> LimitsTable {
    let clean = sweep_sinad(link, &ChannelSpec::default(), grid, seed, TRIALS);
    let sensitivity = snr_at_sinad(&clean, ANALOG_SINAD_DB);
    let mut table = LimitsTable::analog(entry, seed, sensitivity);
    let op_db = table
        .operating_point_db()
        .expect("the committed grid must bracket 12 dB SINAD");
    let clean_sinad = -sinad_metric(link, &ChannelSpec::default(), op_db, seed ^ 0x11e5, 2);
    table
        .rows
        .extend(axis_rows(link, op_db, seed ^ 0x11e5, clean_sinad));
    table
}

#[test]
#[ignore = "full limits run; the axis searches are minutes of sweeping"]
fn every_committed_limits_table_still_holds() {
    for (stem, entry, link, grid, seed) in table_stems() {
        let committed = limits::load_json(&baseline_path(stem)).unwrap();
        let measured = measure_table(entry, &link(), grid, seed);
        if let Err(faults) = limits::compare_tables(&measured, &committed, LIMITS_TOLERANCE) {
            panic!("{stem} regressed:\n  {}", faults.join("\n  "));
        }
    }
}

fn write_curve(curve: &SinadCurve, stem: &str) {
    let path = baseline_path(stem);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    save_json(curve, &path).unwrap();
}

#[test]
#[ignore = "full sweep; run to (re)generate the committed SINAD curves"]
fn measure_all_curves_full() {
    for m in measurements() {
        let curve = measure(m, true);
        for p in &curve.points {
            println!(
                "{:>28} {:>6.1} dB -> SINAD {:>7.2} dB, THD {:>6.2} %",
                m.stem, p.snr_db, p.sinad_db, p.thd_percent
            );
        }
        write_curve(&curve, m.stem);
    }
}

#[test]
#[ignore = "full limits run; run to (re)generate the committed tables"]
fn measure_all_limits_full() {
    for (stem, entry, link, grid, seed) in table_stems() {
        let table = measure_table(entry, &link(), grid, seed);
        println!("--- {entry}: {table:#?}");
        let path = baseline_path(stem);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(&table, &path).unwrap();
    }
}

#[test]
#[ignore = "prints the committed numbers this phase's catalog rows quote; asserts nothing"]
fn print_catalog_numbers() {
    for m in measurements() {
        let committed = load_curve(m.stem);
        let sensitivity = snr_at_sinad(&committed, ANALOG_SINAD_DB);
        let twenty = snr_at_sinad(&committed, 20.0);
        let gap = m.reference.oracle().map(|(name, oracle, from, _)| {
            let hi = committed.points.last().unwrap().snr_db;
            (
                name,
                worst_shortfall_db(&committed, &oracle, from, hi),
                threshold_db(&committed, &oracle, KNEE_DROP_DB),
            )
        });
        println!(
            "{:<32} 12 dB SINAD at {:>7} dB, 20 dB at {:>7} dB, oracle {:?}",
            m.stem,
            sensitivity.map_or("—".to_string(), |v| format!("{v:.2}")),
            twenty.map_or("—".to_string(), |v| format!("{v:.2}")),
            gap
        );
    }
}
