//! §5 measurement bundle for the four multicarrier entries (MODEM-PLAN §7 phase 9): committed BER
//! curves for GFDM on both receivers, UFMC, FBMC/OQAM and OTFS; the §4.3 limits tables; and the
//! level-1 E2E loopbacks.
//!
//! The chains live in `ber::catalog::multicarrier`; the committed artifacts live in
//! `baselines/multicarrier/`.
//!
//! **What the phase is about.** Three of the four waveforms are orthogonal maps from points to
//! samples, so under thermal noise alone they can be neither better nor worse than the
//! constellation they carry — every dB their curves sit from Gray QPSK's exact closed form is
//! their own framing overhead, and every overhead here is arithmetic rather than a fitted
//! constant. GFDM is the exception and the interesting one: its pulses overlap by construction, so
//! it has no unitary reading at all, and its two receivers are the two ways of living with that.
//! What the other three are *for* shows up on axes AWGN cannot see, and the entries measure it
//! there — UFMC's out-of-band suppression, FBMC's prefix-free spectrum, and OTFS's diversity
//! against the nulled subcarrier phase 6 recorded as an uncoded chain's whole loss.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_modem::{
    ber::{
        Curve,
        catalog::{
            self, DRIFT_TOLERANCE_DB, FULL_ERRORS, Measurement,
            multicarrier::{
                CAP, GFDM_LIMITS, GFDM_OVERHEAD_DB, GFDM_ZF_SEED, OTFS_LIMITS, SYMBOLS, fbmc_link,
                gfdm_amplification_db, gfdm_zf_link, ofdm_reference_link, otfs_link, ufmc_link,
            },
            ofdm::{LEAD, RATE},
        },
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{Cfo, ChannelSpec, ClockError, Drift, IqImbalance, TimingOffset},
        limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable, penalty_criterion},
        sweep::{self, Link, sweep_ber},
    },
    ofdm::OfdmParams,
};

fn baseline_path(stem: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/{stem}.json"))
}

fn load_curve(stem: &str) -> Curve {
    sweep::load_json(&baseline_path(stem)).unwrap()
}

fn measurements() -> Vec<&'static Measurement> {
    ["gfdm", "ufmc", "fbmc", "otfs"]
        .into_iter()
        .flat_map(|name| catalog::find(name).unwrap().measurements)
        .collect()
}

fn measure(m: &Measurement, full: bool) -> Curve {
    let tier = m.tier(full);
    sweep_ber(
        &(m.link)(),
        &ChannelSpec::default(),
        tier.grid,
        tier.seed,
        tier.min_errors,
        tier.max_trial_bits,
    )
}

fn sensitivity(stem: &str) -> f64 {
    limits::ebn0_at_ber(&load_curve(stem), 1e-3).expect("committed curve must bracket BER 1e-3")
}

/// The smoke gate every `cargo test` runs: the front of each committed grid, reproduced.
#[test]
fn every_committed_curve_reproduces_its_smoke_prefix() {
    for m in measurements() {
        let measured = measure(m, false);
        let committed = load_curve(m.stem);
        let drift = m.drift_db(&measured, &committed).expect("a usable point");
        assert!(
            drift.abs() < DRIFT_TOLERANCE_DB,
            "{}: drifted {drift} dB from its committed curve",
            m.stem
        );
    }
}

/// The §4.1 acceptance: every oracle row sits on its constellation's own closed form, shifted by
/// an overhead that is a closed form of the geometry. This is the phase's headline — three of the
/// four waveforms are transparent, and the measurement says so.
#[test]
fn every_oracle_row_sits_on_its_constellation() {
    for m in measurements() {
        let committed = load_curve(m.stem);
        let Some((reference, gap, tolerance)) = m.reference_gap(&committed) else {
            continue;
        };
        assert!(
            gap.abs() < tolerance,
            "{}: {gap:+.3} dB from {reference} (tolerance {tolerance})",
            m.stem
        );
    }
}

/// **The GFDM row's whole distance from Gray QPSK, attributed rather than tolerated.** It is
/// commit-and-guard because an inverse amplifies each point differently, so what the curve reads
/// is an average over a spread of per-point SNRs and not a shift of the closed form. What the two
/// contributions *are* is still checkable: the block's one cyclic prefix, and the mean row energy
/// of `A⁻¹` — both computed from the geometry, neither fitted, and together within a tenth of a dB
/// of the measured gap.
#[test]
fn the_zero_forcing_rows_distance_from_qpsk_is_its_own_prefix_and_inverse() {
    let curve = load_curve("multicarrier/gfdm_zf_awgn");
    let measured = sweep::worst_penalty_db(&curve, sdrmm_modem::ber::theory::qpsk_ber, 6.0, 11.0);
    let predicted = *GFDM_OVERHEAD_DB + gfdm_amplification_db();
    assert!(
        (measured - predicted).abs() < 0.15,
        "measured {measured:+.3} dB, prefix {:+.3} + inverse {:+.3} = {predicted:+.3}",
        *GFDM_OVERHEAD_DB,
        gfdm_amplification_db()
    );
}

/// **GFDM's whole trade as two numbers.** Zero forcing removes the self-interference and pays a
/// noise amplification for it; the matched filter pays nothing and keeps the interference as an
/// error floor. So the matched tier leads at low Eb/N0 and walls at high, and the crossing is what
/// makes the pair an entry rather than a preference.
#[test]
fn the_gfdm_tiers_cross_and_the_matched_one_walls() {
    let zf = load_curve("multicarrier/gfdm_zf_awgn");
    let mf = load_curve("multicarrier/gfdm_mf_awgn");
    let at = |curve: &Curve, db: f64| {
        curve
            .points
            .iter()
            .find(|p| (p.ebn0_db - db).abs() < 1e-9)
            .unwrap()
            .rate()
    };
    // At the top of the matched tier's own grid it has stopped improving: that is the floor.
    let top = mf.points.last().unwrap();
    let previous = mf.points[mf.points.len() - 2];
    let decade = (previous.rate() / top.rate()).log10();
    assert!(
        decade < 1.0,
        "the matched tier is still falling a decade per grid step: {decade}"
    );
    // …and the zero-forcing tier has not: it is orders below by the same point.
    assert!(
        at(&zf, top.ebn0_db) < top.rate() / 5.0,
        "zero forcing {:e} against matched {:e} at {} dB",
        at(&zf, top.ebn0_db),
        top.rate(),
        top.ebn0_db
    );
    // At the bottom of the grid the matched tier is ahead, which is the other half of the trade:
    // there is nothing to zero-force out of noise, and the inverse's amplification is pure cost.
    let low = mf.points[0].ebn0_db;
    assert!(
        at(&mf, low) <= at(&zf, low),
        "matched {:e} is not ahead of zero forcing {:e} at {low} dB",
        at(&mf, low),
        at(&zf, low)
    );
}

// --- The OTFS headline (§4.1, cross-entry) -------------------------------------------------------

/// A two-tap echo deep enough to null subcarriers, applied to a whole waveform, plus the channel
/// response the genie receiver is then told about — so the comparison below isolates the precoder
/// and measures nothing about channel estimation.
fn echo(wave: &mut [Complex<f32>], delay: usize, gain: Complex<f32>) {
    for n in (delay..wave.len()).rev() {
        let tap = wave[n - delay];
        wave[n] += gain * tap;
    }
}

fn echo_response(params: &OfdmParams, delay: usize, gain: Complex<f32>) -> Vec<Complex<f32>> {
    params
        .map()
        .occupied()
        .iter()
        .map(|sub| {
            let bin = sub.bin as f64;
            let phase = -std::f64::consts::TAU * bin * delay as f64 / params.fft() as f64;
            Complex::new(1.0, 0.0) + gain * Complex::from_polar(1.0, phase as f32)
        })
        .collect()
}

/// **The phase's headline, measured — and it is not the one the literature's summary suggests.**
///
/// Phase 6 recorded that an uncoded one-tap equaliser loses a nulled subcarrier outright. OTFS
/// spreads every symbol over every subcarrier, so the intuition is that a null should cost the
/// frame a little instead of costing one subcarrier everything. The measurement says that depends
/// entirely on the equaliser, and says so in both directions:
///
/// - With **zero forcing**, spreading makes things *worse*. Dividing by a near-null multiplies
///   that bin's noise by `1/|H|²`, and the despread then shares that amplified noise out over
///   every symbol in the frame — so instead of one subcarrier's bits being lost, all of them are
///   degraded.
/// - With **MMSE**, which bounds the amplification instead of inverting it, the diversity is
///   there: the null's bin is attenuated rather than amplified, and the despread averages a
///   bounded loss over the frame.
///
/// Both numbers are asserted, because the pair is the finding: **spreading turns a localised
/// failure into a shared one, and whether that is an improvement is a property of the equaliser
/// and not of the precoder.**
#[test]
fn what_otfs_spreading_buys_depends_entirely_on_the_equaliser() {
    let params = OfdmParams::wifi_like();
    let (delay, gain) = (4usize, Complex::new(-0.944f32, 0.0));
    let response = echo_response(&params, delay, gain);
    let deepest = response
        .iter()
        .map(|h| f64::from(h.norm()))
        .fold(f64::INFINITY, f64::min);
    assert!(
        deepest < 0.1,
        "the echo nulls nothing: deepest |H| {deepest}"
    );

    let ber = |precode: bool, mmse: bool| {
        let mut link = if precode {
            otfs_link()
        } else {
            ofdm_reference_link()
        };
        let inner = std::mem::replace(&mut link.modulate, Box::new(|_| Vec::new()));
        link.modulate = Box::new(move |bits| {
            let mut wave = inner(bits);
            echo(&mut wave, delay, gain);
            wave
        });
        link.demodulate = equalising_demodulator(&response, precode, mmse);
        sweep_ber(
            &link,
            &ChannelSpec::default(),
            &[14.0],
            0x0_7f5,
            400,
            400_000,
        )
        .points[0]
            .rate()
    };
    let (ofdm_zf, otfs_zf) = (ber(false, false), ber(true, false));
    let (ofdm_mmse, otfs_mmse) = (ber(false, true), ber(true, true));
    assert!(
        otfs_zf > 2.0 * ofdm_zf,
        "zero forcing: OTFS {otfs_zf:e} should be well behind plain OFDM {ofdm_zf:e}"
    );
    assert!(
        otfs_mmse < ofdm_mmse / 2.0,
        "MMSE: OTFS {otfs_mmse:e} should be well ahead of plain OFDM {ofdm_mmse:e}"
    );
    // And the OFDM row's own floor is the phase-6 result restated: one nulled subcarrier out of
    // 48, its bits essentially random, is a BER floor near 1/96 whatever the equaliser.
    assert!(
        (0.002..0.05).contains(&ofdm_mmse),
        "plain OFDM's floor through a null: {ofdm_mmse:e}"
    );
}

/// A demodulator that reads the *unequalised* subcarriers — the genie is told a flat channel — and
/// then applies the stated equaliser itself, optionally despreading. Written here rather than in
/// the registry because a measurement taken through a channel the receiver was told about is a
/// comparison, not an entry.
fn equalising_demodulator(
    response: &[Complex<f32>],
    precode: bool,
    mmse: bool,
) -> sdrmm_modem::ber::sweep::DemodulateFn {
    use sdrmm_modem::{multicarrier::OtfsPrecoder, ofdm::OfdmDemod};
    let params = OfdmParams::wifi_like();
    let mut demod = OfdmDemod::new(params.clone())
        .with_pilot_tracking(false)
        .with_window_backoff(0);
    demod.genie(
        LEAD + params.data_offset(),
        &vec![Complex::new(1.0, 0.0); params.map().occupied().len()],
        1.0,
    );
    // The genie's channel covers every occupied bin; only the data bins reach the output, in the
    // map's own order.
    let data: Vec<Complex<f32>> = params
        .map()
        .data()
        .iter()
        .map(|sub| {
            let index = params
                .map()
                .occupied()
                .iter()
                .position(|occupied| occupied.bin == sub.bin)
                .expect("every data subcarrier is occupied");
            response[index]
        })
        .collect();
    let grid = sdrmm_modem::multicarrier::OtfsGrid::new(params.data_subcarriers(), SYMBOLS);
    let constellation = sdrmm_modem::constellation::tables::qam_square(4).expect("qpsk table");
    // MMSE's regularisation is the noise-to-signal ratio at the operating point, which the genie
    // knows for the same reason it knows the channel.
    let nu = 10f32.powf(-14.0 / 10.0);
    Box::new(move |wave| {
        let mut demod = demod.clone();
        let mut tf = Vec::with_capacity(grid.points());
        demod.demodulate(wave, SYMBOLS, &mut tf);
        if tf.len() != grid.points() {
            return Vec::new();
        }
        for (k, value) in tf.iter_mut().enumerate() {
            let h = data[k % data.len()];
            *value *= if mmse {
                h.conj() / (h.norm_sqr() + nu)
            } else {
                h.inv()
            };
        }
        let points = if precode {
            let mut dd = vec![Complex::new(0.0, 0.0); grid.points()];
            OtfsPrecoder::new(grid).despread(&tf, &mut dd);
            dd
        } else {
            tf
        };
        let labels: Vec<u32> = points
            .iter()
            .map(|&p| constellation.hard_slice(p))
            .collect();
        catalog::linear::labels_to_bits(&labels, 2)
    })
}

// --- Level-1 E2E (§4.4) ---------------------------------------------------------------------------

/// Every entry's payload survives its own link at a stated margin above its committed 1e-3
/// sensitivity. The matched GFDM tier is exempt and its exemption is the finding: an error floor
/// is not a sensitivity, so no margin exists at which it is clean — the same shape of exemption
/// the tracked-timing 16-QAM row carries.
/// One level-1 loopback: artifact stem, chain, and the margin above its own sensitivity.
type Loopback = (&'static str, fn() -> Link, f64);

#[test]
fn every_entry_loops_back_clean_at_its_stated_margin() {
    let rows: [Loopback; 4] = [
        ("multicarrier/gfdm_zf_awgn", gfdm_zf_link, 6.0),
        ("multicarrier/ufmc_awgn", ufmc_link, 6.0),
        ("multicarrier/fbmc_awgn", fbmc_link, 6.0),
        ("multicarrier/otfs_awgn", otfs_link, 6.0),
    ];
    for (stem, build, margin) in rows {
        let mut link = build();
        let payloads = Payloads::new(0x9c_e2e, 8, link.bits_per_trial);
        let mut channel =
            channel_at_margin(&ChannelSpec::default(), &link, sensitivity(stem), margin);
        assert_eq!(
            loopback(&mut link, &mut channel, payloads),
            Ok(()),
            "{stem} did not survive +{margin} dB over its own sensitivity"
        );
    }
}

// --- §4.3 limits ------------------------------------------------------------------------------------

const LIMITS_TOLERANCE: f64 = 0.2;

/// The axes every multicarrier table carries — one set, so the tables are row-for-row comparable
/// and the differences between them belong to the waveforms.
fn axis_rows(link: &Link, op_db: f64, seed: u64, clean: &Curve) -> Vec<LimitRow> {
    let penalty = penalty_criterion(clean, op_db, 1.0).expect("the grid must cover op − 1 dB");
    let probe = |spec: ChannelSpec| limits::measure_ber(link, &spec, op_db, seed, 60, 200_000);
    vec![
        limits::measure_axis_row("static CFO", "cycles/sample", penalty, 0.01, 1e-7, |cfo| {
            probe(ChannelSpec::default().cfo(Cfo::from_cycles_per_sample(cfo)))
        }),
        // A narrow bracket, because these receivers carry no carrier loop at all: a drift of
        // `d` cycles/sample² turns a 1408-sample frame by `½·d·N²` cycles, so anything a wider
        // bracket would resolve is already a whole turn.
        limits::measure_axis_row(
            "frequency drift",
            "cycles/sample^2",
            penalty,
            1e-8,
            1e-14,
            |rate| probe(ChannelSpec::default().drift(Drift::from_hz_per_s(rate, 1.0))),
        ),
        limits::measure_axis_row("sample clock", "ppm", penalty, 20_000.0, 1.0, |ppm| {
            probe(ChannelSpec::default().clock(ClockError::new(ppm)))
        }),
        limits::measure_axis_row(
            "static timing offset",
            "samples",
            Criterion::FailureBer,
            32.0,
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
    ]
}

type Table = (&'static str, &'static str, fn() -> Link, u64);

const TABLES: [Table; 2] = [
    (GFDM_LIMITS, "gfdm-zf", gfdm_zf_link, GFDM_ZF_SEED),
    (OTFS_LIMITS, "otfs", otfs_link, OTFS_SEED_FOR_LIMITS),
];

/// The OTFS table is measured at its own seed rather than the curve's, so the axis searches are
/// independent realisations of the same chain.
const OTFS_SEED_FOR_LIMITS: u64 = 0x0_07f6;

/// The sensitivity sweep the tables are built on: the committed grid plus two points, because a
/// §4.3 operating point is the 1e-3 crossing *plus three dB* and the ≤1 dB criterion is read a dB
/// below that — which is past the top of a grid drawn for the curve rather than for the table.
const LIMITS_GRID: &[f64] = &[0.0, 2.0, 4.0, 6.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];

fn measure_table(entry: &str, link: &Link, seed: u64) -> LimitsTable {
    let sensitivity = limits::measure_sensitivity(
        link,
        &ChannelSpec::default(),
        LIMITS_GRID,
        seed,
        FULL_ERRORS,
        CAP,
    );
    let mut table = LimitsTable::new(entry, seed, &sensitivity);
    let op_db = table
        .operating_point_db()
        .expect("the committed grid must bracket BER 1e-3");
    table
        .rows
        .extend(axis_rows(link, op_db, seed ^ 0x11e5, &sensitivity.curve));
    table.rows.push(limits::measure_profile_degradation(
        link,
        &ChannelSpec::default(),
        CompositeProfile::StaticIndoor,
        LIMITS_GRID,
        seed ^ 0x51de,
        400,
        400_000,
    ));
    table
}

#[test]
#[ignore = "full limits run; the axis searches are minutes of sweeping"]
fn every_committed_limits_table_still_holds() {
    for (stem, entry, build, seed) in TABLES {
        let committed = limits::load_json(&baseline_path(stem)).unwrap();
        let measured = measure_table(entry, &build(), seed);
        if let Err(faults) = limits::compare_tables(&measured, &committed, LIMITS_TOLERANCE) {
            panic!("{stem} regressed:\n  {}", faults.join("\n  "));
        }
    }
}

// --- Regeneration -----------------------------------------------------------------------------------

#[test]
#[ignore = "full sweep; run to (re)generate the committed curves"]
fn measure_all_curves_full() {
    for m in measurements() {
        let curve = measure(m, true);
        println!("--- {}", m.stem);
        for p in &curve.points {
            println!(
                "{:>5.1} dB  {:>8}/{:<10} {:.3e}",
                p.ebn0_db,
                p.errors,
                p.trials,
                p.rate()
            );
        }
        if let Some((reference, gap, tolerance)) = m.reference_gap(&curve) {
            println!("   {gap:+.4} dB vs {reference} (tolerance {tolerance})");
        }
        let path = baseline_path(m.stem);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        sweep::save_json(&curve, &path).unwrap();
    }
}

#[test]
#[ignore = "full limits run; run to (re)generate the committed tables"]
fn measure_all_limits_full() {
    for (stem, entry, build, seed) in TABLES {
        let table = measure_table(entry, &build(), seed);
        println!("--- {entry}: {table:#?}");
        let path = baseline_path(stem);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        limits::save_json(&table, &path).unwrap();
    }
}

#[test]
#[ignore = "prints the committed numbers this phase's catalog rows quote; asserts nothing"]
fn print_catalog_numbers() {
    println!("sample rate {} MHz", RATE / 1e6);
    for m in measurements() {
        let curve = load_curve(m.stem);
        let gap = m
            .reference_gap(&curve)
            .map(|(name, gap, _)| format!("{gap:+.3} dB vs {name}"));
        println!(
            "{:<32} 1e-3 at {:>7} dB  {}",
            m.stem,
            limits::ebn0_at_ber(&curve, 1e-3).map_or("—".to_string(), |v| format!("{v:.2}")),
            gap.unwrap_or_else(|| "commit-and-guard".to_string())
        );
    }
}
