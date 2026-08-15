#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::{
    BAUD, DEVIATION_HZ, RATE, RRC_ALPHA, STEADY_PREAMBLE, STEADY_TAIL, UW_SYMBOLS, alternating,
    baseline_path, dmr_entry, find_uw, recovered_symbols, uw_dibits,
};
use num_complex::Complex;
use sdrmm_channels::testgen::dv as tg;
use sdrmm_dsp::{
    Bptc196,
    fec::conv::{CONFIDENT, Soft},
};
use sdrmm_modem::{
    ber::{
        Curve, CurvePoint,
        impair::{Awgn, ChannelSpec, Impairment},
        rng::Rng,
        sweep,
    },
    cpm::TIMING_BW_BURST,
};
use serde::{Deserialize, Serialize};

const INFO_BITS: usize = Bptc196::DATA_BITS;
const PAYLOAD_SYMBOLS: usize = Bptc196::CODED_BITS / 2;

const GRID: [f64; 6] = [13.0, 14.0, 15.0, 16.0, 17.0, 18.0];
const SEED: u64 = 0x0d5f;

const FULL_MIN_ERRORS: u64 = 600;
const FULL_MAX_FRAMES: u64 = 30_000;

const AT_BER: f64 = 1e-3;

const RECIPE: &str = "96 info bits -> Bptc196 -> tg::dibits -> steady framing \
    (88 preamble + 24 sync + 98 payload + 40 tail) -> tg::c4fm (CpmMod) 48k/4800 -> AWGN \
    (Eb per info bit, overhead charged) -> channel filter -> CpmDemod (burst timing bw) -> \
    Mapping::soft_bits x CONFIDENT; hard = decode(sign), soft = decode_soft, same stream; \
    gain = horizontal gap of the accepted-frame BER curves at 1e-3, release";

fn modulate(coded: &[bool; Bptc196::CODED_BITS]) -> Vec<Complex<f32>> {
    let mut symbols: Vec<u8> = alternating(STEADY_PREAMBLE).collect();
    symbols.extend(uw_dibits());
    symbols.extend(tg::dibits(coded));
    symbols.extend(alternating(STEADY_TAIL));
    tg::c4fm(&symbols, RATE, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

fn received_soft(wave: &[Complex<f32>]) -> Option<[Soft; Bptc196::CODED_BITS]> {
    let entry = dmr_entry();
    let symbols = recovered_symbols(wave, true, TIMING_BW_BURST);
    let sliced: Vec<u8> = symbols.iter().map(|&s| entry.mapping().slice(s)).collect();
    let at = find_uw(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 56, &uw_dibits())?;
    let start = at + UW_SYMBOLS;
    if symbols.len() < start + PAYLOAD_SYMBOLS {
        return None;
    }
    let mut soft = [0 as Soft; Bptc196::CODED_BITS];
    let mut demapped = Vec::with_capacity(2);
    for k in 0..PAYLOAD_SYMBOLS {
        demapped.clear();
        entry.mapping().soft_bits(symbols[start + k], &mut demapped);
        soft[2 * k] = (demapped[0].0 * f32::from(CONFIDENT)) as Soft;
        soft[2 * k + 1] = (demapped[1].0 * f32::from(CONFIDENT)) as Soft;
    }
    Some(soft)
}

#[derive(Default, Clone, Copy)]
struct Tally {
    frames: u64,
    rejected: u64,
    bit_errors: u64,
    wrong_frames: u64,
}

impl Tally {
    fn accepted(&self) -> u64 {
        self.frames - self.rejected
    }

    fn score(&mut self, decoded: Option<([bool; INFO_BITS], u32)>, payload: &[bool; INFO_BITS]) {
        self.frames += 1;
        let Some((data, _)) = decoded else {
            self.rejected += 1;
            return;
        };
        let wrong = data.iter().zip(payload).filter(|(a, b)| a != b).count() as u64;
        self.bit_errors += wrong;
        self.wrong_frames += u64::from(wrong > 0);
    }
}

struct PointStats {
    ebn0_db: f64,
    hard: Tally,
    soft: Tally,
    raw_bit_errors: u64,
    raw_bits: u64,
}

fn point_seed(seed: u64, index: usize) -> u64 {
    seed.wrapping_add((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

fn measure_point(
    ebn0_db: f64,
    index: usize,
    seed: u64,
    min_errors: u64,
    max_frames: u64,
) -> PointStats {
    let mut rng = Rng::new(point_seed(seed, index));
    let channel = ChannelSpec::default()
        .awgn(Awgn::for_ebn0(ebn0_db, INFO_BITS as u64))
        .build();
    let mut stats = PointStats {
        ebn0_db,
        hard: Tally::default(),
        soft: Tally::default(),
        raw_bit_errors: 0,
        raw_bits: 0,
    };
    while (stats.hard.bit_errors < min_errors || stats.soft.bit_errors < min_errors)
        && stats.hard.frames < max_frames
    {
        let mut payload = [false; INFO_BITS];
        for bit in &mut payload {
            *bit = rng.next_u64() & 1 == 1;
        }
        let coded = Bptc196::encode(&payload);
        let mut wave = modulate(&coded);
        channel.apply(&mut wave, &mut rng);
        match received_soft(&wave) {
            Some(soft) => {
                let hard: [bool; Bptc196::CODED_BITS] = std::array::from_fn(|i| soft[i] > 0);
                stats.raw_bit_errors +=
                    hard.iter().zip(&coded).filter(|(a, b)| a != b).count() as u64;
                stats.raw_bits += Bptc196::CODED_BITS as u64;
                stats.hard.score(Bptc196::decode(&hard), &payload);
                stats.soft.score(Bptc196::decode_soft(&soft), &payload);
            }
            None => {
                stats.hard.frames += 1;
                stats.hard.rejected += 1;
                stats.soft.frames += 1;
                stats.soft.rejected += 1;
            }
        }
    }
    stats
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CodedCurves {
    ber: Curve,
    fer: Curve,
    undetected: Curve,
}

fn coded_curves(
    points: &[PointStats],
    tally: impl Fn(&PointStats) -> Tally,
    name: &str,
) -> CodedCurves {
    let point = |f: &dyn Fn(Tally) -> (u64, u64)| -> Vec<CurvePoint> {
        points
            .iter()
            .map(|p| {
                let (errors, trials) = f(tally(p));
                CurvePoint {
                    ebn0_db: p.ebn0_db,
                    errors,
                    trials,
                }
            })
            .collect()
    };
    let label = |what: &str| {
        format!("dmr bptc196 {name}, phase-3 cpm chain, {what}, seed {SEED:#x}, release")
    };
    CodedCurves {
        ber: Curve {
            label: label("post-FEC BER over accepted frames"),
            points: point(&|t| (t.bit_errors, t.accepted() * INFO_BITS as u64)),
        },
        fer: Curve {
            label: label("FER (refused frames / all frames)"),
            points: point(&|t| (t.rejected, t.frames)),
        },
        undetected: Curve {
            label: label("undetected-error rate (wrong accepted frames / accepted frames)"),
            points: point(&|t| (t.wrong_frames, t.accepted())),
        },
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct GainRecord {
    at_ber: f64,
    gain_db: f64,
    seed: u64,
    recipe: String,
}

fn gain_db(hard: &CodedCurves, soft: &CodedCurves) -> f64 {
    sweep::penalty_db_vs_curve(&hard.ber, &soft.ber, AT_BER)
}

fn save<T: Serialize>(value: &T, path: &Path) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    std::fs::write(path, text).unwrap();
}

fn load<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn coded_frame_round_trips_clean_at_high_ebn0() {
    let stats = measure_point(30.0, 0, 0x0c1f, 1, 1);
    for (name, t) in [("hard", stats.hard), ("soft", stats.soft)] {
        assert_eq!(t.frames, 1);
        assert_eq!(t.rejected, 0, "{name} refused a clean frame");
        assert_eq!(t.bit_errors, 0, "{name} residual errors on a clean frame");
    }

    let mut rng = Rng::new(point_seed(0x0c1f, 0));
    let channel = ChannelSpec::default()
        .awgn(Awgn::for_ebn0(30.0, INFO_BITS as u64))
        .build();
    let mut payload = [false; INFO_BITS];
    for bit in &mut payload {
        *bit = rng.next_u64() & 1 == 1;
    }
    let coded = Bptc196::encode(&payload);
    let mut wave = modulate(&coded);
    channel.apply(&mut wave, &mut rng);
    let soft = received_soft(&wave).expect("clean frame lost before the sync");
    for (i, (&s, &sent)) in soft.iter().zip(&coded).enumerate() {
        assert!(
            (s > 0) == sent || s == 0,
            "coded bit {i}: sent {sent}, confidently wrong soft value {s}"
        );
    }
}

#[test]
fn coded_curves_match_committed_baselines() {
    let hard: CodedCurves = load(&baseline_path("dmr_bptc_hard_cpm.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_bptc_soft_cpm.json"));
    let points: Vec<PointStats> = GRID[..2]
        .iter()
        .enumerate()
        .map(|(i, &db)| measure_point(db, i, SEED, FULL_MIN_ERRORS, FULL_MAX_FRAMES))
        .collect();
    let measured_hard = coded_curves(&points, |p| p.hard, "hard");
    let measured_soft = coded_curves(&points, |p| p.soft, "soft");
    for (name, measured, committed) in [
        ("hard ber", &measured_hard.ber, &hard.ber),
        ("hard fer", &measured_hard.fer, &hard.fer),
        (
            "hard undetected",
            &measured_hard.undetected,
            &hard.undetected,
        ),
        ("soft ber", &measured_soft.ber, &soft.ber),
        ("soft fer", &measured_soft.fer, &soft.fer),
        (
            "soft undetected",
            &measured_soft.undetected,
            &soft.undetected,
        ),
    ] {
        let worst = sweep::worst_penalty_db_vs_curve(measured, committed, GRID[0], GRID[1]);
        assert!(worst.abs() < 0.5, "{name} drift vs committed: {worst} dB");
    }
}

#[test]
fn committed_gain_re_derives_from_the_committed_curves() {
    let hard: CodedCurves = load(&baseline_path("dmr_bptc_hard_cpm.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_bptc_soft_cpm.json"));
    let gain: GainRecord = load(&baseline_path("dmr_bptc_gain_cpm.json"));
    assert_eq!(gain.at_ber, AT_BER);
    assert_eq!(gain.seed, SEED);
    assert_eq!(gain.recipe, RECIPE);
    let derived = gain_db(&hard, &soft);
    assert!(
        derived.is_finite(),
        "committed curves no longer bracket BER {AT_BER}"
    );
    assert!(
        (derived - gain.gain_db).abs() < 1e-9,
        "committed gain {} dB, curves say {derived} dB",
        gain.gain_db
    );
}

#[test]
fn the_gain_survives_the_migration() {
    let old: GainRecord = load(&baseline_path("dmr_bptc_gain.json"));
    let new: GainRecord = load(&baseline_path("dmr_bptc_gain_cpm.json"));
    assert!(
        new.gain_db > old.gain_db - 0.5,
        "soft-BPTC gain collapsed across the migration: phase 0 {} dB, cpm chain {} dB",
        old.gain_db,
        new.gain_db
    );
    assert!(
        new.gain_db >= 1.0,
        "soft-BPTC gain {} dB fell out of the plan's expected 1-2 dB order",
        new.gain_db
    );
    println!(
        "soft-BPTC gain at BER {AT_BER}: phase 0 {:.2} dB -> cpm chain {:.2} dB",
        old.gain_db, new.gain_db
    );
}

fn print_point(p: &PointStats) {
    println!(
        "{:>5.1} dB  raw {:.3e}  hard fer {:.3e} ber {:.3e} undet {}/{}  \
         soft fer {:.3e} ber {:.3e} undet {}/{}",
        p.ebn0_db,
        p.raw_bit_errors as f64 / p.raw_bits.max(1) as f64,
        p.hard.rejected as f64 / p.hard.frames.max(1) as f64,
        p.hard.bit_errors as f64 / (p.hard.accepted() * INFO_BITS as u64).max(1) as f64,
        p.hard.wrong_frames,
        p.hard.accepted(),
        p.soft.rejected as f64 / p.soft.frames.max(1) as f64,
        p.soft.bit_errors as f64 / (p.soft.accepted() * INFO_BITS as u64).max(1) as f64,
        p.soft.wrong_frames,
        p.soft.accepted(),
    );
}

fn assert_curves_match(name: &str, measured: &CodedCurves, committed: &CodedCurves) {
    for (what, m, c) in [
        ("ber", &measured.ber, &committed.ber),
        ("fer", &measured.fer, &committed.fer),
        ("undetected", &measured.undetected, &committed.undetected),
    ] {
        assert_eq!(
            m.points.len(),
            c.points.len(),
            "{name} {what}: grid changed"
        );
        for (mp, cp) in m.points.iter().zip(&c.points) {
            let ratio = (mp.rate().max(1e-12) / cp.rate().max(1e-12)).log10().abs();
            assert!(
                ratio < 0.1,
                "{name} {what} at {} dB: committed {:.3e}, measured {:.3e}",
                cp.ebn0_db,
                cp.rate(),
                mp.rate()
            );
        }
    }
}

#[test]
#[ignore = "full sweep (~1e5 coded frames); run in release to (re)generate the committed artifacts"]
fn measure_soft_bptc_gain_full() {
    let points: Vec<PointStats> = GRID
        .iter()
        .enumerate()
        .map(|(i, &db)| {
            let p = measure_point(db, i, SEED, FULL_MIN_ERRORS, FULL_MAX_FRAMES);
            print_point(&p);
            p
        })
        .collect();
    let hard = coded_curves(&points, |p| p.hard, "hard");
    let soft = coded_curves(&points, |p| p.soft, "soft");
    let gain = GainRecord {
        at_ber: AT_BER,
        gain_db: gain_db(&hard, &soft),
        seed: SEED,
        recipe: RECIPE.to_string(),
    };
    assert!(
        gain.gain_db.is_finite(),
        "grid no longer brackets BER {AT_BER} on both curves"
    );
    println!("soft-BPTC gain at BER {AT_BER}: {:.2} dB", gain.gain_db);
    for (name, curves) in [
        ("dmr_bptc_hard_cpm.json", &hard),
        ("dmr_bptc_soft_cpm.json", &soft),
    ] {
        let path = baseline_path(name);
        if path.exists() {
            assert_curves_match(name, curves, &load(&path));
        } else {
            save(curves, &path);
            println!("baseline created at {}", path.display());
        }
    }
    let path = baseline_path("dmr_bptc_gain_cpm.json");
    if path.exists() {
        let committed: GainRecord = load(&path);
        assert!(
            (gain.gain_db - committed.gain_db).abs() < 0.25,
            "gain moved: committed {} dB, measured {} dB",
            committed.gain_db,
            gain.gain_db
        );
    } else {
        save(&gain, &path);
        println!("gain recorded at {}", path.display());
    }
}

#[test]
#[ignore = "prints coarse curves to place the sweep grid; asserts nothing"]
fn probe_grid() {
    for db in [14.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 24.0] {
        let p = measure_point(db, (db * 2.0) as usize, 0x9998, 150, 2_000);
        print_point(&p);
    }
}
