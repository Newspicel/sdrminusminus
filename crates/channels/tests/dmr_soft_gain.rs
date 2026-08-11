//! Soft-BPTC gain on the phase-0 DMR chain (MODEM-PLAN §7 phase 1): the same testgen C4FM →
//! channel filter → `Fsk4Demod` chain the committed uncoded baselines were measured on, now
//! carrying BPTC(196,96)-coded payloads decoded twice from the *identical* received stream —
//! once hard (`Bptc196::decode` on the signs), once soft (`Bptc196::decode_soft` on
//! `fsk4::soft_bits` output) — so the measured gap between the two curves is the soft
//! decoder's alone, with the channel realisation, framing overhead and Eb accounting common
//! to both. The headline number, the horizontal gap at post-FEC BER 1e-3, is committed in
//! `baselines/dmr/dmr_bptc_gain.json` and is the phase-1 regression gate.
//!
//! **Bit→dibit order.** `Bptc196::encode`'s 196 coded bits are mapped pairwise onto 98 dibits,
//! bit `2k` the dibit's high bit — `tg::dibits`' pairing, and exactly the order the real
//! channel uses: `dv/dmr.rs::data_burst` reassembles the BPTC block from burst bits 0..98 and
//! 166..264 in transmission order, and `dv/mod.rs` unpacks every recovered dibit high bit
//! first. Each coded frame rides the phase-0 steady framing: 88-symbol preamble, the 24-symbol
//! BS voice sync, 98 payload symbols, 40-symbol tail.
//!
//! **Eb accounting.** Eb is per *information* bit (crate-root convention): 96 bits against the
//! whole radiated frame, so the preamble/sync/tail energy (250 symbols carrying 98) and the
//! code redundancy (196/96) are both charged, as the phase-0 steady curve charges its own
//! overhead. That puts this axis ~6.9 dB right of the uncoded steady axis at equal channel
//! SNR; the gain is a horizontal read between two curves with identical overhead, so none of
//! it survives into the committed number.
//!
//! **Metrics are separate (§4.1).** Each committed file carries three labelled curves:
//! - `ber`  — residual payload-bit errors over *accepted* frames only (trials = 96 × accepted).
//!   A refused frame contributes no bits here; folding invented half-wrong frames into BER
//!   would let the refusal policy masquerade as bit errors.
//! - `fer`  — refused frames (decode returned `None`, or the sync could not be placed at all)
//!   over all frames.
//! - `undetected` — accepted frames carrying at least one wrong payload bit, over accepted
//!   frames: the rate at which a decoder hands wrong data onward as good.
//!
//! **Short independent frames.** One 96-bit frame per trial, fresh front end per trial: the
//! phase-0 finding is that the timing loop wanders past ~2000 continuous symbols, and these
//! 250-symbol frames sit far inside the clean region. The full run confirms the crossings
//! don't lean on that floor: post-FEC BER 1e-3 is reached at raw dibit BER ~5.3e-2 (soft) /
//! ~3.7e-2 (hard), several times above the long-stream ~1e-2 floor. What the sweep *does*
//! surface is a per-frame error-clustering population even on frames this short — raw BER
//! decays toward ~1e-2-order territory only slowly at the top of the grid, and the hard
//! decoder keeps refusing ~24% of frames at 18 dB where soft refuses ~1.7% — so the
//! accepted-frame BER settles onto an undetected-frame-driven floor (hard ~5e-4, soft
//! ~1.3e-4) rather than falling forever. The undetected rates are high in absolute terms
//! (0.3–15% of accepted frames across the sweep) because BPTC(196,96) is the only integrity
//! layer in this chain — the real channel stacks CRC/RS checks above it — which is exactly
//! why §4.1 wants the metric committed on its own axis rather than blended into BER.
//!
//! **Measured result (full run, release, seed 0xd5f):** hard crosses post-FEC BER 1e-3 at
//! 16.51 dB, soft at 14.91 dB — **gain 1.60 dB**, inside the plan's expected 1–2 dB order.
//! Soft also collapses FER across the whole sweep (e.g. 43% → 8.6% at 16 dB).
//!
//! The hard decoder's input is the sign of the same soft stream the soft decoder reads
//! (`soft > 0`), so the decoders are compared on identical evidence; a sub-quantum slicer
//! difference right at a decision boundary is charged to neither.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::{
    BAUD, DEVIATION_HZ, RATE, RRC_ALPHA, STEADY_PREAMBLE, STEADY_TAIL, UW_SYMBOLS, alternating,
    baseline_path, find_uw, recovered_symbols, uw_dibits,
};
use num_complex::Complex;
use sdrmm_channels::testgen::dv as tg;
use sdrmm_dsp::{Bptc196, fec::conv::Soft, fsk4};
use sdrmm_modem::ber::{
    Curve, CurvePoint,
    impair::{Awgn, ChannelSpec, Impairment},
    rng::Rng,
    sweep,
};
use serde::{Deserialize, Serialize};

const INFO_BITS: usize = Bptc196::DATA_BITS;
const PAYLOAD_SYMBOLS: usize = Bptc196::CODED_BITS / 2;

/// Sweep grid bracketing post-FEC BER 1e-3 on *both* curves (probe-placed: soft crosses near
/// 15 dB, hard near 16.5 dB), with a shoulder either side — the top points show the floor the
/// accepted-frame BER settles onto, which is part of the committed picture.
const GRID: [f64; 6] = [13.0, 14.0, 15.0, 16.0, 17.0, 18.0];
const SEED: u64 = 0x0d5f;

/// Error budget per point, on each decoder's residual-bit count. The `MIN_ERRORS_PER_POINT`
/// note scales budgets with the local log-slope, and this chain adds clustering: residual bits
/// arrive ~5–6 to a wrong frame, so the independent count behind a point is its *wrong frames*
/// — 600 bit errors is ~100 wrong frames, which at the measured ~0.3–0.4 decade/dB waterfall
/// holds a point's horizontal 95% interval near ±0.25 dB, inside the 0.5 dB gates. The frame
/// cap bounds the shoulder points where soft errors stop arriving; 30 000 frames still
/// resolves BER ~1e-4 with a real count rather than a bound.
const FULL_MIN_ERRORS: u64 = 600;
const FULL_MAX_FRAMES: u64 = 30_000;

/// The committed gain is read at this post-FEC BER (MODEM-PLAN §7 phase 1).
const AT_BER: f64 = 1e-3;

const RECIPE: &str = "96 info bits -> Bptc196 -> tg::dibits -> steady framing \
    (88 preamble + 24 sync + 98 payload + 40 tail) -> tg::c4fm 48k/4800 -> AWGN \
    (Eb per info bit, overhead charged) -> channel filter -> Fsk4Demod -> fsk4::soft_bits; \
    hard = decode(sign), soft = decode_soft, same stream; gain = horizontal gap of the \
    accepted-frame BER curves at 1e-3, release";

// --- The coded chain -------------------------------------------------------------------------

fn modulate(coded: &[bool; Bptc196::CODED_BITS]) -> Vec<Complex<f32>> {
    let mut symbols: Vec<u8> = alternating(STEADY_PREAMBLE).collect();
    symbols.extend(uw_dibits());
    symbols.extend(tg::dibits(coded));
    symbols.extend(alternating(STEADY_TAIL));
    tg::c4fm(&symbols, RATE, BAUD, DEVIATION_HZ, RRC_ALPHA)
}

/// Demodulate, align on the sync (searched, as the steady chain does), and demap the 98
/// payload symbols to 196 soft values. `None` only when the output is too short to even place
/// the sync — a frame lost before any decoder saw it, counted as refused by both.
fn received_soft(wave: &[Complex<f32>]) -> Option<[Soft; Bptc196::CODED_BITS]> {
    let symbols = recovered_symbols(wave, true);
    let sliced: Vec<u8> = symbols.iter().map(|&s| fsk4::slice(s)).collect();
    let at = find_uw(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 56, &uw_dibits())?;
    let start = at + UW_SYMBOLS;
    if symbols.len() < start + PAYLOAD_SYMBOLS {
        return None;
    }
    let mut soft = [0 as Soft; Bptc196::CODED_BITS];
    for k in 0..PAYLOAD_SYMBOLS {
        let [negative, outer] = fsk4::soft_bits(symbols[start + k]);
        soft[2 * k] = negative;
        soft[2 * k + 1] = outer;
    }
    Some(soft)
}

// --- Per-point measurement -------------------------------------------------------------------

/// One decoder's counts at one point; the three committed metrics read straight off it.
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

/// The sweep runner's own seeding convention (`sweep::point_seed`): a golden-ratio stride
/// keeps `index → seed` injective, so any point regenerates alone from `(seed, index)`.
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

// --- Committed artifacts ---------------------------------------------------------------------

/// One decoder's committed measurement: the three §4.1 metrics as separately labelled curves.
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
        format!("dmr bptc196 {name}, phase-0 steady chain, {what}, seed {SEED:#x}, release")
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

/// The committed headline: the horizontal gap between the hard and soft accepted-frame BER
/// curves at [`AT_BER`], re-derivable from the two committed curve files alone.
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

// --- Always-run regression gates -------------------------------------------------------------

/// Harness defects (mis-alignment, wrong bit pairing, sign convention) are loud before any
/// statistics: at 30 dB one frame must survive both decoders untouched, with zero corrections.
#[test]
fn coded_frame_round_trips_clean_at_high_ebn0() {
    let stats = measure_point(30.0, 0, 0x0c1f, 1, 1);
    for (name, t) in [("hard", stats.hard), ("soft", stats.soft)] {
        assert_eq!(t.frames, 1);
        assert_eq!(t.rejected, 0, "{name} refused a clean frame");
        assert_eq!(t.bit_errors, 0, "{name} residual errors on a clean frame");
    }
    assert_eq!(
        stats.raw_bit_errors, 0,
        "clean frame arrived with raw errors"
    );
}

/// Smoke tier of the committed curves: the first two grid points re-measured with the
/// committed budgets. Point realisations are named by `(seed, index)` (the sweep runner's
/// convention), so a grid prefix reproduces the committed points bit-identically on one host
/// and the 0.5 dB slack absorbs only cross-platform float drift. All three metrics gate: at
/// these low-SNR points every count is in the hundreds, real statistics for each.
#[test]
fn coded_curves_match_committed_baselines() {
    let hard: CodedCurves = load(&baseline_path("dmr_bptc_hard.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_bptc_soft.json"));
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

/// The committed headline number must be exactly what the committed curves say: the gain is
/// re-derived from the two curve artifacts and compared to the recorded one, so neither can
/// drift without the other.
#[test]
fn committed_gain_re_derives_from_the_committed_curves() {
    let hard: CodedCurves = load(&baseline_path("dmr_bptc_hard.json"));
    let soft: CodedCurves = load(&baseline_path("dmr_bptc_soft.json"));
    let gain: GainRecord = load(&baseline_path("dmr_bptc_gain.json"));
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

// --- Full re-measurement (nightly; regenerates the committed artifacts) ----------------------

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

/// Point-by-point in rate (the curves are non-monotone on their floors, where a horizontal
/// read is nonzero even for an identical curve): same-host regeneration reproduces committed
/// counts bit-for-bit, so the log-ratio allowance is for cross-platform float drift only.
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

/// Run in release: `cargo test -p sdrmm-channels --release --test dmr_soft_gain -- --ignored`.
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
    for (name, curves) in [("dmr_bptc_hard.json", &hard), ("dmr_bptc_soft.json", &soft)] {
        let path = baseline_path(name);
        if path.exists() {
            assert_curves_match(name, curves, &load(&path));
        } else {
            save(curves, &path);
            println!("baseline created at {}", path.display());
        }
    }
    let path = baseline_path("dmr_bptc_gain.json");
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

// --- Exploration (never asserted; kept ignored for grid placement) ---------------------------

#[test]
#[ignore = "prints coarse curves to place the sweep grid; asserts nothing"]
fn probe_grid() {
    for db in [14.0, 16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0, 24.0] {
        let p = measure_point(db, (db * 2.0) as usize, 0x9998, 150, 2_000);
        print_point(&p);
    }
}
