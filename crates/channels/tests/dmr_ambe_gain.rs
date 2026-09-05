#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use blip25_vocoder::halfrate::frame::{
    CODE_WIDTHS, INFO_WIDTHS, decode_code_vectors, decode_frame_soft, deinterleave,
    encode_code_vectors, interleave,
};
use common::{
    BAUD, DEVIATION_HZ, RATE, RRC_ALPHA, STEADY_PREAMBLE, STEADY_TAIL, UW_SYMBOLS, alternating,
    baseline_path, dmr_entry, find_uw, recovered_symbols, uw_dibits,
};
use num_complex::Complex;
use sdrmm_channels::testgen::dv as tg;
use sdrmm_modem::cpm::TIMING_BW_BURST;
use sdrmm_modem_test_support::ber::{
    Curve, CurvePoint,
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
    sweep,
};
use serde::{Deserialize, Serialize};

const FRAMES_PER_TRIAL: usize = 3;
const DIBITS_PER_FRAME: usize = 36;
const SOFT_BITS: usize = DIBITS_PER_FRAME * 2;
const PAYLOAD_SYMBOLS: usize = FRAMES_PER_TRIAL * DIBITS_PER_FRAME;
const INFO_BITS: usize = FRAMES_PER_TRIAL * 49;
const PROTECTED_BITS: u64 = (INFO_WIDTHS[0] + INFO_WIDTHS[1]) as u64;

const GRID: [f64; 6] = [14.0, 16.0, 18.0, 20.0, 22.0, 24.0];
const SEED: u64 = 0x0a3b;

const FULL_MIN_ERRORS: u64 = 600;
const FULL_MAX_TRIALS: u64 = 30_000;

const AT_BER: f64 = 1e-3;

const RECIPE: &str = "3 half-rate AMBE+2 frames per trial -> encode_code_vectors -> Annex S \
    interleave -> tg::dibits -> steady framing (88 preamble + 24 sync + 108 payload + 40 tail) -> \
    tg::c4fm (CpmMod) 48k/4800 -> AWGN (Eb per info bit, overhead charged) -> channel filter -> \
    CpmDemod (burst timing bw) -> Mapping::soft_bits x i8::MAX; hard = deinterleave(sign) + \
    decode_code_vectors, soft = decode_frame_soft, same stream; scored over the Golay-protected \
    info vectors u0 and u1; gain = horizontal gap of the BER curves at 1e-3, release";

fn info_vectors(rng: &mut Rng) -> [u16; 4] {
    std::array::from_fn(|i| (rng.next_u64() as u16) & ((1u16 << INFO_WIDTHS[i]) - 1))
}

fn modulate(frames: &[[u8; DIBITS_PER_FRAME]]) -> Vec<Complex<f32>> {
    let mut symbols: Vec<u8> = alternating(STEADY_PREAMBLE).collect();
    symbols.extend(uw_dibits());
    symbols.extend(frames.iter().flatten().copied());
    symbols.extend(alternating(STEADY_TAIL));
    tg::c4fm(&symbols, RATE, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

struct Received {
    hard: Vec<[u8; DIBITS_PER_FRAME]>,
    soft: Vec<[i8; SOFT_BITS]>,
}

fn receive(wave: &[Complex<f32>]) -> Option<Received> {
    let entry = dmr_entry();
    let symbols = recovered_symbols(wave, true, TIMING_BW_BURST);
    let sliced: Vec<u8> = symbols.iter().map(|&s| entry.mapping().slice(s)).collect();
    let at = find_uw(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 56, &uw_dibits())?;
    let start = at + UW_SYMBOLS;
    if symbols.len() < start + PAYLOAD_SYMBOLS {
        return None;
    }
    let mut received = Received {
        hard: Vec::with_capacity(FRAMES_PER_TRIAL),
        soft: Vec::with_capacity(FRAMES_PER_TRIAL),
    };
    let mut demapped = Vec::with_capacity(2);
    for frame in 0..FRAMES_PER_TRIAL {
        let base = start + frame * DIBITS_PER_FRAME;
        let mut hard = [0u8; DIBITS_PER_FRAME];
        let mut soft = [0i8; SOFT_BITS];
        for k in 0..DIBITS_PER_FRAME {
            hard[k] = sliced[base + k];
            demapped.clear();
            entry.mapping().soft_bits(symbols[base + k], &mut demapped);
            soft[2 * k] = (demapped[0].0 * f32::from(i8::MAX)) as i8;
            soft[2 * k + 1] = (demapped[1].0 * f32::from(i8::MAX)) as i8;
        }
        received.hard.push(hard);
        received.soft.push(soft);
    }
    Some(received)
}

#[derive(Default, Clone, Copy)]
struct Tally {
    frames: u64,
    bit_errors: u64,
    wrong_frames: u64,
    uncorrectable: u64,
}

impl Tally {
    fn score(&mut self, decoded: &[u16; 4], uncorrectable: bool, sent: &[u16; 4]) {
        self.frames += 1;
        self.uncorrectable += u64::from(uncorrectable);
        let wrong: u32 = (0..2)
            .map(|v| (decoded[v] ^ sent[v]) & ((1u16 << INFO_WIDTHS[v]) - 1))
            .map(u16::count_ones)
            .sum();
        self.bit_errors += u64::from(wrong);
        self.wrong_frames += u64::from(wrong > 0);
    }

    fn lost(&mut self) {
        self.frames += 1;
        self.bit_errors += PROTECTED_BITS;
        self.wrong_frames += 1;
        self.uncorrectable += 1;
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
    max_trials: u64,
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
    let mut trials = 0u64;
    while (stats.hard.bit_errors < min_errors || stats.soft.bit_errors < min_errors)
        && trials < max_trials
    {
        trials += 1;
        let sent: Vec<[u16; 4]> = (0..FRAMES_PER_TRIAL)
            .map(|_| info_vectors(&mut rng))
            .collect();
        let coded: Vec<[u32; 4]> = sent.iter().map(encode_code_vectors).collect();
        let frames: Vec<[u8; DIBITS_PER_FRAME]> = coded.iter().map(interleave).collect();
        let mut wave = modulate(&frames);
        channel.apply(&mut wave, &mut rng);
        let Some(received) = receive(&wave) else {
            for _ in 0..FRAMES_PER_TRIAL {
                stats.hard.lost();
                stats.soft.lost();
            }
            continue;
        };
        for (frame, sent) in sent.iter().enumerate() {
            let hard = deinterleave(&received.hard[frame]);
            for (v, &width) in CODE_WIDTHS.iter().enumerate() {
                stats.raw_bit_errors += u64::from((hard[v] ^ coded[frame][v]).count_ones());
                stats.raw_bits += u64::from(width);
            }
            let hard = decode_code_vectors(hard);
            stats
                .hard
                .score(&hard.info, hard.errors[0] == u8::MAX, sent);
            let soft = decode_frame_soft(&received.soft[frame]);
            stats
                .soft
                .score(&soft.info, soft.errors[0] == u8::MAX, sent);
        }
    }
    stats
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CodedCurves {
    ber: Curve,
    fer: Curve,
    uncorrectable: Curve,
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
        format!("dmr half-rate ambe fec {name}, phase-3 cpm chain, {what}, seed {SEED:#x}, release")
    };
    CodedCurves {
        ber: Curve {
            label: label("post-FEC BER over the Golay-protected info bits"),
            points: point(&|t| (t.bit_errors, t.frames * PROTECTED_BITS)),
        },
        fer: Curve {
            label: label("frames whose protected info vectors came out wrong"),
            points: point(&|t| (t.wrong_frames, t.frames)),
        },
        uncorrectable: Curve {
            label: label("frames the extended Golay refused (synth-side repeat or mute)"),
            points: point(&|t| (t.uncorrectable, t.frames)),
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
fn a_clean_frame_round_trips_through_both_decoders() {
    let stats = measure_point(30.0, 0, 0x0c1f, 1, 1);
    for (name, t) in [("hard", stats.hard), ("soft", stats.soft)] {
        assert_eq!(t.frames, FRAMES_PER_TRIAL as u64);
        assert_eq!(t.bit_errors, 0, "{name} residual errors on a clean frame");
        assert_eq!(t.uncorrectable, 0, "{name} refused a clean frame");
    }
}

#[test]
fn the_soft_decoder_never_trails_the_hard_one() {
    for (index, &db) in GRID[..3].iter().enumerate() {
        let p = measure_point(db, index, SEED, 200, 400);
        assert!(
            p.soft.bit_errors <= p.hard.bit_errors,
            "at {db} dB soft made {} protected-bit errors against hard's {}",
            p.soft.bit_errors,
            p.hard.bit_errors
        );
    }
}

#[test]
fn coded_curves_match_committed_baselines() {
    let hard: CodedCurves = load(&baseline_path("dmr_ambe_hard.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_ambe_soft.json"));
    let points: Vec<PointStats> = GRID[..2]
        .iter()
        .enumerate()
        .map(|(i, &db)| measure_point(db, i, SEED, FULL_MIN_ERRORS, FULL_MAX_TRIALS))
        .collect();
    let measured_hard = coded_curves(&points, |p| p.hard, "hard");
    let measured_soft = coded_curves(&points, |p| p.soft, "soft");
    for (name, measured, committed) in [
        ("hard ber", &measured_hard.ber, &hard.ber),
        ("hard fer", &measured_hard.fer, &hard.fer),
        ("soft ber", &measured_soft.ber, &soft.ber),
        ("soft fer", &measured_soft.fer, &soft.fer),
    ] {
        let worst = sweep::worst_penalty_db_vs_curve(measured, committed, GRID[0], GRID[1]);
        assert!(worst.abs() < 0.5, "{name} drift vs committed: {worst} dB");
    }
}

#[test]
fn committed_gain_re_derives_from_the_committed_curves() {
    let hard: CodedCurves = load(&baseline_path("dmr_ambe_hard.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_ambe_soft.json"));
    let gain: GainRecord = load(&baseline_path("dmr_ambe_gain.json"));
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
    assert!(
        gain.gain_db >= 1.0,
        "soft-AMBE gain {} dB no longer pays for the soft path",
        gain.gain_db
    );
}

fn print_point(p: &PointStats) {
    println!(
        "{:>5.1} dB  raw {:.3e}  hard ber {:.3e} fer {:.3e} refused {:.3e}  \
         soft ber {:.3e} fer {:.3e} refused {:.3e}",
        p.ebn0_db,
        p.raw_bit_errors as f64 / p.raw_bits.max(1) as f64,
        p.hard.bit_errors as f64 / (p.hard.frames * PROTECTED_BITS).max(1) as f64,
        p.hard.wrong_frames as f64 / p.hard.frames.max(1) as f64,
        p.hard.uncorrectable as f64 / p.hard.frames.max(1) as f64,
        p.soft.bit_errors as f64 / (p.soft.frames * PROTECTED_BITS).max(1) as f64,
        p.soft.wrong_frames as f64 / p.soft.frames.max(1) as f64,
        p.soft.uncorrectable as f64 / p.soft.frames.max(1) as f64,
    );
}

fn assert_curves_match(name: &str, measured: &CodedCurves, committed: &CodedCurves) {
    for (what, m, c) in [
        ("ber", &measured.ber, &committed.ber),
        ("fer", &measured.fer, &committed.fer),
        (
            "uncorrectable",
            &measured.uncorrectable,
            &committed.uncorrectable,
        ),
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
#[ignore = "full sweep (~1e5 ambe frames); run in release to (re)generate the committed artifacts"]
fn measure_soft_ambe_gain_full() {
    let points: Vec<PointStats> = GRID
        .iter()
        .enumerate()
        .map(|(i, &db)| {
            let p = measure_point(db, i, SEED, FULL_MIN_ERRORS, FULL_MAX_TRIALS);
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
    println!("soft-AMBE gain at BER {AT_BER}: {:.2} dB", gain.gain_db);
    for (name, curves) in [("dmr_ambe_hard.json", &hard), ("dmr_ambe_soft.json", &soft)] {
        let path = baseline_path(name);
        if path.exists() {
            assert_curves_match(name, curves, &load(&path));
        } else {
            save(curves, &path);
        }
    }
    let path = baseline_path("dmr_ambe_gain.json");
    if path.exists() {
        let committed: GainRecord = load(&path);
        assert!(
            (gain.gain_db - committed.gain_db).abs() < 0.5,
            "gain drifted: committed {} dB, measured {} dB",
            committed.gain_db,
            gain.gain_db
        );
    } else {
        save(&gain, &path);
    }
}
