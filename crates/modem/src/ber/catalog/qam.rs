//! The quadrature-amplitude catalog entries ( §6, linear rows 6–10): square QAM from
//! 16 to 1024, cross-QAM 32 and 128, star-QAM, DVB-T hierarchical QAM and DVB-S2 APSK.
//!
//! **These five rows are the same code.** That is not a remark, it is the acceptance: §3.3 says a
//! code path that special-cases "the" constellation is a defect, and the way to prove it is to
//! measure five geometries — a square grid, a grid with its corners removed, concentric rings,
//! warped rails, and unequal ring populations — through one modulator, one demodulator and one
//! demapper. Every difference between the rows below is a [`Constellation`](crate::constellation::Constellation)
//! and a loop bandwidth.
//!
//! **The loop bandwidth is per-entry data and it is measured.** A decision-directed loop is only
//! as good as its decisions, and a denser table's are worse: at the substrate's wide 0.01 the
//! 256-QAM chain's waterfall walls at 3e-2, and at 0.001 it sits on theory. The rule the rows
//! encode is one narrowing step per two bits.
//!
//! **The four exotic rows have no closed form**, and their reference is
//! [`theory::NearestNeighbour`] read off the table itself: minimum distance, neighbour count and
//! the labelling's own measured bits-per-error. It reproduces the closed forms it generalises
//! within 2 % by 15 dB (asserted in `theory`), which makes it a real acceptance rather than a
//! curve compared against itself — and the committed artifact guards the rest.

use std::sync::LazyLock;

use crate::{
    ber::{
        catalog::{
            Measurement, Reference, Tier,
            linear::{
                FULL_CAP, coherent_link, coherent_tracked_link, differential_link, params, table,
            },
        },
        sweep::Link,
        theory::{self, NearestNeighbour},
    },
    constellation::{Constellation, ConstellationError, tables},
    linear::{CarrierLoop, LinearTiming, PhaseDetector},
};

/// Loop bandwidths by table density, one narrowing step per two bits (see the module docs).
pub const LOOP_BW_16: f64 = 0.003;
pub const LOOP_BW_64: f64 = 0.003;
pub const LOOP_BW_256: f64 = 0.001;
pub const LOOP_BW_1024: f64 = 0.0003;

fn square_link(name: &str, m: u32, loop_bw: f64) -> Link {
    coherent_link(
        &format!(
            "{name} uncoded, coherent tier: LinearMod -> feedforward timing -> decision-directed \
             Costas (bw {loop_bw}) -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud"
        ),
        params(tables::qam_square(m), 0.0, false),
        move || Some(CarrierLoop::new(PhaseDetector::DecisionDirected, loop_bw)),
    )
}

#[must_use]
pub fn qam16_link() -> Link {
    square_link("16-qam", 16, LOOP_BW_16)
}

#[must_use]
pub fn qam64_link() -> Link {
    square_link("64-qam", 64, LOOP_BW_64)
}

#[must_use]
pub fn qam256_link() -> Link {
    square_link("256-qam", 256, LOOP_BW_256)
}

#[must_use]
pub fn qam1024_link() -> Link {
    square_link("1024-qam", 1024, LOOP_BW_1024)
}

/// 16-QAM on the **tracking** timing tier — the comparison that says what the feedforward
/// estimator is worth on a burst (§5 item 2). Everything else about the chain is identical.
#[must_use]
pub fn qam16_tracked_link() -> Link {
    coherent_tracked_link(
        "16-qam uncoded, tracking timing tier: LinearMod -> SymbolSync (bw 0.005) -> \
         decision-directed Costas -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::qam_square(16), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::DecisionDirected,
                LOOP_BW_16,
            ))
        },
        LinearTiming {
            timing_bw: 0.005,
            power_symbols: 1_000.0,
        },
    )
}

// --- The exotic geometries ----------------------------------------------------------------------

fn exotic_link(name: &str, table: Result<Constellation, ConstellationError>, loop_bw: f64) -> Link {
    coherent_link(
        &format!(
            "{name} uncoded, coherent tier: LinearMod -> feedforward timing -> decision-directed \
             Costas (bw {loop_bw}) -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud"
        ),
        params(table, 0.0, false),
        move || Some(CarrierLoop::new(PhaseDetector::DecisionDirected, loop_bw)),
    )
}

/// Cross-QAM 32 — DVB-C's odd-bit order, five bits on a grid with its corners cut away.
#[must_use]
pub fn cross32_link() -> Link {
    exotic_link("32-cross-qam", tables::qam_cross(32), LOOP_BW_16)
}

/// Cross-QAM 128.
#[must_use]
pub fn cross128_link() -> Link {
    exotic_link("128-cross-qam", tables::qam_cross(128), LOOP_BW_256)
}

/// The star-QAM ring ratio. 2.0 is the classic two-ring 16-star, the geometry chosen so amplitude
/// and phase separate cleanly enough to be detected differentially.
pub const STAR_RADII: &[f64] = &[1.0, 2.0];

/// Points per ring — the phase alphabet the differential rule accumulates in, while the ring
/// index rides absolutely (see [`differential_encode`](super::linear)).
pub const STAR_PHASES: u32 = 8;

/// The star table: two rings of eight, amplitude bit in the high position, phase bits Gray around
/// each ring.
pub fn star16_table() -> Result<Constellation, ConstellationError> {
    tables::qam_star(STAR_RADII, STAR_PHASES)
}

/// Star-QAM 16, coherent tier.
#[must_use]
pub fn star16_link() -> Link {
    exotic_link("16-star-qam", star16_table(), LOOP_BW_16)
}

/// Star-QAM 16, differential tier — the reason the geometry exists. The phase bits ride the
/// difference between consecutive symbols, so the receiver needs no absolute carrier phase; the
/// amplitude bit is read from the product's own radius, which
/// [`differential_detect`](crate::linear::differential_detect) preserves by dividing out the
/// reference symbol's magnitude rather than squaring it.
#[must_use]
pub fn star16_differential_link() -> Link {
    differential_link(
        "16-star-qam uncoded, differential tier: differentially encoded indices -> LinearMod -> \
         feedforward timing -> open loop -> y·conj(y₋₁)/|y₋₁|, RRC α=0.35 span 8, 8 sps, \
         48 kHz 6000 baud",
        params(star16_table(), 0.0, false),
        table("16-star-qam differences", star16_table()),
        STAR_PHASES,
    )
}

/// DVB-T's hierarchical α — the ratio of the distance between points carrying different
/// high-priority bits to the distance within a quadrant. 2.0 is one of the spec's three values
/// and the catalog's reference configuration.
pub const HIERARCHICAL_ALPHA: f64 = 2.0;

/// Non-uniform (hierarchical) 16-QAM at α = 2: the same labelling as uniform 16-QAM on warped
/// rails, which buys the two high-priority bits distance at the low-priority pair's expense.
#[must_use]
pub fn hierarchical16_link() -> Link {
    exotic_link(
        "16-qam hierarchical α=2",
        tables::qam_hierarchical(16, HIERARCHICAL_ALPHA),
        LOOP_BW_16,
    )
}

/// DVB-S2 ring ratios at code rate 3/4 (EN 302 307-1 tables 9 and 10).
pub const APSK16_GAMMA: f64 = 3.15;
pub const APSK32_GAMMA1: f64 = 2.84;
pub const APSK32_GAMMA2: f64 = 5.27;

/// DVB-S2 16-APSK: 4 + 12 points, coherent tier.
#[must_use]
pub fn apsk16_link() -> Link {
    exotic_link(
        "16-apsk (DVB-S2 γ=3.15)",
        tables::apsk16_dvbs2(APSK16_GAMMA),
        LOOP_BW_16,
    )
}

/// DVB-S2 32-APSK: 4 + 12 + 16 points.
#[must_use]
pub fn apsk32_link() -> Link {
    exotic_link(
        "32-apsk (DVB-S2 γ₁=2.84 γ₂=5.27)",
        tables::apsk32_dvbs2(APSK32_GAMMA1, APSK32_GAMMA2),
        LOOP_BW_256,
    )
}

// --- Table-driven oracles -------------------------------------------------------------------------
//
// The exotic geometries have no closed form, so their reference is read off the table. Each is a
// `LazyLock` because `Reference::Oracle` takes a plain `fn` pointer — deliberately, so a
// reference cannot capture mutable state — and the geometry only has to be measured once.

macro_rules! table_oracle {
    ($lazy:ident, $ber:ident, $table:expr) => {
        static $lazy: LazyLock<NearestNeighbour> = LazyLock::new(|| NearestNeighbour::of(&$table));

        fn $ber(ebn0_db: f64) -> f64 {
            $lazy.ber(ebn0_db)
        }
    };
}

table_oracle!(
    CROSS32_NN,
    cross32_ber,
    table("32-cross-qam", tables::qam_cross(32))
);
table_oracle!(
    CROSS128_NN,
    cross128_ber,
    table("128-cross-qam", tables::qam_cross(128))
);
table_oracle!(STAR16_NN, star16_ber, table("16-star-qam", star16_table()));
table_oracle!(
    HIER16_NN,
    hierarchical16_ber,
    table(
        "16-qam hierarchical",
        tables::qam_hierarchical(16, HIERARCHICAL_ALPHA)
    )
);
table_oracle!(
    APSK16_NN,
    apsk16_ber,
    table("16-apsk", tables::apsk16_dvbs2(APSK16_GAMMA))
);
table_oracle!(
    APSK32_NN,
    apsk32_ber,
    table(
        "32-apsk",
        tables::apsk32_dvbs2(APSK32_GAMMA1, APSK32_GAMMA2)
    )
);

fn qam16_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(16, ebn0_db)
}

fn qam64_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(64, ebn0_db)
}

fn qam256_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(256, ebn0_db)
}

fn qam1024_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(1024, ebn0_db)
}

// --- Committed sweep parameters ----------------------------------------------------------------

pub const QAM16_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0];
pub const QAM64_GRID: &[f64] = &[12.0, 13.0, 14.0, 15.0, 16.0, 17.0];
pub const QAM256_GRID: &[f64] = &[16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0];
pub const QAM1024_GRID: &[f64] = &[20.0, 21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 27.0];
pub const CROSS32_GRID: &[f64] = &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
pub const CROSS128_GRID: &[f64] = &[15.0, 16.0, 17.0, 18.0, 19.0];
pub const STAR16_GRID: &[f64] = &[11.0, 12.0, 13.0, 14.0, 15.0];
pub const STAR16_DIFF_GRID: &[f64] = &[13.0, 14.0, 15.0, 16.0, 17.0];
pub const HIER16_GRID: &[f64] = &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0];
pub const APSK16_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0];
pub const APSK32_GRID: &[f64] = &[13.0, 14.0, 15.0, 16.0];

pub const QAM16_AWGN: &str = "linear/qam16_awgn";
pub const QAM16_TRACKED_AWGN: &str = "linear/qam16_tracked_awgn";
pub const QAM64_AWGN: &str = "linear/qam64_awgn";
pub const QAM256_AWGN: &str = "linear/qam256_awgn";
pub const QAM1024_AWGN: &str = "linear/qam1024_awgn";
pub const CROSS32_AWGN: &str = "linear/cross32_awgn";
pub const CROSS128_AWGN: &str = "linear/cross128_awgn";
pub const STAR16_AWGN: &str = "linear/star16_awgn";
pub const STAR16_DIFF_AWGN: &str = "linear/star16_differential_awgn";
pub const HIER16_AWGN: &str = "linear/hierarchical16_awgn";
pub const APSK16_AWGN: &str = "linear/apsk16_awgn";
pub const APSK32_AWGN: &str = "linear/apsk32_awgn";
pub const QAM_LIMITS: &str = "linear/qam16_limits";
pub const QAM_PERF: &str = "linear/qam_perf";

/// Same tolerance and the same reasons as the PSK rows carry.
pub const ORACLE_TOLERANCE_DB: f64 = 0.75;

/// The table-driven bound is a truncated union bound, so it is an approximation at the shoulder
/// and asymptotically exact in the tail — a looser tolerance than the closed forms get, and one
/// that has to hold across the whole grid.
pub const NEAREST_NEIGHBOUR_TOLERANCE_DB: f64 = 1.25;

const fn oracle_row(
    stem: &'static str,
    link: fn() -> Link,
    grid: &'static [f64],
    seed: u64,
    name: &'static str,
    ber: fn(f64) -> f64,
    tolerance_db: f64,
) -> Measurement {
    Measurement {
        stem,
        link,
        full: Tier {
            grid,
            seed,
            min_errors: super::FULL_ERRORS,
            max_trial_bits: FULL_CAP,
        },
        smoke_points: super::SMOKE_POINTS,
        reference: Reference::Oracle {
            name,
            ber,
            tolerance_db,
        },
    }
}

/// Square QAM, plus the timing-tier comparison at M = 16.
pub const SQUARE: &[Measurement] = &[
    oracle_row(
        QAM16_AWGN,
        qam16_link,
        QAM16_GRID,
        0x9a16,
        "Gray square 16-QAM",
        qam16_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        QAM64_AWGN,
        qam64_link,
        QAM64_GRID,
        0x9a64,
        "Gray square 64-QAM",
        qam64_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        QAM256_AWGN,
        qam256_link,
        QAM256_GRID,
        0x9a25,
        "Gray square 256-QAM",
        qam256_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        QAM1024_AWGN,
        qam1024_link,
        QAM1024_GRID,
        0x9a10,
        "Gray square 1024-QAM",
        qam1024_ber,
        ORACLE_TOLERANCE_DB,
    ),
    Measurement::committed(
        QAM16_TRACKED_AWGN,
        qam16_tracked_link,
        QAM16_GRID,
        0x9a17,
        FULL_CAP,
    ),
];

/// Cross-QAM.
pub const CROSS: &[Measurement] = &[
    oracle_row(
        CROSS32_AWGN,
        cross32_link,
        CROSS32_GRID,
        0xc032,
        "table-driven nearest-neighbour bound",
        cross32_ber,
        NEAREST_NEIGHBOUR_TOLERANCE_DB,
    ),
    oracle_row(
        CROSS128_AWGN,
        cross128_link,
        CROSS128_GRID,
        0xc128,
        "table-driven nearest-neighbour bound",
        cross128_ber,
        NEAREST_NEIGHBOUR_TOLERANCE_DB,
    ),
];

/// Star-QAM, both tiers.
pub const STAR: &[Measurement] = &[
    oracle_row(
        STAR16_AWGN,
        star16_link,
        STAR16_GRID,
        0x57a1,
        "table-driven nearest-neighbour bound",
        star16_ber,
        NEAREST_NEIGHBOUR_TOLERANCE_DB,
    ),
    Measurement::committed(
        STAR16_DIFF_AWGN,
        star16_differential_link,
        STAR16_DIFF_GRID,
        0x57a2,
        FULL_CAP,
    ),
];

/// Non-uniform / hierarchical QAM.
pub const HIERARCHICAL: &[Measurement] = &[oracle_row(
    HIER16_AWGN,
    hierarchical16_link,
    HIER16_GRID,
    0x81e2,
    "table-driven nearest-neighbour bound",
    hierarchical16_ber,
    NEAREST_NEIGHBOUR_TOLERANCE_DB,
)];

/// APSK.
pub const APSK: &[Measurement] = &[
    oracle_row(
        APSK16_AWGN,
        apsk16_link,
        APSK16_GRID,
        0xa516,
        "table-driven nearest-neighbour bound",
        apsk16_ber,
        NEAREST_NEIGHBOUR_TOLERANCE_DB,
    ),
    oracle_row(
        APSK32_AWGN,
        apsk32_link,
        APSK32_GRID,
        0xa532,
        "table-driven nearest-neighbour bound",
        apsk32_ber,
        NEAREST_NEIGHBOUR_TOLERANCE_DB,
    ),
];
