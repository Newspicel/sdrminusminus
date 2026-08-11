//! Phase-0 baseline of the CURRENT DMR chain (MODEM-PLAN §7 phase 0 accept) — the
//! pre-migration reference phase 3 will be held to. Transmit side is the established one:
//! `testgen::dv::c4fm` (RRC α=0.2 shaping, FM at ±1944 Hz outer deviation, 48 kHz / 4800
//! baud). Receive side is the chain as production runs it: the DMR channel-selection filter
//! into `Fsk4Demod`, sliced to dibits. The committed artifacts live in `baselines/dmr/` and
//! regress here:
//!
//! - `dmr_steady_uncoded.json` — continuously-keyed dibit BER curve.
//! - `dmr_burst_uncoded.json`  — the same chain through the calibrated `impair::BurstModel`,
//!   carved exactly as the `fsk4::tests::keyed` recipe radiates: 132 content symbols of every
//!   288 (24-symbol sync + 108 payload symbols per burst), full pulse tails kept, one-symbol
//!   keying ramps, receiver noise floor 40 dB down in the gaps. BER is counted over payload
//!   symbols only; the sync overhead *is* charged to Eb (per-information-bit accounting), so
//!   this curve sits right of the steady one by the framing cost as well as the TDMA penalty.
//! - `dmr_limits.json` — §4.3 resistance table: sensitivities off the steady curve, axis rows
//!   (CFO, drift, sample-clock ppm, static timing) on the steady chain, burst rows (dead
//!   time, burst shortening, level step) on the burst chain, all under the documented
//!   criterion override in [`DMR_CRITERION`].
//!
//! **The baseline's loudest finding:** on *continuous* random 4FSK the current chain's timing
//! loop wanders, and past ~2000 symbols it occasionally slips outright — a dibit-BER floor
//! near 1e-2 that no Eb/N0 buys back (the fsk4 unit tests never see it: they check at most a
//! few hundred symbols, and in TDMA operation the carrier gate freezes the loop through every
//! gap before wander can accumulate). The floor is why the burst-model curve *beats* the
//! steady one at high Eb/N0, why sensitivity at 1e-3/1e-4 is committed as unmeasured, and why
//! the limits rows operate at a documented 3e-2 criterion instead of §4.3's 1e-3-anchored
//! default. Phase 3's engine must beat this floor, and now has the committed number to beat.
//!
//! Alignment is never assumed: each chain locates the DMR BS voice sync in its own sliced
//! output (the fsk4 tests' searched-alignment idiom, made payload-blind), and the burst chain
//! re-anchors per burst — position against slip, levels via `fsk4::SyncLevels`, exactly the
//! decoder's own sync-anchored correction.
//!
//! Warm-up differs per chain on purpose. The steady chain is met the way a continuously keyed
//! carrier is: the demodulator has heard 0.2 s of quiet receiver noise (no channel noise), so
//! its gate holds open through the signal whatever the swept Eb/N0. The burst chain instead
//! carries 0.25 s of dead air *inside* the waveform, ahead of the first burst, so the AWGN
//! axis covers it and the gate measures its floor from the channel it will actually gate —
//! without that, dead-time behaviour at low Eb/N0 would be measured with the gate blinded.
//!
//! Committed numbers were measured in `--release` (the curve labels say so); every run is
//! seeded, so re-measurement with unchanged code reproduces them bit-for-bit on one host.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use num_complex::Complex;
use sdrmm_channels::{
    ChannelCtx, ChannelOutputs, ChannelRx, DmrChannel, channel_filter, testgen::dv as tg,
};
use sdrmm_dsp::{Fsk4Demod, fsk4};
use sdrmm_modem::ber::{
    Curve,
    impair::{Awgn, BurstModel, Cfo, ChannelSpec, ClockError, Drift, Impairment, TimingOffset},
    limits::{self, Criterion, LimitRow, LimitsTable},
    rng::Rng,
    sweep::{self, Link},
};
use sdrmm_wire::{ChannelParams, ChannelSettings, DecoderEvent, DmrParams, DvFrameKind};

const RATE: f64 = 48_000.0;
const BAUD: f64 = 4_800.0;
const DEVIATION_HZ: f64 = 1_944.0;
const RRC_ALPHA: f64 = 0.2;
const SPS: usize = 10;

/// DMR BS-sourced voice sync (ETSI TS 102 361-1 §9.1.1) — the unique word both chains align
/// on and the burst chain anchors levels to, as the decoder itself does.
const UW: u64 = 0x755F_D7DF_75F7;
const UW_SYMBOLS: usize = 24;

/// Samples of dead air ahead of the first burst, so the gate's floor estimate has settled
/// (its own settle window is 3840 samples) on the channel's true noise before any burst.
const BURST_LEAD_SAMPLES: usize = 12_000;

fn dmr_params() -> ChannelParams {
    ChannelParams::Dmr(DmrParams::default())
}

fn uw_dibits() -> Vec<u8> {
    tg::dibits(&tg::bits(UW, 48))
}

/// Receiver noise at 40 dB below a unit carrier — what the steady chain's demodulator hears
/// before the transmission, exactly the `fsk4` tests' `listening` convention.
fn quiet(seed: u64, len: usize) -> Vec<Complex<f32>> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|_| {
            let re = (rng.uniform() * 2.0 - 1.0) * 0.01;
            let im = (rng.uniform() * 2.0 - 1.0) * 0.01;
            Complex::new(re as f32, im as f32)
        })
        .collect()
}

/// The receive front end under measurement, as production runs it: the DMR channel-selection
/// filter (the receiver's noise bandwidth — without it the discriminator eats the full 48 kHz
/// and the waterfall shifts ~6 dB right) into `Fsk4Demod`, fresh per trial so every trial is
/// independent and reproducible from its own seed.
fn recovered_symbols(wave: &[Complex<f32>], warm_up: bool) -> Vec<f32> {
    let mut filter = channel_filter(&dmr_params()).unwrap();
    let mut demod = Fsk4Demod::new(RATE, BAUD, DEVIATION_HZ, RRC_ALPHA);
    let mut filtered = Vec::new();
    if warm_up {
        let mut discard = Vec::new();
        filter.process(&quiet(0x1157, (RATE * 0.2) as usize), &mut filtered);
        demod.process(&filtered, &mut discard);
    }
    let mut symbols = Vec::new();
    filter.process(wave, &mut filtered);
    demod.process(&filtered, &mut symbols);
    symbols
}

fn uw_distance(sliced: &[u8], at: usize, uw: &[u8]) -> u32 {
    uw.iter()
        .enumerate()
        .map(|(i, &d)| (sliced[at + i] ^ d).count_ones())
        .sum()
}

/// Best sync position in `lo..=hi` by Hamming distance — the searched-alignment idiom. No
/// threshold: a chain too degraded to place its sync scores its garbage as bit errors.
fn find_uw(sliced: &[u8], lo: usize, hi: usize, uw: &[u8]) -> Option<usize> {
    let last = hi.min(sliced.len().checked_sub(uw.len())?);
    (lo..=last).min_by_key(|&at| uw_distance(sliced, at, uw))
}

fn dibit_bits(dibit: u8, bits: &mut Vec<bool>) {
    bits.push(dibit & 0b10 != 0);
    bits.push(dibit & 0b01 != 0);
}

// --- Steady-state chain ----------------------------------------------------------------------

/// Clock pull-in from a cold phase costs ~80 symbols (fsk4's own tests); the preamble covers
/// that before the sync so the payload is met by a locked loop.
const STEADY_PREAMBLE: usize = 88;
const STEADY_BITS: usize = 4096;
/// Trailing filler past the payload: the front end is a whole filter cascade late (~24
/// symbols), so the transmitter must keep shaping that long past the last payload symbol or
/// the demodulator never emits it.
const STEADY_TAIL: usize = 40;

fn alternating(len: usize) -> impl Iterator<Item = u8> {
    (0..len).map(|i| if i % 2 == 0 { 0b01 } else { 0b11 })
}

fn steady_link() -> Link {
    Link {
        label: "dmr steady uncoded, testgen c4fm -> channel filter -> Fsk4Demod, \
                88-symbol preamble + 24-symbol sync overhead in Eb, release"
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
            let sliced: Vec<u8> = recovered_symbols(wave, true)
                .iter()
                .map(|&s| fsk4::slice(s))
                .collect();
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

// --- Burst (TDMA) chain ----------------------------------------------------------------------

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
    /// either side — `fsk4::tests::keyed`'s `radiated`, so keying never robs the matched
    /// filter of the pulse tails it is built around. The one-symbol ramps live inside it.
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
        let symbols = recovered_symbols(wave, false);
        let sliced: Vec<u8> = symbols.iter().map(|&s| fsk4::slice(s)).collect();
        let uw = uw_dibits();
        let frame = self.frame_symbols();
        let lead = self.lead_frames() * frame;
        let mut levels = fsk4::SyncLevels::new();
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
                // The decoder's own trick: the burst's sync knows the levels better than any
                // loop can. Most recent symbol in the pattern's low bits, so the measured
                // window runs backwards from the sync's last symbol.
                let measured: Vec<f32> = (0..UW_SYMBOLS)
                    .map(|i| symbols[at + UW_SYMBOLS - 1 - i])
                    .collect();
                levels.anchor(UW, &measured);
            }
            for k in 0..self.payload_symbols {
                let dibit = at
                    .and_then(|at| symbols.get(at + UW_SYMBOLS + k))
                    .map_or(0, |&s| fsk4::slice(levels.correct(s)));
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
        "dmr burst uncoded, testgen c4fm -> BurstModel 132/156 sym TDMA -> channel filter \
         -> Fsk4Demod + SyncLevels, sync+preamble overhead in Eb, dead time excluded, release",
    )
}

// --- Committed artifacts ---------------------------------------------------------------------

fn baseline_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("baselines/dmr/{name}"))
}

/// Sweep grids covering each chain's waterfall *and* its error floor — the floor is part of
/// the baseline, not a nuisance: phase 3 has to beat it, and can only be held to a number
/// that was committed.
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

// --- Limits table ----------------------------------------------------------------------------

/// Documented §4.3 criterion override for this entry. The default criterion operates at
/// sensitivity(1e-3) + 3 dB — a point the current chain cannot state: its continuous-mode
/// error floor (timing-loop wander on long random 4FSK, see the module docs) sits at ~1e-2,
/// above the 1e-3 the default hangs everything off. So this table's rows pass while BER
/// stays at or under 3e-2 with the link held at its measured 3e-2 sensitivity + 6 dB — the
/// same shape of criterion, restated at a ratio the chain reaches, with the margin widened
/// so the clean floor (~1.2e-2 there) sits a factor below the limit instead of against it.
const DMR_FAILURE_BER: f64 = 3e-2;
const DMR_MARGIN_DB: f64 = 6.0;
const DMR_CRITERION: &str = "BER <= 3e-2 at sensitivity(3e-2) + 6 dB \
    (documented 4.3 override: the chain's continuous-mode floor sits above the 1e-3 default)";

/// Only `ber_limit()` feeds the search; the rows carry [`DMR_CRITERION`] instead of the
/// enum's label, which names the default operating point this entry cannot state.
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
        // "Minimum burst length" spelled so higher stays better for the comparator: the
        // symbols removable from the 108-payload burst; min burst = 24-symbol sync + rest.
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
        // Attenuation of alternate bursts: the decay-limited direction of the level tracker,
        // recovered (or not) within the burst's own 24-symbol sync via SyncLevels.
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
/// smoke tier can read the operating point straight off `dmr_steady_uncoded.json`.
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
    let mut table = LimitsTable::new("dmr-current-chain", STEADY_SEED, &sensitivity);
    let op_db = dmr_operating_point(&sensitivity.curve);
    table.rows = steady_axis_rows(&steady, op_db);
    table.rows.extend(burst_axis_rows(op_db));
    table
}

// --- Always-run regression gates -------------------------------------------------------------

/// A harness defect (alignment, sign, level scale) is loud before any statistics: with
/// almost no noise, one trial of each chain must sit on the chain's own residual floor. That
/// floor is not zero — the timing loop wanders on long continuous random 4FSK (module docs)
/// — but a harness bug (mis-alignment, wrong bit pairing) reads tens of percent, an order
/// past anything the chain itself produces.
#[test]
fn both_chains_round_trip_near_their_floor_at_high_ebn0() {
    let steady = limits::measure_ber(&steady_link(), &ChannelSpec::default(), 30.0, 0x0c1e, 1, 1);
    assert!(steady < 3e-2, "steady chain floor {steady} at 30 dB Eb/N0");
    let recipe = BurstRecipe::dmr(BURST_FRAMES);
    let burst = limits::measure_ber(&burst_link(), &recipe.channel(), 30.0, 0x0c1e, 1, 1);
    assert!(burst < 2e-2, "burst chain floor {burst} at 30 dB Eb/N0");
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
    let committed = sweep::load_json(&baseline_path("dmr_steady_uncoded.json")).unwrap();
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
    let committed = sweep::load_json(&baseline_path("dmr_burst_uncoded.json")).unwrap();
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
    let committed = limits::load_json(&baseline_path("dmr_limits.json")).unwrap();
    // The table's sensitivity sweep is parameter-identical to the committed steady curve, so
    // the operating point reads off that artifact without a resweep.
    let steady_curve = sweep::load_json(&baseline_path("dmr_steady_uncoded.json")).unwrap();
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

// --- Level-2 E2E (MODEM-PLAN §4.4) -----------------------------------------------------------

/// A complete synthetic call from the existing testgen builders, through the impair channel
/// at a healthy margin, into the actual `DmrChannel` — decoded events asserted field by
/// field. The dead-air lead is inside the waveform so the AWGN covers it and the front end's
/// gate measures the operational floor, as in the burst chain.
///
/// 15 dB Eb/N0, not 12: the fragile path here is late entry — four consecutive voice bursts
/// of embedded link-control fragments, all of which must survive their QR(16,7) and BPTC —
/// and at 12 dB (uncoded burst BER ~1.5e-2) it loses a fragment. The margin is a statement
/// about the current chain too, and phase 3 inherits it as a bound to improve on.
#[test]
fn synthetic_call_decodes_through_an_impaired_channel() {
    let call = tg::dmr::Call::default();
    let mut wave = vec![Complex::default(); (RATE * 0.25) as usize];
    wave.extend(tg::dmr::transmission(&call, RATE));
    // 3 repeated headers + 6 voice bursts + terminator = 10 bursts of 264 bits.
    let info_bits = 10 * 264;
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

    let header = frames
        .iter()
        .find(|f| f.kind == DvFrameKind::Header)
        .expect("no voice LC header decoded through the impaired channel");
    assert_eq!(header.slot, Some(1));
    assert_eq!(header.color_code, Some(u16::from(call.color_code)));
    assert_eq!(header.group_call, Some(true));
    assert_eq!(header.destination, Some(call.destination));
    assert_eq!(header.source, Some(call.source));

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

// --- Full re-measurement (nightly; regenerates the committed artifacts) ----------------------

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
        // Point-by-point in rate, not horizontally: the committed curve is non-monotone
        // around its floor (module docs), and a horizontal read against a non-monotone curve
        // is nonzero even for the identical curve. Same seeds and budgets make each point a
        // reproduction of the committed one; the ratio allowance is for float drift across
        // hosts, nothing else.
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
        "dmr_steady_uncoded.json",
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
        "dmr_burst_uncoded.json",
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
    let path = baseline_path("dmr_limits.json");
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

// --- Exploration (never asserted; kept ignored for phase-3 bracket work) -------------------

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
