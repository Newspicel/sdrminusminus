#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{
    BAUD, DEVIATION_HZ, RATE, RRC_ALPHA, STEADY_PREAMBLE, STEADY_TAIL, UW_SYMBOLS, alternating,
    baseline_path, dmr_entry, dmr_params, find_uw, recovered_symbols, uw_dibits, uw_recent_first,
};
use num_complex::Complex;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, ChannelRx, DmrChannel, channel_filter, testgen::dv as tg,
};
use sdrmm_modem::{
    ber::{
        Curve,
        impair::{Awgn, BurstModel, Cfo, ChannelSpec, ClockError, Drift, Impairment, TimingOffset},
        limits::{self, Criterion, LimitRow, LimitsTable},
        rng::Rng,
        sweep::{self, Link},
    },
    cpm::{KnownSymbols, TIMING_BW_BURST, TIMING_BW_CONTINUOUS},
};
use sdrmm_wire::{ChannelSettings, DecoderEvent, DvFrameKind};

const SPS: usize = 10;

/// Samples of dead air ahead of the first burst, so the gate's floor estimate has settled
/// (its own settle window is 3840 samples) on the channel's true noise before any burst.
const BURST_LEAD_SAMPLES: usize = 12_000;

/// Symbols an anchored level estimate survives — the decoder's own allowance. The chains here
/// never tick the hook, so within a trial an anchor only ever expires by being replaced.
const ANCHOR_TIMEOUT: u32 = 4_800;

fn dibit_bits(dibit: u8, bits: &mut Vec<bool>) {
    bits.push(dibit & 0b10 != 0);
    bits.push(dibit & 0b01 != 0);
}

fn slice_all(symbols: &[f32]) -> Vec<u8> {
    let entry = dmr_entry();
    symbols.iter().map(|&s| entry.mapping().slice(s)).collect()
}

const STEADY_BITS: usize = 4096;

fn steady_link() -> Link {
    Link {
        label: "dmr steady uncoded, testgen c4fm (CpmMod) -> channel filter -> CpmDemod at \
                burst timing bw, 88-symbol preamble + 24-symbol sync overhead in Eb, release"
            .to_string(),
        bits_per_trial: STEADY_BITS,
        modulate: Box::new(|bits| {
            let mut symbols: Vec<u8> = alternating(STEADY_PREAMBLE).collect();
            symbols.extend(uw_dibits());
            symbols.extend(tg::dibits(bits));
            symbols.extend(alternating(STEADY_TAIL));
            tg::c4fm(&symbols, RATE, BAUD, DEVIATION_HZ, RRC_ALPHA)
        }),
        demodulate: Box::new(|wave| {
            let sliced = slice_all(&recovered_symbols(wave, true, TIMING_BW_BURST));
            let uw = uw_dibits();
            let Some(at) = find_uw(&sliced, STEADY_PREAMBLE, STEADY_PREAMBLE + 56, &uw) else {
                return Vec::new();
            };
            let mut bits = Vec::with_capacity(STEADY_BITS);
            for k in 0..STEADY_BITS / 2 {
                let dibit = sliced.get(at + UW_SYMBOLS + k).copied().unwrap_or(0);
                dibit_bits(dibit, &mut bits);
            }
            bits
        }),
    }
}

/// One parameterisation of the DMR-shaped burst chain. The defaults are DMR's own numbers:
/// 132 content symbols (24 sync + 108 payload) in every 288, i.e. 156 symbols dead — the
/// limits axes vary one field each.
#[derive(Clone, Copy)]
struct BurstRecipe {
    payload_symbols: usize,
    off_symbols: usize,
    payload_frames: usize,
    /// `BurstModel` level step applied to alternate bursts; negative attenuates.
    level_step_db: f64,
}

impl BurstRecipe {
    fn dmr(payload_frames: usize) -> Self {
        Self {
            payload_symbols: 108,
            off_symbols: 156,
            payload_frames,
            level_step_db: 0.0,
        }
    }

    fn content(&self) -> usize {
        UW_SYMBOLS + self.payload_symbols
    }

    fn frame_symbols(&self) -> usize {
        self.content() + self.off_symbols
    }

    /// The radiated window per frame, in samples: the content symbols plus the full RRC tails
    /// either side, so keying never robs the matched filter of the pulse tails it is built
    /// around. The one-symbol ramps live inside it.
    fn on_samples(&self) -> usize {
        self.content() * SPS + 150
    }

    fn lead_frames(&self) -> usize {
        BURST_LEAD_SAMPLES.div_ceil(self.frame_symbols() * SPS)
    }

    fn bits(&self) -> usize {
        2 * self.payload_symbols * self.payload_frames
    }

    /// `c4fm`'s shaping delay is exactly the tail reach, so frame `j`'s radiated window holds
    /// symbols `[j·frame, j·frame + content)` with their tails: frame 0 is the acquisition
    /// preamble, frames 1..=payload_frames each carry sync + payload.
    fn symbols(&self, payload: &[bool]) -> Vec<u8> {
        let frame = self.frame_symbols();
        let mut symbols: Vec<u8> =
            alternating(frame * self.payload_frames + self.content()).collect();
        let uw = uw_dibits();
        let dibits = tg::dibits(payload);
        for p in 0..self.payload_frames {
            let base = frame * (p + 1);
            symbols[base..base + UW_SYMBOLS].copy_from_slice(&uw);
            let src = &dibits[p * self.payload_symbols..(p + 1) * self.payload_symbols];
            symbols[base + UW_SYMBOLS..base + self.content()].copy_from_slice(src);
        }
        symbols
    }

    /// The impairment template carrying this recipe's TDMA carving; the sweep owns AWGN.
    fn channel(&self) -> ChannelSpec {
        let frame_samples = self.frame_symbols() * SPS;
        ChannelSpec::default().burst(BurstModel::new(
            self.on_samples(),
            frame_samples - self.on_samples(),
            SPS,
            self.level_step_db,
            40.0,
        ))
    }

    fn link(&self, label: &str) -> Link {
        let recipe = *self;
        let demod_recipe = *self;
        Link {
            label: label.to_string(),
            bits_per_trial: self.bits(),
            modulate: Box::new(move |bits| {
                let mut wave =
                    vec![Complex::default(); recipe.lead_frames() * recipe.frame_symbols() * SPS];
                wave.extend(tg::c4fm(
                    &recipe.symbols(bits),
                    RATE,
                    BAUD,
                    DEVIATION_HZ,
                    RRC_ALPHA,
                ));
                wave
            }),
            demodulate: Box::new(move |wave| demod_recipe.demodulate(wave)),
        }
    }

    fn demodulate(&self, wave: &[Complex<f32>]) -> Vec<bool> {
        let entry = dmr_entry();
        let symbols = recovered_symbols(wave, false, TIMING_BW_BURST);
        let sliced = slice_all(&symbols);
        let uw = uw_dibits();
        let pattern = uw_recent_first();
        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut levels = KnownSymbols::new(&entry, ANCHOR_TIMEOUT);
        let mut bits = Vec::with_capacity(self.bits());
        let mut delay: usize = 0;
        for p in 0..self.payload_frames {
            let expect = lead + frame * (p + 1);
            // The first burst's sync is searched wide (front-end delay unknown); later bursts
            // only locally, tracking whatever slip the dead time cost the clock.
            let (lo, hi) = if p == 0 {
                (expect, expect + 56)
            } else {
                ((expect + delay).saturating_sub(4), expect + delay + 4)
            };
            let at = find_uw(&sliced, lo, hi, &uw);
            if let Some(at) = at {
                delay = at.saturating_sub(expect);
                let measured: Vec<f32> = (0..UW_SYMBOLS)
                    .map(|i| symbols[at + UW_SYMBOLS - 1 - i])
                    .collect();
                levels.anchor(&pattern, &measured);
            }
            for k in 0..self.payload_symbols {
                let dibit = at
                    .and_then(|at| symbols.get(at + UW_SYMBOLS + k))
                    .map_or(0, |&s| entry.mapping().slice(levels.correct(s)));
                dibit_bits(dibit, &mut bits);
            }
        }
        bits
    }
}

/// Frames per trial: enough payload (2592 bits) to amortise the acquisition frame, short
/// enough that a trial stays one TDMA breath (~2.5 s of air time per trial).
const BURST_FRAMES: usize = 12;

fn burst_link() -> Link {
    BurstRecipe::dmr(BURST_FRAMES).link(
        "dmr burst uncoded, testgen c4fm (CpmMod) -> BurstModel 132/156 sym TDMA -> channel \
         filter -> CpmDemod + KnownSymbols, sync+preamble overhead in Eb, dead time excluded, \
         release",
    )
}

/// Sweep grids covering each chain's waterfall *and* its error floor — the floor is part of
/// the committed picture, and phase 0's grid is kept so the two generations' points pair.
const STEADY_GRID: [f64; 15] = [
    4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];
const BURST_GRID: [f64; 13] = [
    6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0,
];

const STEADY_SEED: u64 = 0x0d59;
const BURST_SEED: u64 = 0x0d5b;

/// Error budget of the committed curves. Errors here arrive in two populations — a steady
/// trickle and ~1000-error bursts when a trial's clock slips — so the budget is set by the
/// slow tail, not the Gaussian arithmetic: 3000 errors averages 50-100 trials per point,
/// enough that one slipped trial no longer swings its point by half an order of magnitude.
const FULL_ERRORS: u64 = 3_000;
const FULL_CAP: u64 = 8_000_000;

const DMR_FAILURE_BER: f64 = 3e-2;
const DMR_MARGIN_DB: f64 = 6.0;
const DMR_CRITERION: &str = "BER <= 3e-2 at sensitivity(3e-2) + 6 dB \
    (documented 4.3 override: the chain's continuous-mode floor sits above the 1e-3 default)";

fn dmr_search_criterion() -> Criterion {
    Criterion::MaxPenalty {
        penalty_db: 0.0,
        max_ber: DMR_FAILURE_BER,
    }
}

/// The override's operating point, read off the steady sensitivity curve.
fn dmr_operating_point(steady_curve: &Curve) -> f64 {
    limits::ebn0_at_ber(steady_curve, DMR_FAILURE_BER)
        .expect("the steady grid must bracket BER 3e-2")
        + DMR_MARGIN_DB
}

fn axis_row(
    axis: &str,
    unit: &str,
    max_axis: f64,
    tolerance: f64,
    ber_at: impl Fn(f64) -> f64,
) -> LimitRow {
    LimitRow {
        axis: axis.to_string(),
        unit: unit.to_string(),
        threshold: limits::search_axis_limit(dmr_search_criterion(), max_axis, tolerance, ber_at),
        criterion: DMR_CRITERION.to_string(),
    }
}

/// One seeded probe at the operating point. 150 errors holds a clean probe's 95% interval
/// well under the 2.5x gap between the operating-point floor and the 3e-2 limit, and the
/// slip-burst tail is why the budget is not smaller.
fn probe(link: &Link, spec: &ChannelSpec, op_db: f64) -> f64 {
    limits::measure_ber(link, spec, op_db, STEADY_SEED ^ 0xbe5, 150, 60_000)
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
        axis_row("sample clock", "ppm", 100_000.0, 500.0, |ppm| {
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

/// Burst axes vary the recipe itself, so every probe builds its own link — the searched value
/// must reshape the transmitter and the accounting together, not just an impairment knob.
fn burst_axis_rows(op_db: f64) -> Vec<LimitRow> {
    let probe_frames = 6;
    vec![
        axis_row("dead time", "symbols", 1_024.0, 16.0, |off| {
            let mut recipe = BurstRecipe::dmr(probe_frames);
            recipe.off_symbols = (off.round() as usize).max(16);
            let link = recipe.link("dead-time probe");
            probe(&link, &recipe.channel(), op_db)
        }),
        axis_row(
            "burst shortening",
            "payload symbols removed (of 108)",
            96.0,
            2.0,
            |removed| {
                let mut recipe = BurstRecipe::dmr(probe_frames);
                recipe.payload_symbols = 108 - (removed.round() as usize).min(96);
                let link = recipe.link("burst-length probe");
                probe(&link, &recipe.channel(), op_db)
            },
        ),
        axis_row(
            "level step",
            "dB attenuation of alternate bursts",
            12.0,
            0.25,
            |db| {
                let mut recipe = BurstRecipe::dmr(probe_frames);
                recipe.level_step_db = -db;
                let link = recipe.link("level-step probe");
                probe(&link, &recipe.channel(), op_db)
            },
        ),
    ]
}

/// The full table. The sensitivity sweep is parameter-identical to the committed steady
/// curve (same link, grid, seed, budgets), so the table's curve *is* that artifact and the
/// smoke tier can read the operating point straight off `dmr_steady_uncoded_cpm.json`.
fn measure_limits() -> LimitsTable {
    let steady = steady_link();
    let sensitivity = limits::measure_sensitivity(
        &steady,
        &ChannelSpec::default(),
        &STEADY_GRID,
        STEADY_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let mut table = LimitsTable::new("dmr-cpm-chain", STEADY_SEED, &sensitivity);
    let op_db = dmr_operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&steady, op_db);
    table.rows.extend(burst_axis_rows(op_db));
    table
}

/// A harness defect (alignment, sign, level scale) is loud before any statistics: with
/// almost no noise, one trial of each chain must sit on the chain's own residual floor. That
/// floor is not zero at the burst timing bandwidth (module docs) — but a harness bug
/// (mis-alignment, wrong bit pairing) reads tens of percent, an order past anything the chain
/// itself produces.
#[test]
fn both_chains_round_trip_near_their_floor_at_high_ebn0() {
    let steady = limits::measure_ber(&steady_link(), &ChannelSpec::default(), 30.0, 0x0c1e, 1, 1);
    assert!(steady < 3e-2, "steady chain floor {steady} at 30 dB Eb/N0");
    let recipe = BurstRecipe::dmr(BURST_FRAMES);
    let burst = limits::measure_ber(&burst_link(), &recipe.channel(), 30.0, 0x0c1e, 1, 1);
    assert!(burst < 2e-2, "burst chain floor {burst} at 30 dB Eb/N0");
}

/// The phase-0 finding, resolved and held at the channel level: a continuous random stream
/// through the full production chain (channel filter included) at the entry's *continuous*
/// timing operating point holds ≤ 1e-3 over 18 000 post-acquisition symbols — measured 0
/// errors — where the phase-0 chain, and this chain at its burst operating point, floor near
/// 1e-2 with errors *accumulating* past ~2000 symbols. The engine's own tests hold the same
/// number on the bare demodulator; this one proves the DMR chain composition keeps it.
///
/// The 2000-symbol skip is the continuous point's cold acquisition, measured through this
/// chain: the channel filter ahead of the demodulator stretches the 0.003 cy/sym loop's
/// pull-in to ~1500 symbols (on this seed the transient's last error sits at symbol 1539,
/// and nothing follows), a once-per-tune cost with the opposite shape of the phase-0 defect —
/// a head transient that ends, not a wander that begins.
#[test]
fn a_continuous_stream_at_the_continuous_operating_point_beats_the_phase0_floor() {
    let mut rng = Rng::new(0x5eed);
    let sent: Vec<u8> = (0..20_000).map(|_| (rng.next_u64() & 3) as u8).collect();
    let wave = tg::c4fm(&sent, RATE, BAUD, DEVIATION_HZ, RRC_ALPHA);
    let got = slice_all(&recovered_symbols(&wave, true, TIMING_BW_CONTINUOUS));
    // Searched alignment (filter cascade delay), scored over a post-acquisition window.
    let (delay, _) = (0..64usize)
        .map(|d| {
            let errors = (3_000..4_000usize)
                .filter(|&i| sent.get(i.wrapping_sub(d)) != got.get(i))
                .count();
            (d, errors)
        })
        .min_by_key(|&(_, errors)| errors)
        .unwrap();
    let mut errors = 0usize;
    let mut total = 0usize;
    for (i, symbol) in got.iter().enumerate().skip(2_000) {
        let Some(sent) = sent.get(i.wrapping_sub(delay)) else {
            continue;
        };
        total += 1;
        errors += usize::from(sent != symbol);
    }
    assert!(total > 17_500, "only {total} symbols recovered");
    println!(
        "continuous 4FSK through the DMR chain at TIMING_BW_CONTINUOUS: {errors} dibit errors \
         in {total} post-acquisition symbols ({:.1e})",
        errors as f64 / total as f64
    );
    assert!(
        errors <= total / 1_000,
        "{errors} dibit errors in {total}: the continuous floor is back"
    );
}

#[test]
fn the_new_chain_meets_the_phase0_reference() {
    for (old_name, new_name, waterfall_bers) in [
        (
            "dmr_steady_uncoded.json",
            "dmr_steady_uncoded_cpm.json",
            &[1e-1, 7e-2, 5e-2, 3.5e-2][..],
        ),
        (
            "dmr_burst_uncoded.json",
            "dmr_burst_uncoded_cpm.json",
            &[1e-1, 5e-2, 2e-2, 1e-2, 5e-3][..],
        ),
    ] {
        let old = sweep::load_json(&baseline_path(old_name)).unwrap();
        let new = sweep::load_json(&baseline_path(new_name)).unwrap();
        for &ber in waterfall_bers {
            let penalty = sweep::penalty_db_vs_curve(&new, &old, ber);
            assert!(
                penalty < 0.5,
                "{new_name} vs {old_name} at BER {ber:.0e}: {penalty:+.2} dB (must be within \
                 0.5 dB of phase 0 or better)"
            );
            println!("{new_name} at BER {ber:.0e}: {penalty:+.2} dB vs phase 0");
        }
        let (old_floor, new_floor) = (
            old.points.last().unwrap().rate(),
            new.points.last().unwrap().rate(),
        );
        println!("{new_name} top-of-grid floor {new_floor:.2e} (phase 0: {old_floor:.2e})");
    }

    let old = limits::load_json(&baseline_path("dmr_limits.json")).unwrap();
    let new = limits::load_json(&baseline_path("dmr_limits_cpm.json")).unwrap();
    if let Err(faults) = limits::compare_tables(&new, &old, 0.2) {
        panic!("limits rows worse than phase 0 beyond the 20% tolerance: {faults:#?}");
    }
    for row in &new.rows {
        let committed = old.rows.iter().find(|r| r.axis == row.axis).unwrap();
        println!(
            "{:<20} {:>12.4} {} (phase 0: {:.4})",
            row.axis, row.threshold, row.unit, committed.threshold
        );
    }
}

/// Smoke tier of the committed curves: the first three grid points re-measured with the
/// committed budgets. A sweep point's realisation is named by (seed, grid index), so a
/// prefix of the grid reproduces the committed points exactly — bit-identical on one host —
/// and the 0.5 dB slack only has cross-platform float drift to absorb. Prefix points rather
/// than mid-waterfall ones because any chain change moves every point, and only a prefix
/// keeps the (seed, index) pairing that makes the comparison exact rather than a fresh
/// realisation of a heavy-tailed measurement.
#[test]
fn steady_curve_matches_committed_baseline() {
    let committed = sweep::load_json(&baseline_path("dmr_steady_uncoded_cpm.json")).unwrap();
    let measured = sweep::sweep_ber(
        &steady_link(),
        &ChannelSpec::default(),
        &STEADY_GRID[..3],
        STEADY_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let worst = sweep::worst_penalty_db_vs_curve(&measured, &committed, 4.0, 6.0);
    assert!(worst.abs() < 0.5, "steady drift vs committed: {worst} dB");
}

#[test]
fn burst_curve_matches_committed_baseline() {
    let committed = sweep::load_json(&baseline_path("dmr_burst_uncoded_cpm.json")).unwrap();
    let measured = sweep::sweep_ber(
        &burst_link(),
        &BurstRecipe::dmr(BURST_FRAMES).channel(),
        &BURST_GRID[..3],
        BURST_SEED,
        FULL_ERRORS,
        FULL_CAP,
    );
    let worst = sweep::worst_penalty_db_vs_curve(&measured, &committed, 6.0, 8.0);
    assert!(worst.abs() < 0.5, "burst drift vs committed: {worst} dB");
}

/// Smoke tier of the limits table: every axis row re-measured with the committed budgets must
/// sit within 20% of its committed threshold, one-sided — moving better is never a failure.
/// The operating point comes from the committed sensitivity, so this test does not pay for a
/// sensitivity resweep; the curve smoke tests above guard that number.
#[test]
fn limits_rows_match_committed_table() {
    let committed = limits::load_json(&baseline_path("dmr_limits_cpm.json")).unwrap();
    let steady_curve = sweep::load_json(&baseline_path("dmr_steady_uncoded_cpm.json")).unwrap();
    let op_db = dmr_operating_point(&steady_curve);
    let steady = steady_link();
    let mut measured = steady_axis_rows(&steady, op_db);
    measured.extend(burst_axis_rows(op_db));
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
    assert!(faults.is_empty(), "limits regressions: {faults:#?}");
}

/// A complete synthetic call from the existing testgen builders, through the impair channel
/// at a healthy margin, into the actual `DmrChannel`. The dead-air lead is inside the waveform
/// so the AWGN covers it and the front end's gate measures the operational floor, as in the
/// burst chain. A receiver may acquire after the one conventional call header, so the late-entry
/// LC and terminator are the events this impaired-channel test requires; the clean-path test
/// checks every header field.
///
/// 15 dB Eb/N0, not 12: the fragile path here is late entry — four consecutive voice bursts
/// of embedded link-control fragments, all of which must survive their QR(16,7) and BPTC —
/// and at 12 dB (uncoded burst BER ~1.5e-2) the phase-0 chain lost a fragment.
#[test]
fn synthetic_call_decodes_through_an_impaired_channel() {
    let call = tg::dmr::Call::default();
    let mut wave = vec![Complex::default(); (RATE * 0.25) as usize];
    wave.extend(tg::dmr::transmission(&call, RATE));
    let info_bits = 8 * 264;
    let channel = ChannelSpec::default()
        .awgn(Awgn::for_ebn0(15.0, info_bits))
        .build();
    channel.apply(&mut wave, &mut Rng::new(0x0e2e));

    let mut chan = DmrChannel::new(
        ChannelCtx { input_rate: RATE },
        ChannelSettings {
            offset_hz: 0.0,
            squelch_db: None,
            params: dmr_params(),
        },
    )
    .unwrap();
    let mut filter = channel_filter(&dmr_params()).unwrap();
    let mut filtered = Vec::new();
    let mut out = ChannelOutputs::default();
    let mut frames = Vec::new();
    // Ragged blocks, as the decoder unit tests feed: burst state must survive any split.
    let mut pos = 0;
    for len in [997usize, 1, 4_096, 65, 2_048, 7].iter().cycle() {
        if pos >= wave.len() {
            break;
        }
        let end = (pos + len).min(wave.len());
        filter.process(&wave[pos..end], &mut filtered);
        out.reset();
        chan.process(&filtered, &mut out);
        for event in out.events.drain(..) {
            let DecoderEvent::Dv(frame) = event else {
                panic!("unexpected event kind");
            };
            frames.push(frame);
        }
        pos = end;
    }

    let voice = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Voice)
        .expect("late entry: no embedded link control survived the impaired channel");
    assert_eq!(voice.destination, Some(call.destination));
    assert_eq!(voice.source, Some(call.source));
    assert_eq!(voice.color_code, Some(u16::from(call.color_code)));

    let terminator = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Terminator)
        .expect("no terminator decoded through the impaired channel");
    assert_eq!(terminator.destination, Some(call.destination));
    assert_eq!(terminator.source, Some(call.source));
}

fn remeasure_curve(link: &Link, template: &ChannelSpec, grid: &[f64], seed: u64, name: &str) {
    let curve = sweep::sweep_ber(link, template, grid, seed, FULL_ERRORS, FULL_CAP);
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
}

/// Run in release: `cargo test -p sdrmm-channels --release --test dmr_baseline -- --ignored`.
#[test]
#[ignore = "full sweep (~2e7 trial bits); run in release to (re)generate the committed curve"]
fn measure_dmr_steady_full() {
    remeasure_curve(
        &steady_link(),
        &ChannelSpec::default(),
        &STEADY_GRID,
        STEADY_SEED,
        "dmr_steady_uncoded_cpm.json",
    );
}

#[test]
#[ignore = "full sweep (~2e7 trial bits); run in release to (re)generate the committed curve"]
fn measure_dmr_burst_full() {
    remeasure_curve(
        &burst_link(),
        &BurstRecipe::dmr(BURST_FRAMES).channel(),
        &BURST_GRID,
        BURST_SEED,
        "dmr_burst_uncoded_cpm.json",
    );
}

#[test]
#[ignore = "full limits run; run in release to (re)generate the committed table"]
fn measure_dmr_limits_full() {
    let table = measure_limits();
    println!(
        "sensitivity 1e-2 {:?}  1e-3 {:?}  1e-4 {:?}",
        table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
    );
    for row in &table.rows {
        println!("{:<20} {:>12.4} {}", row.axis, row.threshold, row.unit);
    }
    let path = baseline_path("dmr_limits_cpm.json");
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
#[ignore = "prints coarse curves to choose sweep grids; asserts nothing"]
fn probe_grids() {
    for (link, template, name) in [
        (steady_link(), ChannelSpec::default(), "steady"),
        (
            burst_link(),
            BurstRecipe::dmr(BURST_FRAMES).channel(),
            "burst",
        ),
    ] {
        let grid: Vec<f64> = (2..=12).map(|d| f64::from(d) * 2.0).collect();
        let curve = sweep::sweep_ber(&link, &template, &grid, 0x9999, 100, 200_000);
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
