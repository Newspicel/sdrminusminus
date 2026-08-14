//! §5 measurement bundle for the four spread-spectrum entries (MODEM-PLAN §7 phase 7): committed
//! BER curves for direct sequence, CCK, chirp spreading and hopping; the processing gain measured
//! against `10·log₁₀(chips/symbol)`; the §4.3 limits tables; and level-1 E2E loopbacks.
//!
//! The chains live in `ber::catalog::spread`; the committed artifacts live in `baselines/spread/`.
//!
//! **What the phase is about, in one sentence each.** Under AWGN a spreader is transparent, so the
//! direct-sequence and hopping rows are held to their own constellation's closed form and any dB
//! they sit from it is framing. What spreading buys shows up on the *interference* axes instead —
//! measured on the correlator in `spread::dsss`, where §7's `10·log₁₀(chips/symbol)` is the
//! assertion, and reported here as the committed jammer rows plus the gap between the two
//! quantities (`the_jammer_rows_move_less_than_the_processing_gain_and_the_catalog_says_so`).
//! CCK's chips buy rate rather than rejection, and the trade is committed as a pair of
//! numbers against the direct-sequence row on the identical chip rate. And chirp spreading turns
//! out to be the third member of an identity phase 5 measured twice — M cyclic shifts of one
//! sweep are the same signalling set as M tones or M slots — so it answers to the exact
//! noncoherent orthogonal oracle at `M = 2^SF` rather than to a committed reference.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use sdrmm_modem::{
    ber::{
        Curve,
        catalog::{
            FULL_ERRORS,
            spread::{
                BARKER11_AWGN, BARKER11_GRID, BARKER11_QPSK_AWGN, BARKER11_QPSK_GRID,
                BARKER11_QPSK_SEED, BARKER11_SEED, CCK_LIMITS, CCK11_AWGN, CCK11_GRID, CCK11_SEED,
                CCK55_AWGN, CCK55_GRID, CCK55_SEED, CHIP_SAMPLE_RATE, CHIP_SPS, CSS_BANDWIDTH,
                CSS_LIMITS, CSS_PREAMBLE, CSS_SF7_AWGN, CSS_SF7_GRID, CSS_SF7_SEED, CSS_SF10_AWGN,
                CSS_SF10_GRID, CSS_SF10_SEED, CSS_SF12_AWGN, CSS_SF12_GRID, CSS_SF12_SEED,
                DSSS_LIMITS, DSSS_PAYLOAD, FHSS_AWGN, FHSS_GRID, FHSS_LIMITS, FHSS_SEED, FULL_CAP,
                HOP_CHANNELS, M31_AWGN, M31_GRID, M31_LIMITS, M31_SEED, PREAMBLE, barker11_link,
                barker11_qpsk_link, cck11_link, cck55_link, css_link, css_overhead_db, css_payload,
                dsss_overhead_db, fhss_link, hop_sequence, m31_link,
            },
        },
        e2e::{Payloads, channel_at_margin, loopback},
        impair::{Cfo, ChannelSpec, ClockError, Drift, Interferer, IqImbalance, TimingOffset},
        limits::{self, CompositeProfile, Criterion, LimitRow, LimitsTable},
        sweep::{self, Link},
        theory,
    },
    spread::FhssDemod,
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

/// Every committed row: its chain, grid, seed and artifact. One list, read by the drift gate, the
/// clean-channel gate and the regeneration runner, so a row cannot join two of the three.
type Row = (
    &'static str,
    fn() -> Link,
    &'static [f64],
    u64,
    &'static str,
);

const ROWS: &[Row] = &[
    (
        "barker-11 bpsk",
        barker11_link,
        BARKER11_GRID,
        BARKER11_SEED,
        BARKER11_AWGN,
    ),
    (
        "barker-11 qpsk",
        barker11_qpsk_link,
        BARKER11_QPSK_GRID,
        BARKER11_QPSK_SEED,
        BARKER11_QPSK_AWGN,
    ),
    ("m31 bpsk", m31_link, M31_GRID, M31_SEED, M31_AWGN),
    ("cck 8-bit", cck11_link, CCK11_GRID, CCK11_SEED, CCK11_AWGN),
    ("cck 4-bit", cck55_link, CCK55_GRID, CCK55_SEED, CCK55_AWGN),
    (
        "css sf7",
        css_sf7_link,
        CSS_SF7_GRID,
        CSS_SF7_SEED,
        CSS_SF7_AWGN,
    ),
    (
        "css sf10",
        css_sf10_link,
        CSS_SF10_GRID,
        CSS_SF10_SEED,
        CSS_SF10_AWGN,
    ),
    (
        "css sf12",
        css_sf12_link,
        CSS_SF12_GRID,
        CSS_SF12_SEED,
        CSS_SF12_AWGN,
    ),
    ("fhss barker-11", fhss_link, FHSS_GRID, FHSS_SEED, FHSS_AWGN),
];

fn css_sf7_link() -> Link {
    css_link(7)
}

fn css_sf10_link() -> Link {
    css_link(10)
}

fn css_sf12_link() -> Link {
    css_link(12)
}

// --- Always-run harness gates ----------------------------------------------------------------

/// Every chain has a floor, and it is below anything a curve measures: a defect in the framing,
/// the acquisition or the bit packing shows up here rather than as a mysterious tolerance failure
/// three tests later.
#[test]
fn every_chain_round_trips_clean_at_high_ebn0() {
    for (name, link, ..) in ROWS {
        let ber = limits::measure_ber(&link(), &ChannelSpec::default(), 20.0, 0x0_5b00, 1, 1);
        assert!(ber < 1e-4, "{name} floor {ber} at 20 dB Eb/N0");
    }
}

/// Every committed curve reproduces its artifact's first points at the committed seed and budgets
/// — the drift gate that makes each of them a regression test rather than a snapshot.
#[test]
fn every_committed_curve_matches_its_baseline() {
    for (name, link, grid, seed, stem) in ROWS {
        let committed = load_curve(stem);
        let measured = sweep::sweep_ber(
            &link(),
            &ChannelSpec::default(),
            &grid[..3],
            *seed,
            FULL_ERRORS,
            FULL_CAP,
        );
        let worst = sweep::worst_penalty_db_vs_curve(&measured, &committed, grid[0], grid[2]);
        assert!(worst.abs() < 0.5, "{name} drift vs committed: {worst} dB");
    }
}

/// One judged row: the committed artifact, its grid, and the shifted closed form it answers to.
type OracleRow = (&'static str, &'static [f64], Box<dyn Fn(f64) -> f64>);

/// Every oracle-matched row and its reference. Three of the four entries are here; CCK is the one
/// commit-and-guard row, and what stands in for its closed form is the rate trade measured below.
fn oracle_rows() -> Vec<OracleRow> {
    vec![
        (
            BARKER11_AWGN,
            BARKER11_GRID,
            Box::new(|db| theory::bpsk_ber(db - dsss_overhead_db())),
        ),
        (
            BARKER11_QPSK_AWGN,
            BARKER11_QPSK_GRID,
            Box::new(|db| theory::qpsk_ber(db - dsss_overhead_db())),
        ),
        (
            M31_AWGN,
            M31_GRID,
            Box::new(|db| theory::bpsk_ber(db - dsss_overhead_db())),
        ),
        (
            FHSS_AWGN,
            FHSS_GRID,
            Box::new(|db| theory::bpsk_ber(db - dsss_overhead_db())),
        ),
        (
            CSS_SF7_AWGN,
            CSS_SF7_GRID,
            Box::new(|db| theory::mfsk_noncoherent_ber(128, db - css_overhead_db())),
        ),
        (
            CSS_SF10_AWGN,
            CSS_SF10_GRID,
            Box::new(|db| theory::mfsk_noncoherent_ber(1024, db - css_overhead_db())),
        ),
        (
            CSS_SF12_AWGN,
            CSS_SF12_GRID,
            Box::new(|db| theory::mfsk_noncoherent_ber(4096, db - css_overhead_db())),
        ),
    ]
}

/// **The phase's acceptance.** Every spread row that has a closed form sits on it, shifted by the
/// frame's own overhead and by nothing else.
///
/// Two claims are being made at once and they are worth separating. For the direct-sequence and
/// hopping rows the claim is that *spreading is transparent under AWGN*: a chip carries `1/N` of
/// the symbol's energy and the correlator collects `N` of them, so a spread BPSK curve is a BPSK
/// curve and a hopped one is the same curve again. For the chirp rows the claim is the phase-5
/// identity's third member: `2^SF` cyclic shifts of one sweep are `2^SF` orthogonal signals, so
/// the rows answer to the exact noncoherent orthogonal form at M = 128, 1024 and 4096 — orders
/// no alternating binomial sum can evaluate, which is why the harness grew a second evaluation of
/// that oracle.
#[test]
fn every_oracle_row_sits_on_its_own_closed_form() {
    for (stem, grid, oracle) in oracle_rows() {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, &oracle, grid[0], *grid.last().unwrap());
        assert!(
            worst.abs() < 0.5,
            "{stem}: worst penalty {worst} dB vs its shifted closed form"
        );
    }
}

/// **The processing gain, and what a BER threshold does and does not read.**
///
/// MODEM-PLAN §7 phase 7 asks for the gain measured against `10·log₁₀(chips/symbol)`, and it is —
/// directly, on the correlator, in `spread::dsss`: at a stated C/I the length-31 code collects
/// `10·log10(31/11) = 4.50 dB` less interference than Barker-11, measured, and against an
/// unspread system of the same data rate each code collects `10·log₁₀(N)` less.
///
/// **The committed limits rows do not move by that number, and the gap is the finding.** They
/// read the Eb/N0 at which BER crosses 1e-2 under a narrowband jammer, and that threshold moves
/// by about 0.9 dB where the collected interference power moves by 4.5. A mean interference power
/// and a BER threshold are not the same quantity: a tone's despread contribution is a
/// *coherent* vector of near-constant magnitude across a whole burst rather than an independent
/// draw per symbol, so the error rate is set by how often that vector lands near the decision
/// boundary — and the rest of the burst, being pushed the *other* way, contributes almost
/// nothing to compensate. What the rows are for is the entry's own resistance number; what the
/// gain is for is the comparison, and the two are reported separately rather than one being
/// quoted as the other.
#[test]
fn the_jammer_rows_move_less_than_the_processing_gain_and_the_catalog_says_so() {
    let jammer = |stem: &str| {
        limits::load_json(&baseline_path(stem))
            .unwrap()
            .rows
            .iter()
            .find(|r| r.axis == "narrowband J/C")
            .unwrap_or_else(|| panic!("{stem} carries no narrowband jammer row"))
            .threshold
    };
    let measured = jammer(M31_LIMITS) - jammer(DSSS_LIMITS);
    let gain = 10.0 * (31.0f64 / 11.0).log10();
    assert!((gain - 4.4997).abs() < 1e-3, "predicted gain {gain} dB");
    assert!(
        measured > 0.3,
        "the length-31 code must still tolerate more than Barker-11; it read {measured} dB"
    );
    assert!(
        measured < 0.5 * gain,
        "the jammer row now moves {measured} dB against the {gain} dB of collected interference; \
         if a BER threshold has started tracking mean interference power, this entry's docs are \
         wrong and should be rewritten rather than the bound widened"
    );
}

/// The other half of the same claim, and the reason the jammer row is not simply "more is
/// better": spreading buys **nothing** against thermal noise. The two codes' sensitivities must be
/// the same number, or the rejection above would be a sensitivity difference wearing a jammer's
/// name.
#[test]
fn the_two_spreading_factors_have_the_same_sensitivity() {
    let gap = sensitivity(M31_AWGN) - sensitivity(BARKER11_AWGN);
    assert!(
        gap.abs() < 0.3,
        "the length-31 code sits {gap} dB from Barker-11 under AWGN, where a spreader should cost \
         and buy exactly nothing"
    );
}

/// **CCK's trade, as the pair of numbers that is its commit-and-guard reference.** Eight chips
/// carry eight bits where Barker-11 carries one over eleven — eleven times the rate at the same
/// chip rate — and CCK is nonetheless *ahead* in Eb/N0, because it is a block code in sixteen real
/// dimensions rather than an uncoded point (`spread::cck`'s distance test carries the geometry
/// behind that). The 4-bit rate sits between them.
#[test]
fn cck_buys_rate_and_is_still_ahead_of_the_direct_sequence_row() {
    let barker = sensitivity(BARKER11_AWGN);
    let cck8 = sensitivity(CCK11_AWGN);
    let cck4 = sensitivity(CCK55_AWGN);
    let margin = barker - cck8;
    assert!(
        (0.7..2.5).contains(&margin),
        "8-bit CCK sits {margin} dB ahead of Barker-11 BPSK at 1e-3"
    );
    assert!(
        (cck4 - cck8).abs() < 1.0,
        "the two CCK rates sit {} dB apart",
        cck4 - cck8
    );
    // The rate itself, which is the whole reason the codebook exists: eleven times the bits per
    // chip period.
    let barker_bits_per_chip = 1.0f64 / 11.0;
    let cck_bits_per_chip = 8.0f64 / 8.0;
    assert!((cck_bits_per_chip / barker_bits_per_chip - 11.0).abs() < 1e-12);
}

/// **The chirp entry's own trade**: a spreading factor spends `2^SF` chips on `SF` bits, so the
/// alphabet grows exponentially while the payload grows linearly — and the noncoherent orthogonal
/// curve improves with M. Committed as an ordering plus the size of the step, since a
/// monotonicity alone would pass on a broken engine that simply got slower.
#[test]
fn sensitivity_improves_with_the_spreading_factor() {
    let sf7 = sensitivity(CSS_SF7_AWGN);
    let sf10 = sensitivity(CSS_SF10_AWGN);
    let sf12 = sensitivity(CSS_SF12_AWGN);
    assert!(sf10 < sf7 && sf12 < sf10, "{sf7} / {sf10} / {sf12} dB");
    let total = sf7 - sf12;
    assert!(
        (1.0..3.0).contains(&total),
        "SF12 sits {total} dB below SF7, where the closed form predicts about 1.9"
    );
    // What it costs: a symbol 32 times as long for 12 bits instead of 7, so the same message takes
    // 18.7 times the air time. The sensitivity is bought with time, and the catalog says so.
    let air_time = f64::from(1 << 12) / f64::from(1 << 7) * 7.0 / 12.0;
    assert!((air_time - 18.667).abs() < 1e-2);
}

/// **Hopping costs nothing when the sequencer is known**, which is the framework's whole claim.
/// Measured against the row it carries rather than against a tolerance: the de-hop is the exact
/// inverse of the hop, so the two curves are the same curve.
#[test]
fn hopping_costs_nothing_over_the_entry_it_carries() {
    let cost = sensitivity(FHSS_AWGN) - sensitivity(BARKER11_AWGN);
    assert!(
        cost.abs() < 0.3,
        "hopping costs {cost} dB over the entry it carries"
    );
}

/// **What hopping buys, and the limit an uncoded link puts on it.**
///
/// A narrowband interferer parked on one channel reaches only the dwells that land there — `1/C`
/// of them, exactly — where the same interferer on an unhopped link reaches the whole burst. That
/// exposure is the framework's deliverable and it is asserted as a fraction, since it is a
/// property of the schedule and not a statistic.
///
/// **Turning it into link margin is what an uncoded chain cannot do**, and the committed tables
/// say so: both entries carry the same parked interferer at the same absolute frequency, and the
/// hopped one tolerates barely a fraction of a dB more. The arithmetic is unavoidable — with a
/// third of the dwells destroyed the average BER is already 0.17, far past the §4.3 failure
/// floor, so the hopping entry fails at very nearly the level that destroys those dwells at all.
/// What recovers the other two thirds is coding and interleaving *across hops*, which is channel
/// coding and lives beside the FEC in `sdrmm-dsp` rather than here (MODEM-PLAN §1.1) — the same
/// conclusion, and for the same reason, that phase 6 reached about a nulled subcarrier.
#[test]
fn a_parked_jammer_reaches_only_its_own_share_of_a_hopped_burst() {
    let sequence = hop_sequence(11);
    let hops = sequence.order().len();
    let demod = FhssDemod::new(sequence);
    for channel in 0..HOP_CHANNELS {
        let exposed = demod.dwells_on(channel, hops) as f64 / hops as f64;
        assert!(
            (exposed - 1.0 / HOP_CHANNELS as f64).abs() < 0.12,
            "a jammer on channel {channel} reaches {exposed} of the burst, where the plan's \
             {HOP_CHANNELS} channels predict {}",
            1.0 / HOP_CHANNELS as f64
        );
    }

    let parked = |stem: &str| {
        limits::load_json(&baseline_path(stem))
            .unwrap()
            .rows
            .iter()
            .find(|r| r.axis == "parked jammer J/C")
            .unwrap_or_else(|| panic!("{stem} carries no parked jammer row"))
            .threshold
    };
    let bought = parked(FHSS_LIMITS) - parked(DSSS_LIMITS);
    assert!(
        bought > -0.3,
        "hopping cost {bought} dB against a parked jammer, which it cannot do: the de-hop is the \
         hop's inverse, so the jammed dwells are the unhopped case and the rest are better"
    );
    assert!(
        bought < 1.5,
        "hopping now buys {bought} dB of margin against a parked jammer. An uncoded chain cannot: \
         losing 1/{HOP_CHANNELS} of the dwells already puts the average BER past the failure \
         floor. If this has changed, the entry has gained coding across hops and its docs are \
         wrong rather than this bound"
    );
}

// --- Level-1 E2E (§4.4) ------------------------------------------------------------------------

/// A short version of a committed chain, so a loopback is a property test rather than a sweep.
fn loopback_at_margin(mut link: Link, stem: &str, margin_db: f64, seed: u64) {
    let payloads = Payloads::new(seed, 8, link.bits_per_trial);
    let mut channel =
        channel_at_margin(&ChannelSpec::default(), &link, sensitivity(stem), margin_db);
    if let Err(mismatch) = loopback(&mut link, &mut channel, payloads) {
        panic!("{stem}: {mismatch}");
    }
}

#[test]
fn barker11_loops_back_clean_at_6db_margin() {
    loopback_at_margin(barker11_link(), BARKER11_AWGN, 6.0, 0x0_e5b1);
}

#[test]
fn barker11_qpsk_loops_back_clean_at_6db_margin() {
    loopback_at_margin(barker11_qpsk_link(), BARKER11_QPSK_AWGN, 6.0, 0x0_e5b2);
}

#[test]
fn m31_loops_back_clean_at_6db_margin() {
    loopback_at_margin(m31_link(), M31_AWGN, 6.0, 0x0_e5b3);
}

#[test]
fn cck_loops_back_clean_at_6db_margin() {
    loopback_at_margin(cck11_link(), CCK11_AWGN, 6.0, 0x0_e5c8);
    loopback_at_margin(cck55_link(), CCK55_AWGN, 6.0, 0x0_e5c4);
}

#[test]
fn css_loops_back_clean_at_6db_margin() {
    loopback_at_margin(css_link(7), CSS_SF7_AWGN, 6.0, 0x0_e5c7);
    loopback_at_margin(css_link(12), CSS_SF12_AWGN, 6.0, 0x0_e5cc);
}

/// The plan's own phase-7 acceptance, in its own words: "payload survives a hopping channel with
/// the sequencer known".
#[test]
fn the_payload_survives_a_hopping_channel_with_the_sequencer_known() {
    loopback_at_margin(fhss_link(), FHSS_AWGN, 6.0, 0x0_e5f4);
}

// --- Limits tables (§4.3) ----------------------------------------------------------------------

const PROBE_ERRORS: u64 = 120;
const PROBE_BITS: u64 = 30_000;

/// The narrowband-jammer row's own budget, and it is far larger than every other axis's for a
/// reason that is the definition rather than caution: **processing gain is an average over where
/// the jammer sits**, the offset is drawn once per trial, and a probe of fourteen trials measures
/// the worst of fourteen draws instead. Measured: at the standard budget the length-31 code read
/// only 0.55 dB better than Barker-11, where the two codes' despread jammer powers differ by the
/// full 4.2 dB (`spread::dsss`'s direct measurement). Two hundred draws is what makes the row a
/// property of the code.
const JAMMER_ERRORS: u64 = 400;
const JAMMER_BITS: u64 = 500_000;

/// The composite-profile rows sweep a whole curve twice, so their grid has to bracket the 1e-3
/// crossing of both the clean chain and the impaired one.
const DSSS_PROFILE_GRID: [f64; 6] = [5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
const CCK_PROFILE_GRID: [f64; 6] = [4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
const CSS_PROFILE_GRID: [f64; 6] = [2.0, 3.0, 4.0, 5.0, 6.0, 7.0];

/// Axis brackets, named because the gates read them: a row that reaches its bracket is an axis
/// the entry does not fail on, and saying so takes the bracket's own value.
const JAMMER_AXIS_DB: f64 = 40.0;

/// The jammer axis is searched from a floor rather than from zero, because CCK's rate costs it
/// enough interference tolerance to fail with the jammer already *below* the carrier — and a row
/// pinned at exactly 0.0 records nothing and cannot be guarded (a 20 % allowance on zero is zero).
const JAMMER_FLOOR_DB: f64 = 10.0;
const TIMING_AXIS_SAMPLES: f64 = 16.0;

fn probe(link: &Link, spec: &ChannelSpec, op_db: f64, seed: u64) -> f64 {
    limits::measure_ber(link, spec, op_db, seed, PROBE_ERRORS, PROBE_BITS)
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

/// The narrowband jammer row, and the axis convention behind it: the search finds the *largest*
/// value an entry survives, so the axis is stated as interference-over-carrier rather than the
/// carrier-over-interference a datasheet would print. Larger is better, which is what makes the
/// row directly comparable across spreading factors — and the comparison *is* the processing
/// gain.
///
/// The jammer's offset is drawn per realisation over the chain's own chip band, because that is
/// what processing gain is defined as: an average over where a narrowband interferer sits, not a
/// tone at one frequency (`ber::impair::interference` records what a fixed tone measures instead).
fn jammer_row(link: &Link, op_db: f64, seed: u64, half_band_cycles: f64) -> LimitRow {
    shifted_jammer_row("narrowband J/C", |jc_db| {
        limits::measure_ber(
            link,
            &ChannelSpec::default().cochannel(Interferer::narrowband(-jc_db, half_band_cycles)),
            op_db,
            seed,
            JAMMER_ERRORS,
            JAMMER_BITS,
        )
    })
}

/// [`axis_row`] over the shifted jammer axis: the search runs on `J/C + `[`JAMMER_FLOOR_DB`] so it
/// can start at zero as `search_axis_limit` requires, and the recorded threshold is shifted back
/// into the J/C a reader wants.
fn shifted_jammer_row(axis: &str, ber_at: impl Fn(f64) -> f64) -> LimitRow {
    let mut row = axis_row(axis, "dB", JAMMER_AXIS_DB, 0.1, |shifted| {
        ber_at(shifted - JAMMER_FLOOR_DB)
    });
    row.threshold -= JAMMER_FLOOR_DB;
    row
}

/// The direct-sequence and CCK axes — one function, because the two entries share their chip
/// rate, their chip pulse and their receiver's shape, so a difference between their tables is the
/// block code.
fn chip_domain_rows(link: &Link, op_db: f64, seed: u64, profile_grid: &[f64]) -> Vec<LimitRow> {
    vec![
        // A burst is read through one fitted phase, so what a carrier offset costs is the turn it
        // accumulates across the payload — the entry's own statement about its geometry, and in
        // family with the linear engine's coherent rows for the same reason.
        axis_row("static CFO", "Hz", 20_000.0, 5.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, CHIP_SAMPLE_RATE)),
                op_db,
                seed,
            )
        }),
        // One phase for a 2.1 ms burst, so a drift's cost is the offset it has walked by the last
        // symbol — the CFO row divided by the burst duration, which is where the bracket comes
        // from.
        axis_row("frequency drift", "Hz/s", 1e6, 200.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, CHIP_SAMPLE_RATE)),
                op_db,
                seed,
            )
        }),
        // The clock row is the burst search's: one whole-sample origin for the whole burst, so a
        // drifting clock walks the chip instants off the correlator's grid.
        axis_row("sample clock", "ppm", 200.0, 0.5, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        // The search covers whole samples only, so a fractional offset is the residual this row
        // measures — a quarter of a chip is a quarter of a sample here.
        axis_row(
            "static timing offset",
            "samples",
            TIMING_AXIS_SAMPLES,
            0.05,
            |d| {
                probe(
                    link,
                    &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                    op_db,
                    seed,
                )
            },
        ),
        jammer_row(link, op_db, seed, 0.5 / CHIP_SPS as f64),
        axis_row("IQ gain", "dB", 6.0, 0.05, |db| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(db, 0.0)),
                op_db,
                seed,
            )
        }),
        axis_row("IQ phase", "deg", 30.0, 0.25, |deg| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(0.0, deg)),
                op_db,
                seed,
            )
        }),
        // The hop plan's centre channel is baseband, which is where this entry always sits — so
        // this row is the same interferer the hopping table prices, reaching all of this burst
        // and a third of a hopped one.
        shifted_jammer_row("parked jammer J/C", |jc_db| {
            limits::measure_ber(
                link,
                &ChannelSpec::default().cochannel(Interferer::parked(
                    -jc_db,
                    hop_sequence(11).offset_cycles(HOP_CHANNELS / 2),
                )),
                op_db,
                seed,
                JAMMER_ERRORS,
                JAMMER_BITS,
            )
        }),
        limits::measure_profile_degradation(
            link,
            &ChannelSpec::default(),
            CompositeProfile::StaticIndoor,
            profile_grid,
            seed ^ 0x51de,
            PROBE_ERRORS,
            600_000,
        ),
    ]
}

/// The chirp axes. Only the carrier row differs in *kind* from the chip-domain ones, and it is the
/// entry's most interesting number: a chirp turns a frequency offset into a timing shift, the
/// preamble-bin estimate absorbs both together, and the payload is read through the same combined
/// correction — so this row lands orders above every other entry's.
fn css_rows(link: &Link, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![
        axis_row("static CFO", "Hz", CSS_BANDWIDTH * 0.5, 200.0, |hz| {
            probe(
                link,
                &ChannelSpec::default().cfo(Cfo::from_hz(hz, CSS_BANDWIDTH)),
                op_db,
                seed,
            )
        }),
        axis_row("frequency drift", "Hz/s", 1e6, 100.0, |hz_s| {
            probe(
                link,
                &ChannelSpec::default().drift(Drift::from_hz_per_s(hz_s, CSS_BANDWIDTH)),
                op_db,
                seed,
            )
        }),
        axis_row("sample clock", "ppm", 200.0, 0.5, |ppm| {
            probe(
                link,
                &ChannelSpec::default().clock(ClockError::new(ppm)),
                op_db,
                seed,
            )
        }),
        axis_row(
            "static timing offset",
            "samples",
            TIMING_AXIS_SAMPLES,
            0.05,
            |d| {
                probe(
                    link,
                    &ChannelSpec::default().timing_offset(TimingOffset::new(d)),
                    op_db,
                    seed,
                )
            },
        ),
        // A noncoherent detector reads magnitudes, so the front-end axes that ruin a coherent
        // entry cost it very little — the same structural result the PPM envelope tier recorded.
        axis_row("IQ gain", "dB", 6.0, 0.05, |db| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(db, 0.0)),
                op_db,
                seed,
            )
        }),
        axis_row("IQ phase", "deg", 30.0, 0.25, |deg| {
            probe(
                link,
                &ChannelSpec::default().iq_imbalance(IqImbalance::new(0.0, deg)),
                op_db,
                seed,
            )
        }),
        limits::measure_profile_degradation(
            link,
            &ChannelSpec::default(),
            CompositeProfile::StaticIndoor,
            &CSS_PROFILE_GRID,
            seed ^ 0x51de,
            PROBE_ERRORS,
            600_000,
        ),
    ]
}

/// The two focused tables: a spreading factor's jammer row alone (the m31 entry exists for the
/// processing-gain comparison and nothing else) and the hopping entry's parked-jammer row.
fn jammer_only_rows(link: &Link, op_db: f64, seed: u64) -> Vec<LimitRow> {
    vec![jammer_row(link, op_db, seed, 0.5 / CHIP_SPS as f64)]
}

fn hopping_rows(link: &Link, op_db: f64, seed: u64) -> Vec<LimitRow> {
    let sequence = hop_sequence(11);
    // A jammer parked on the plan's centre channel — which is baseband, where the unhopped entry
    // always sits, so both tables price the *same* interferer at the same absolute frequency.
    let offset = sequence.offset_cycles(HOP_CHANNELS / 2);
    vec![
        shifted_jammer_row("parked jammer J/C", |jc_db| {
            limits::measure_ber(
                link,
                &ChannelSpec::default().cochannel(Interferer::parked(-jc_db, offset)),
                op_db,
                seed,
                JAMMER_ERRORS,
                JAMMER_BITS,
            )
        }),
        jammer_row(link, op_db, seed, 0.5 / CHIP_SPS as f64),
    ]
}

fn assert_table_matches(stem: &str, rows: Vec<LimitRow>) {
    let committed = limits::load_json(&baseline_path(stem)).unwrap();
    let mut faults = Vec::new();
    for row in &committed.rows {
        let Some(m) = rows.iter().find(|m| m.axis == row.axis) else {
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
        if m.threshold.is_nan() || worse_by > 0.2 * row.threshold.abs().max(0.5) {
            faults.push(format!(
                "row '{}': committed {} -> measured {} {}",
                row.axis, row.threshold, m.threshold, m.unit
            ));
        }
    }
    assert!(faults.is_empty(), "{stem} regressions: {faults:#?}");
}

fn operating_point(stem: &str) -> f64 {
    limits::load_json(&baseline_path(stem))
        .unwrap()
        .operating_point_db()
        .expect("the committed table carries a 1e-3 sensitivity")
}

#[test]
fn dsss_limits_rows_match_committed_table() {
    let op = operating_point(DSSS_LIMITS);
    assert_table_matches(
        DSSS_LIMITS,
        chip_domain_rows(
            &barker11_link(),
            op,
            BARKER11_SEED ^ 0x11e5,
            &DSSS_PROFILE_GRID,
        ),
    );
}

#[test]
fn m31_limits_rows_match_committed_table() {
    let op = operating_point(M31_LIMITS);
    assert_table_matches(
        M31_LIMITS,
        jammer_only_rows(&m31_link(), op, M31_SEED ^ 0x11e5),
    );
}

#[test]
fn cck_limits_rows_match_committed_table() {
    let op = operating_point(CCK_LIMITS);
    assert_table_matches(
        CCK_LIMITS,
        chip_domain_rows(&cck11_link(), op, CCK11_SEED ^ 0x11e5, &CCK_PROFILE_GRID),
    );
}

#[test]
fn css_limits_rows_match_committed_table() {
    let op = operating_point(CSS_LIMITS);
    assert_table_matches(
        CSS_LIMITS,
        css_rows(&css_link(7), op, CSS_SF7_SEED ^ 0x11e5),
    );
}

#[test]
fn fhss_limits_rows_match_committed_table() {
    let op = operating_point(FHSS_LIMITS);
    assert_table_matches(
        FHSS_LIMITS,
        hopping_rows(&fhss_link(), op, FHSS_SEED ^ 0x11e5),
    );
}

/// The chirp entry's carrier row read against its own explanation: a chirp cannot tell a delay
/// from a frequency offset, so the preamble-bin estimate removes both together and the entry
/// tolerates a carrier offset a fraction of its own bandwidth wide — orders past what the
/// chip-domain entries beside it manage, and a property of the waveform rather than of this
/// implementation.
#[test]
fn the_chirp_entry_absorbs_a_carrier_offset_the_chip_entries_cannot() {
    let row = |stem: &str, axis: &str| {
        limits::load_json(&baseline_path(stem))
            .unwrap()
            .rows
            .iter()
            .find(|r| r.axis == axis)
            .unwrap_or_else(|| panic!("{stem} carries no '{axis}' row"))
            .threshold
    };
    let chirp = row(CSS_LIMITS, "static CFO");
    let chips = row(DSSS_LIMITS, "static CFO");
    // Stated relative to each entry's own occupied bandwidth, which is the only comparison the
    // two geometries admit: 125 kHz against 11 MHz.
    let chirp_fraction = chirp / CSS_BANDWIDTH;
    let chip_fraction = chips / CHIP_SAMPLE_RATE;
    assert!(
        chirp_fraction > 100.0 * chip_fraction,
        "the chirp row is {chirp_fraction} of its bandwidth against the chip row's {chip_fraction}"
    );
}

// --- Full re-measurement (nightly; regenerates the committed artifacts) ------------------------

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
    for (name, link, grid, seed, stem) in ROWS {
        println!("--- {name}");
        remeasure_curve(&link(), grid, *seed, stem);
    }
}

fn measure_table_full(
    stem: &str,
    entry: &str,
    link: &Link,
    grid: &[f64],
    seed: u64,
    rows: impl Fn(f64) -> Vec<LimitRow>,
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
    table.rows = rows(op_db);
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
#[ignore = "full limits run; run in release to (re)generate the committed tables"]
fn measure_all_limits_full() {
    let barker = barker11_link();
    measure_table_full(
        DSSS_LIMITS,
        "dsss-barker11-bpsk",
        &barker,
        BARKER11_GRID,
        BARKER11_SEED,
        |op| chip_domain_rows(&barker, op, BARKER11_SEED ^ 0x11e5, &DSSS_PROFILE_GRID),
    );
    let m31 = m31_link();
    measure_table_full(
        M31_LIMITS,
        "dsss-m31-bpsk",
        &m31,
        M31_GRID,
        M31_SEED,
        |op| jammer_only_rows(&m31, op, M31_SEED ^ 0x11e5),
    );
    let cck = cck11_link();
    measure_table_full(CCK_LIMITS, "cck-8bit", &cck, CCK11_GRID, CCK11_SEED, |op| {
        chip_domain_rows(&cck, op, CCK11_SEED ^ 0x11e5, &CCK_PROFILE_GRID)
    });
    let css = css_link(7);
    measure_table_full(
        CSS_LIMITS,
        "css-sf7",
        &css,
        CSS_SF7_GRID,
        CSS_SF7_SEED,
        |op| css_rows(&css, op, CSS_SF7_SEED ^ 0x11e5),
    );
    let fhss = fhss_link();
    measure_table_full(
        FHSS_LIMITS,
        "fhss-barker11",
        &fhss,
        FHSS_GRID,
        FHSS_SEED,
        |op| hopping_rows(&fhss, op, FHSS_SEED ^ 0x11e5),
    );
}

#[test]
#[ignore = "prints the committed numbers this phase's catalog rows quote; asserts nothing"]
fn print_catalog_numbers() {
    println!(
        "framing overhead: direct sequence {:.4} dB ({PREAMBLE} + {DSSS_PAYLOAD} symbols), \
         chirp {:.4} dB ({CSS_PREAMBLE} + {} symbols)",
        dsss_overhead_db(),
        css_overhead_db(),
        css_payload(7)
    );
    for (stem, grid, oracle) in oracle_rows() {
        let curve = load_curve(stem);
        let worst = sweep::worst_penalty_db(&curve, &oracle, grid[0], *grid.last().unwrap());
        println!(
            "{stem}: 1e-3 at {:.2} dB, worst penalty {worst:+.3} dB",
            sensitivity(stem)
        );
    }
    for stem in [CCK11_AWGN, CCK55_AWGN] {
        println!(
            "{stem}: 1e-3 at {:.2} dB (commit-and-guard)",
            sensitivity(stem)
        );
    }
    println!(
        "cck 8-bit is {:+.2} dB ahead of barker-11; hopping costs {:+.2} dB; SF7 -> SF12 buys \
         {:+.2} dB",
        sensitivity(BARKER11_AWGN) - sensitivity(CCK11_AWGN),
        sensitivity(FHSS_AWGN) - sensitivity(BARKER11_AWGN),
        sensitivity(CSS_SF7_AWGN) - sensitivity(CSS_SF12_AWGN),
    );
    for (name, stem) in [
        ("dsss barker-11", DSSS_LIMITS),
        ("dsss m31", M31_LIMITS),
        ("cck 8-bit", CCK_LIMITS),
        ("css sf7", CSS_LIMITS),
        ("fhss", FHSS_LIMITS),
    ] {
        let table = limits::load_json(&baseline_path(stem)).unwrap();
        println!(
            "--- {name}: sensitivity {:?} / {:?} / {:?} dB",
            table.sensitivity_db_1e2, table.sensitivity_db_1e3, table.sensitivity_db_1e4
        );
        for row in &table.rows {
            println!("    {:<24} {:>12.4} {}", row.axis, row.threshold, row.unit);
        }
    }
}
