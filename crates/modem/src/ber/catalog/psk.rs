use crate::{
    ber::{
        catalog::{
            Measurement, Reference, Tier,
            linear::{
                FULL_CAP, coherent_differential_link, coherent_link, differential_link, params,
                table,
            },
        },
        sweep::Link,
        theory,
    },
    constellation::tables,
    linear::{CarrierLoop, PhaseDetector},
};

pub const PSK_LOOP_BW: f64 = 0.003;

#[must_use]
pub fn bpsk_link() -> Link {
    coherent_link(
        "bpsk uncoded, LinearMod -> feedforward timing -> 2nd-power Costas -> unique-word anchor, \
         RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud, 64+32+16 symbol overhead in Eb",
        params(tables::pam(2), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 2 },
                PSK_LOOP_BW,
            ))
        },
    )
}

#[must_use]
pub fn qpsk_link() -> Link {
    coherent_link(
        "qpsk uncoded, LinearMod -> feedforward timing -> 4th-power Costas -> unique-word anchor, \
         RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud, 64+32+16 symbol overhead in Eb",
        params(tables::qam_square(4), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 4 },
                PSK_LOOP_BW,
            ))
        },
    )
}

#[must_use]
pub fn psk8_link() -> Link {
    coherent_link(
        "8-psk uncoded, LinearMod -> feedforward timing -> 8th-power Costas -> unique-word anchor, \
         RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud, 64+32+16 symbol overhead in Eb",
        params(tables::psk(8), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 8 },
                PSK_LOOP_BW,
            ))
        },
    )
}

#[must_use]
pub fn oqpsk_link() -> Link {
    coherent_link(
        "oqpsk uncoded, staggered LinearMod -> unstagger -> feedforward timing -> 4th-power \
         Costas -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::qam_square(4), 0.0, true),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 4 },
                PSK_LOOP_BW,
            ))
        },
    )
}

#[must_use]
pub fn pi2_bpsk_link() -> Link {
    coherent_link(
        "π/2-bpsk uncoded, rotated LinearMod -> feedforward timing -> de-rotate -> 2nd-power \
         Costas -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::pam(2), tables::PI_2_ROTATION, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 4 },
                PSK_LOOP_BW,
            ))
        },
    )
}

fn dpsk_link(name: &str, m: u32) -> Link {
    differential_link(
        &format!(
            "{name} uncoded, differentially encoded indices -> LinearMod -> feedforward timing -> \
             open loop -> y·conj(y₋₁) -> unique-word anchor on the differences, RRC α=0.35 span 8, \
             8 sps, 48 kHz 6000 baud"
        ),
        params(tables::psk(m), 0.0, false),
        table("m-psk differences", tables::psk(m)),
        m,
    )
}

#[must_use]
pub fn dbpsk_link() -> Link {
    dpsk_link("dbpsk", 2)
}

#[must_use]
pub fn dqpsk_link() -> Link {
    dpsk_link("dqpsk", 4)
}

#[must_use]
pub fn dpsk8_link() -> Link {
    dpsk_link("8dpsk", 8)
}

#[must_use]
pub fn pi4_dqpsk_link() -> Link {
    differential_link(
        "π/4-dqpsk uncoded, differentially encoded indices + π/4 per symbol -> LinearMod -> \
         feedforward timing -> de-rotate -> open loop -> y·conj(y₋₁), RRC α=0.35 span 8, 8 sps, \
         48 kHz 6000 baud",
        params(tables::psk(4), tables::PI_4_ROTATION, false),
        table("π/4-dqpsk differences", tables::psk(4)),
        4,
    )
}

#[must_use]
pub fn pi4_dqpsk_coherent_link() -> Link {
    coherent_differential_link(
        "π/4-dqpsk uncoded, coherent tier: 4th-power Costas -> unique-word anchor -> slice -> \
         differential decode of the indices, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::psk(4), tables::PI_4_ROTATION, false),
        4,
        || {
            Some(CarrierLoop::new(
                PhaseDetector::MthPower { m: 8 },
                PSK_LOOP_BW,
            ))
        },
    )
}

pub const BPSK_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
pub const QPSK_GRID: &[f64] = BPSK_GRID;
pub const PSK8_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const DBPSK_GRID: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
pub const DQPSK_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0];
pub const DPSK8_GRID: &[f64] = &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0];

pub const BPSK_AWGN: &str = "linear/bpsk_awgn";
pub const QPSK_AWGN: &str = "linear/qpsk_awgn";
pub const PSK8_AWGN: &str = "linear/psk8_awgn";
pub const OQPSK_AWGN: &str = "linear/oqpsk_awgn";
pub const PI2_BPSK_AWGN: &str = "linear/pi2_bpsk_awgn";
pub const DBPSK_AWGN: &str = "linear/dbpsk_awgn";
pub const DQPSK_AWGN: &str = "linear/dqpsk_awgn";
pub const DPSK8_AWGN: &str = "linear/dpsk8_awgn";
pub const PI4_DQPSK_AWGN: &str = "linear/pi4_dqpsk_awgn";
pub const PI4_DQPSK_COHERENT_AWGN: &str = "linear/pi4_dqpsk_coherent_awgn";
pub const PSK_LIMITS: &str = "linear/qpsk_limits";
pub const PSK_PERF: &str = "linear/psk_perf";

pub const ORACLE_TOLERANCE_DB: f64 = 0.75;

pub const DIFFERENTIAL_TOLERANCE_DB: f64 = 0.75;

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

fn psk8_ber(ebn0_db: f64) -> f64 {
    theory::mpsk_ser(8, ebn0_db) / 3.0
}

pub const COHERENT: &[Measurement] = &[
    oracle_row(
        BPSK_AWGN,
        bpsk_link,
        BPSK_GRID,
        0x0b95,
        "exact ½·erfc(√γ)",
        theory::bpsk_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        QPSK_AWGN,
        qpsk_link,
        QPSK_GRID,
        0x9b53,
        "exact Gray QPSK",
        theory::qpsk_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        PSK8_AWGN,
        psk8_link,
        PSK8_GRID,
        0x8b5c,
        "nearest-boundary 8-PSK SER / 3",
        psk8_ber,
        ORACLE_TOLERANCE_DB,
    ),
];

pub const OFFSET: &[Measurement] = &[
    oracle_row(
        OQPSK_AWGN,
        oqpsk_link,
        QPSK_GRID,
        0x0952,
        "exact Gray QPSK",
        theory::qpsk_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        PI2_BPSK_AWGN,
        pi2_bpsk_link,
        BPSK_GRID,
        0x9120,
        "exact ½·erfc(√γ)",
        theory::bpsk_ber,
        ORACLE_TOLERANCE_DB,
    ),
];

pub const DIFFERENTIAL: &[Measurement] = &[
    oracle_row(
        DBPSK_AWGN,
        dbpsk_link,
        DBPSK_GRID,
        0xdb95,
        "exact ½·e^{−γ}",
        theory::dbpsk_ber,
        DIFFERENTIAL_TOLERANCE_DB,
    ),
    oracle_row(
        DQPSK_AWGN,
        dqpsk_link,
        DQPSK_GRID,
        0xd9b5,
        "exact Marcum-Q DQPSK",
        theory::dqpsk_ber,
        DIFFERENTIAL_TOLERANCE_DB,
    ),
    Measurement::committed(DPSK8_AWGN, dpsk8_link, DPSK8_GRID, 0xd8b5, FULL_CAP),
];

pub const PI4: &[Measurement] = &[
    oracle_row(
        PI4_DQPSK_AWGN,
        pi4_dqpsk_link,
        DQPSK_GRID,
        0x9143,
        "exact Marcum-Q DQPSK",
        theory::dqpsk_ber,
        DIFFERENTIAL_TOLERANCE_DB,
    ),
    Measurement::committed(
        PI4_DQPSK_COHERENT_AWGN,
        pi4_dqpsk_coherent_link,
        QPSK_GRID,
        0x9144,
        FULL_CAP,
    ),
];
