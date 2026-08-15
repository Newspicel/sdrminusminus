#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        Curve,
        catalog::{
            FULL_ERRORS,
            ofdm::{
                BPSK_AWGN, BPSK_GENIE_AWGN, BPSK_GENIE_GRID, BPSK_GENIE_SEED, BPSK_GRID, BPSK_SEED,
                COMB_GRID, COMB_LIMITS, COMB_SEED, DMT_AWGN, DMT_GENIE_AWGN, DMT_GENIE_GRID,
                DMT_GENIE_SEED, DMT_GRID, DMT_OVERHEAD_DB, DMT_SEED, FULL_CAP, LEAD, LIMITS,
                OVERHEAD_DB, QAM16_AWGN, QAM16_GENIE_AWGN, QAM16_GENIE_GRID, QAM16_GENIE_SEED,
                QAM16_GRID, QAM16_SEED, QAM64_AWGN, QAM64_GENIE_AWGN, QAM64_GENIE_GRID,
                QAM64_GENIE_SEED, QAM64_GRID, QAM64_SEED, QPSK_AWGN, QPSK_COMB_AWGN,
                QPSK_GENIE_AWGN, QPSK_GENIE_GRID, QPSK_GENIE_SEED, QPSK_GRID, QPSK_SEED, RATE,
                Receiver, SYMBOLS, bpsk_genie_link, bpsk_link, dmt_genie_link, dmt_link,
                link_sized, qam16_genie_link, qam16_link, qam64_genie_link, qam64_link,
                qpsk_comb_link, qpsk_genie_link, qpsk_link,
            },
        },
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{
            Cfo, ChannelSpec, ClockError, Drift, IqImbalance, Multipath, MultipathProfile,
            TimingOffset,
        },
        limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
        rng::Rng,
        sweep::{self, Link},
        theory,
    },
    constellation::tables,
    ofdm::{ChannelEstimator, OfdmDemod, OfdmMod, OfdmParams},
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

fn sensitivity(stem: &str) -> f64 {
    limits::ebn0_at_ber(&load_curve(stem), 1e-3).expect("committed curve must bracket BER 1e-3")
}

#[test]
fn every_chain_round_trips_clean_at_high_ebn0() {
    for (link, name) in [
        (bpsk_link(), "bpsk"),
        (qpsk_link(), "qpsk"),
        (qam16_link(), "16-qam"),
        (qam64_link(), "64-qam"),
        (bpsk_genie_link(), "bpsk genie"),
        (qpsk_genie_link(), "qpsk genie"),
        (qam16_genie_link(), "16-qam genie"),
        (qam64_genie_link(), "64-qam genie"),
        (qpsk_comb_link(), "qpsk comb"),
        (dmt_link(), "dmt"),
        (dmt_genie_link(), "dmt genie"),
    ] {
        let ber = limits::measure_ber(&link, &ChannelSpec::default(), 30.0, 0x0f_d0, 1, 1);
        assert!(ber < 1e-3, "{name} floor {ber} at 30 dB Eb/N0");
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
fn every_committed_curve_matches_its_baseline() {
    for (link, grid, seed, stem) in [
        (bpsk_link(), BPSK_GRID, BPSK_SEED, BPSK_AWGN),
        (qpsk_link(), QPSK_GRID, QPSK_SEED, QPSK_AWGN),
        (qam16_link(), QAM16_GRID, QAM16_SEED, QAM16_AWGN),
        (qam64_link(), QAM64_GRID, QAM64_SEED, QAM64_AWGN),
        (
            bpsk_genie_link(),
            BPSK_GENIE_GRID,
            BPSK_GENIE_SEED,
            BPSK_GENIE_AWGN,
        ),
        (
            qpsk_genie_link(),
            QPSK_GENIE_GRID,
            QPSK_GENIE_SEED,
            QPSK_GENIE_AWGN,
        ),
        (
            qam16_genie_link(),
            QAM16_GENIE_GRID,
            QAM16_GENIE_SEED,
            QAM16_GENIE_AWGN,
        ),
        (
            qam64_genie_link(),
            QAM64_GENIE_GRID,
            QAM64_GENIE_SEED,
            QAM64_GENIE_AWGN,
        ),
        (qpsk_comb_link(), COMB_GRID, COMB_SEED, QPSK_COMB_AWGN),
        (dmt_link(), DMT_GRID, DMT_SEED, DMT_AWGN),
        (
            dmt_genie_link(),
            DMT_GENIE_GRID,
            DMT_GENIE_SEED,
            DMT_GENIE_AWGN,
        ),
    ] {
        assert_curve_prefix(&link, grid, seed, stem);
    }
}

#[test]
fn every_order_sits_on_its_subcarriers_own_closed_form() {
    for (stem, grid, oracle) in oracle_rows() {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, &oracle, grid[0], *grid.last().unwrap());
        assert!(
            worst.abs() < 0.75,
            "{stem}: worst penalty {worst} dB vs its shifted closed form"
        );
    }
}

type OracleRow = (&'static str, &'static [f64], Box<dyn Fn(f64) -> f64>);

fn oracle_rows() -> [OracleRow; 5] {
    [
        (
            BPSK_GENIE_AWGN,
            BPSK_GENIE_GRID,
            Box::new(|db| theory::bpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            QPSK_GENIE_AWGN,
            QPSK_GENIE_GRID,
            Box::new(|db| theory::qpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            QAM16_GENIE_AWGN,
            QAM16_GENIE_GRID,
            Box::new(|db| theory::mqam_ber(16, db - *OVERHEAD_DB)),
        ),
        (
            QAM64_GENIE_AWGN,
            QAM64_GENIE_GRID,
            Box::new(|db| theory::mqam_ber(64, db - *OVERHEAD_DB)),
        ),
        (
            DMT_GENIE_AWGN,
            DMT_GENIE_GRID,
            Box::new(|db| theory::qpsk_ber(db - *DMT_OVERHEAD_DB)),
        ),
    ]
}

#[test]
fn acquisition_costs_the_recorded_margin_over_the_genie() {
    for (name, acquiring, genie, lo, hi) in [
        ("bpsk", BPSK_AWGN, BPSK_GENIE_AWGN, 0.3, 1.4),
        ("qpsk", QPSK_AWGN, QPSK_GENIE_AWGN, 1.4, 2.9),
        ("16-qam", QAM16_AWGN, QAM16_GENIE_AWGN, 1.4, 2.9),
        ("64-qam", QAM64_AWGN, QAM64_GENIE_AWGN, 1.4, 2.9),
        ("dmt", DMT_AWGN, DMT_GENIE_AWGN, 1.4, 3.6),
    ] {
        let cost = sensitivity(acquiring) - sensitivity(genie);
        assert!(
            (lo..hi).contains(&cost),
            "{name}: acquisition costs {cost} dB over the genie receiver"
        );
    }
}

#[test]
fn the_comb_tier_is_ahead_under_awgn_and_the_limits_tables_say_why() {
    let margin = sensitivity(QPSK_AWGN) - sensitivity(QPSK_COMB_AWGN);
    assert!(
        (0.8..2.0).contains(&margin),
        "the comb tier sits {margin} dB from the long-training tier under AWGN"
    );
    let row = |stem: &str, axis: &str| {
        limits::load_json(&baseline_path(stem))
            .unwrap()
            .rows
            .iter()
            .find(|r| r.axis == axis)
            .unwrap_or_else(|| panic!("{stem} carries no '{axis}' row"))
            .threshold
    };
    let long = row(LIMITS, "two-ray delay");
    let comb = row(COMB_LIMITS, "two-ray delay");
    assert!(
        comb < 0.5 * long,
        "the comb tier tolerates {comb} samples of echo against the long training's {long}"
    );
}

#[test]
fn dmt_costs_the_hermitian_mirror_and_nothing_else() {
    let cost = sensitivity(DMT_GENIE_AWGN) - sensitivity(QPSK_GENIE_AWGN);
    let predicted = *DMT_OVERHEAD_DB - *OVERHEAD_DB;
    assert!(
        (predicted - 3.0103).abs() < 1e-3,
        "predicted {predicted} dB"
    );
    assert!(
        (cost - predicted).abs() < 0.3,
        "DMT costs {cost} dB where the geometry predicts {predicted} dB"
    );
}

#[test]
fn the_channel_estimate_carries_half_the_noise_variance_at_every_snr() {
    let params = OfdmParams::wifi_like();
    let table = tables::qam_square(4).unwrap();
    let mut modulator = OfdmMod::new(params.clone());
    let mut state = 0x0f_e5u32;
    let points: Vec<Complex<f32>> = (0..params.data_subcarriers() * 8)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            table.points()[(state % 4) as usize]
        })
        .collect();
    let mut clean = vec![Complex::new(0.0, 0.0); LEAD];
    modulator.frame(&points, &mut clean);

    for &sigma2 in &[0.02f64, 0.05, 0.1] {
        let sigma = (sigma2 / 2.0).sqrt();
        let (mut mse, mut reported, mut worst_common) = (0.0f64, 0.0f64, 0.0f64);
        let trials = 40u32;
        for trial in 0..trials {
            let mut rng = Rng::new(0x0f_e5 + u64::from(trial));
            let mut wave = clean.clone();
            for s in &mut wave {
                *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
            }
            let mut demod = OfdmDemod::new(params.clone()).with_window_backoff(0);
            demod.acquire(&wave, 128).unwrap();
            let estimate = demod.channel();
            reported += estimate.noise_var();
            let mean: Complex<f64> = params
                .map()
                .occupied()
                .iter()
                .map(|c| {
                    let h = estimate.h(c.bin);
                    Complex::new(f64::from(h.re), f64::from(h.im))
                })
                .sum::<Complex<f64>>()
                / params.map().occupied().len() as f64;
            let common = Complex::from_polar(1.0, -mean.arg());
            worst_common = worst_common.max(mean.arg().abs());
            for c in params.map().occupied() {
                let h = estimate.h(c.bin);
                let h = Complex::new(f64::from(h.re), f64::from(h.im)) * common;
                mse += (h - Complex::new(1.0, 0.0)).norm_sqr();
            }
        }
        let n = f64::from(trials) * params.map().occupied().len() as f64;
        let mse = mse / n;
        let reported = reported / f64::from(trials);
        assert!(
            (reported / sigma2 - 1.0).abs() < 0.1,
            "σ² {sigma2}: the chain measured a noise variance of {reported}"
        );
        assert!(
            (mse / (sigma2 / 2.0) - 1.0).abs() < 0.15,
            "σ² {sigma2}: estimate MSE {mse}, closed form {}",
            sigma2 / 2.0
        );
        assert!(
            worst_common < 0.5,
            "σ² {sigma2}: worst common rotation {worst_common} rad"
        );
    }
}

fn loopback_at_margin(mut link: Link, curve_name: &str, margin_db: f64, seed: u64) {
    let sensitivity = sensitivity(curve_name);
    let payloads = Payloads::new(seed, 4, link.bits_per_trial);
    let mut channel = channel_at_margin(&ChannelSpec::default(), &link, sensitivity, margin_db);
    assert_eq!(loopback(&mut link, &mut channel, payloads), Ok(()));
}

fn short_link(name: &str, receiver: Receiver, m: u32) -> Link {
    link_sized(
        name,
        OfdmParams::wifi_like(),
        tables::qam_square(m).unwrap(),
        receiver,
        8,
    )
}

#[test]
fn qpsk_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        short_link(
            "qpsk-ofdm",
            Receiver::Acquire(ChannelEstimator::LongTraining),
            4,
        ),
        QPSK_AWGN,
        6.0,
        0x0f_e1,
    );
}

#[test]
fn qam16_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        short_link(
            "16qam-ofdm",
            Receiver::Acquire(ChannelEstimator::LongTraining),
            16,
        ),
        QAM16_AWGN,
        6.0,
        0x0f_e2,
    );
}

#[test]
fn qam64_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        short_link(
            "64qam-ofdm",
            Receiver::Acquire(ChannelEstimator::LongTraining),
            64,
        ),
        QAM64_AWGN,
        6.0,
        0x0f_e3,
    );
}

#[test]
fn dmt_loops_back_clean_at_6db_margin() {
    loopback_at_margin(
        link_sized(
            "qpsk-dmt",
            OfdmParams::dmt_like(),
            tables::qam_square(4).unwrap(),
            Receiver::Acquire(ChannelEstimator::LongTraining),
            8,
        ),
        DMT_AWGN,
        6.0,
        0x0f_e4,
    );
}

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, QPSK_SEED ^ 0x11e5, 150, 40_000)
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
const LONG_PROFILE_GRID: [f64; 6] = [9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
const COMB_PROFILE_GRID: [f64; 6] = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0];

const CFO_AXIS_HZ: f64 = 800_000.0;
const DRIFT_AXIS_HZ_S: f64 = 1e9;
const TIMING_AXIS_SAMPLES: f64 = 32.0;
const DELAY_AXIS_SAMPLES: f64 = 32.0;

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
        axis_row("sample clock", "ppm", 10_000.0, 10.0, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
            )
        }),
        axis_row(
            "static timing offset",
            "samples",
            TIMING_AXIS_SAMPLES,
            0.5,
            |d| {
                probe(
                    link,
                    &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                    op_db,
                )
            },
        ),
        axis_row("two-ray delay", "samples", DELAY_AXIS_SAMPLES, 0.5, |d| {
            probe(
                link,
                &ChannelSpec::default().multipath(Multipath::new(MultipathProfile::TwoRay {
                    delay_samples: d.round() as usize,
                    relative_db: -6.0,
                    phase_rad: 0.7,
                })),
                op_db,
            )
        }),
        axis_row("exponential PDP spread", "samples", 16.0, 0.25, |spread| {
            probe(
                link,
                &ChannelSpec::default().multipath(Multipath::new(
                    MultipathProfile::ExponentialPdp {
                        rms_delay_spread_samples: spread.max(1e-3),
                        taps: 12,
                    },
                )),
                op_db,
            )
        }),
        axis_row("IQ gain", "dB", 6.0, 0.05, |db| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(db, 0.0)),
                op_db,
            )
        }),
        axis_row("IQ phase", "deg", 30.0, 0.25, |deg| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(0.0, deg)),
                op_db,
            )
        }),
        limits::measure_profile_degradation(
            link,
            &ChannelSpec::default(),
            CompositeProfile::StaticIndoor,
            profile_grid,
            QPSK_SEED ^ 0x51de,
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

#[test]
fn long_training_tier_limits_rows_match_committed_table() {
    assert_table_matches(LIMITS, &qpsk_link(), &LONG_PROFILE_GRID);
}

#[test]
fn comb_tier_limits_rows_match_committed_table() {
    assert_table_matches(COMB_LIMITS, &qpsk_comb_link(), &COMB_PROFILE_GRID);
}

#[test]
fn the_delay_spread_limit_is_the_cyclic_prefix() {
    let table = limits::load_json(&baseline_path(LIMITS)).unwrap();
    let delay = table
        .rows
        .iter()
        .find(|r| r.axis == "two-ray delay")
        .expect("the committed table carries a two-ray row")
        .threshold;
    let cp = OfdmParams::wifi_like().cp() as f64;
    assert!(
        (0.75 * cp..1.5 * cp).contains(&delay),
        "the two-ray row reads {delay} samples against a {cp}-sample prefix"
    );
}

#[test]
fn the_frequency_selective_rows_are_fade_limited_not_delay_limited() {
    let table = limits::load_json(&baseline_path(LIMITS)).unwrap();
    let row = |axis: &str| {
        table
            .rows
            .iter()
            .find(|r| r.axis == axis)
            .unwrap_or_else(|| panic!("the committed table carries no '{axis}' row"))
            .threshold
    };
    let spread = row("exponential PDP spread");
    assert!(
        spread < 2.0,
        "the exponential-PDP row reads {spread} samples; a dense-scatter channel that mild \
         should still be fading some subcarrier into the noise"
    );
    assert!(
        spread < 0.25 * row("two-ray delay"),
        "a dispersive channel is supposed to cost this chain far more than one specular echo"
    );
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
#[ignore = "full sweep; run in release to (re)generate the committed curves"]
fn measure_all_curves_full() {
    for (name, link, grid, seed, stem) in [
        ("bpsk", bpsk_link(), BPSK_GRID, BPSK_SEED, BPSK_AWGN),
        ("qpsk", qpsk_link(), QPSK_GRID, QPSK_SEED, QPSK_AWGN),
        ("16-qam", qam16_link(), QAM16_GRID, QAM16_SEED, QAM16_AWGN),
        ("64-qam", qam64_link(), QAM64_GRID, QAM64_SEED, QAM64_AWGN),
        (
            "bpsk genie",
            bpsk_genie_link(),
            BPSK_GENIE_GRID,
            BPSK_GENIE_SEED,
            BPSK_GENIE_AWGN,
        ),
        (
            "qpsk genie",
            qpsk_genie_link(),
            QPSK_GENIE_GRID,
            QPSK_GENIE_SEED,
            QPSK_GENIE_AWGN,
        ),
        (
            "16-qam genie",
            qam16_genie_link(),
            QAM16_GENIE_GRID,
            QAM16_GENIE_SEED,
            QAM16_GENIE_AWGN,
        ),
        (
            "64-qam genie",
            qam64_genie_link(),
            QAM64_GENIE_GRID,
            QAM64_GENIE_SEED,
            QAM64_GENIE_AWGN,
        ),
        (
            "qpsk comb",
            qpsk_comb_link(),
            COMB_GRID,
            COMB_SEED,
            QPSK_COMB_AWGN,
        ),
        ("dmt", dmt_link(), DMT_GRID, DMT_SEED, DMT_AWGN),
        (
            "dmt genie",
            dmt_genie_link(),
            DMT_GENIE_GRID,
            DMT_GENIE_SEED,
            DMT_GENIE_AWGN,
        ),
    ] {
        println!("--- {name}");
        remeasure_curve(&link, grid, seed, stem);
    }
}

fn measure_table_full(
    stem: &str,
    entry: &str,
    link: &Link,
    grid: &[f64],
    seed: u64,
    profile_grid: &[f64],
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
    table.rows = measure_rows(link, op_db, profile_grid);
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
fn measure_long_training_limits_full() {
    measure_table_full(
        LIMITS,
        "ofdm-qpsk-reference",
        &qpsk_link(),
        QPSK_GRID,
        QPSK_SEED,
        &LONG_PROFILE_GRID,
    );
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_comb_limits_full() {
    measure_table_full(
        COMB_LIMITS,
        "ofdm-qpsk-comb",
        &qpsk_comb_link(),
        COMB_GRID,
        COMB_SEED,
        &COMB_PROFILE_GRID,
    );
}

#[test]
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (name, link, oracle) in [
        (
            "bpsk",
            bpsk_link(),
            Box::new(|db: f64| theory::bpsk_ber(db - *OVERHEAD_DB)) as Box<dyn Fn(f64) -> f64>,
        ),
        (
            "qpsk",
            qpsk_link(),
            Box::new(|db: f64| theory::qpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            "16-qam",
            qam16_link(),
            Box::new(|db: f64| theory::mqam_ber(16, db - *OVERHEAD_DB)),
        ),
        (
            "64-qam",
            qam64_link(),
            Box::new(|db: f64| theory::mqam_ber(64, db - *OVERHEAD_DB)),
        ),
        (
            "bpsk genie",
            bpsk_genie_link(),
            Box::new(|db: f64| theory::bpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            "qpsk genie",
            qpsk_genie_link(),
            Box::new(|db: f64| theory::qpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            "16-qam genie",
            qam16_genie_link(),
            Box::new(|db: f64| theory::mqam_ber(16, db - *OVERHEAD_DB)),
        ),
        (
            "64-qam genie",
            qam64_genie_link(),
            Box::new(|db: f64| theory::mqam_ber(64, db - *OVERHEAD_DB)),
        ),
        (
            "qpsk comb",
            qpsk_comb_link(),
            Box::new(|db: f64| theory::qpsk_ber(db - *OVERHEAD_DB)),
        ),
        (
            "dmt",
            dmt_link(),
            Box::new(|db: f64| theory::qpsk_ber(db - *DMT_OVERHEAD_DB)),
        ),
        (
            "dmt genie",
            dmt_genie_link(),
            Box::new(|db: f64| theory::qpsk_ber(db - *DMT_OVERHEAD_DB)),
        ),
    ] {
        let grid: Vec<f64> = (4..=22).map(f64::from).collect();
        let curve = sweep::sweep_ber(&link, &ChannelSpec::default(), &grid, 0x9999, 400, 300_000);
        println!("--- {name} ({SYMBOLS} data symbols)");
        for p in &curve.points {
            println!(
                "{:>5.1} dB  BER {:.3e}  (shifted theory {:.3e})",
                p.ebn0_db,
                p.rate(),
                oracle(p.ebn0_db)
            );
        }
    }
}

#[test]
#[ignore = "prints the committed numbers this entry's catalog row quotes; asserts nothing"]
fn print_catalog_numbers() {
    println!(
        "framing overhead {:.4} dB, DMT {:.4} dB",
        *OVERHEAD_DB, *DMT_OVERHEAD_DB
    );
    for (stem, grid, oracle) in oracle_rows() {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, &oracle, grid[0], *grid.last().unwrap());
        println!("{stem}: worst penalty {worst:+.3} dB");
    }
    for (name, acquiring, genie) in [
        ("bpsk", BPSK_AWGN, BPSK_GENIE_AWGN),
        ("qpsk", QPSK_AWGN, QPSK_GENIE_AWGN),
        ("16-qam", QAM16_AWGN, QAM16_GENIE_AWGN),
        ("64-qam", QAM64_AWGN, QAM64_GENIE_AWGN),
        ("dmt", DMT_AWGN, DMT_GENIE_AWGN),
    ] {
        println!(
            "{name}: 1e-3 at {:.2} dB, acquisition costs {:+.2} dB",
            sensitivity(acquiring),
            sensitivity(acquiring) - sensitivity(genie)
        );
    }
    println!(
        "comb tier {:+.2} dB ahead; DMT mirror {:+.2} dB",
        sensitivity(QPSK_AWGN) - sensitivity(QPSK_COMB_AWGN),
        sensitivity(DMT_GENIE_AWGN) - sensitivity(QPSK_GENIE_AWGN)
    );
}
