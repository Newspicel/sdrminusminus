use crate::{
    ber::{
        catalog::{
            Measurement, Reference, Tier,
            linear::{FULL_CAP, coherent_link, envelope_link, params},
        },
        sweep::Link,
        theory,
    },
    constellation::tables,
    linear::{CarrierLoop, EnvelopeTiming, PhaseDetector, TIMING_BW_CONTINUOUS},
};

/// Loop bandwidth for the coherent amplitude rows. A real-axis table has no modulation for an
/// M-th-power detector to strip beyond its 2-fold symmetry, so these rows run the
/// decision-directed detector — and the substrate's wide 0.01 is fine for it here, because the
/// decisions a 2- or 4-level real table makes are reliable well below its own sensitivity.
pub const ASK_LOOP_BW: f64 = 0.01;

#[must_use]
pub fn ook_coherent_link() -> Link {
    coherent_link(
        "ook uncoded, coherent tier: LinearMod -> feedforward timing -> decision-directed Costas \
         -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::ook(), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::DecisionDirected,
                ASK_LOOP_BW,
            ))
        },
    )
}

/// Noncoherent OOK: magnitude detection with the fitted pedestal and scale, no carrier recovery
/// anywhere in the chain.
#[must_use]
pub fn ook_envelope_link() -> Link {
    envelope_link(
        "ook uncoded, noncoherent envelope tier: matched filter -> |·| -> DC removal -> tracking \
         timing -> fitted pedestal and scale, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::ook(), 0.0, false),
        TIMING_BW_CONTINUOUS,
        EnvelopeTiming::CONTINUOUS,
    )
}

/// Coherent Gray 4-PAM — bipolar, the family's efficient member.
#[must_use]
pub fn pam4_link() -> Link {
    coherent_link(
        "4-pam uncoded, coherent tier: LinearMod -> feedforward timing -> decision-directed Costas \
         -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::pam(4), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::DecisionDirected,
                ASK_LOOP_BW,
            ))
        },
    )
}

/// Coherent Gray 8-PAM.
#[must_use]
pub fn pam8_link() -> Link {
    coherent_link(
        "8-pam uncoded, coherent tier: LinearMod -> feedforward timing -> decision-directed Costas \
         -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::pam(8), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::DecisionDirected,
                ASK_LOOP_BW,
            ))
        },
    )
}

/// Coherent unipolar 4-ASK — the same four bits per two symbols as 4-PAM, at the energy penalty
/// unipolar signalling pays.
#[must_use]
pub fn ask4_link() -> Link {
    coherent_link(
        "4-ask (unipolar) uncoded, coherent tier: LinearMod -> feedforward timing -> \
         decision-directed Costas -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud",
        params(tables::ask(4), 0.0, false),
        || {
            Some(CarrierLoop::new(
                PhaseDetector::DecisionDirected,
                ASK_LOOP_BW,
            ))
        },
    )
}

pub const OOK_COHERENT_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const OOK_ENVELOPE_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const PAM4_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0];
pub const PAM8_GRID: &[f64] = &[12.0, 13.0, 14.0, 15.0, 16.0, 17.0];
pub const ASK4_GRID: &[f64] = &[13.0, 14.0, 15.0, 16.0, 17.0];

pub const OOK_COHERENT_AWGN: &str = "linear/ook_coherent_awgn";
pub const OOK_ENVELOPE_AWGN: &str = "linear/ook_envelope_awgn";
pub const PAM4_AWGN: &str = "linear/pam4_awgn";
pub const PAM8_AWGN: &str = "linear/pam8_awgn";
pub const ASK4_AWGN: &str = "linear/ask4_awgn";
pub const OOK_LIMITS: &str = "linear/ook_envelope_limits";
pub const ASK_PERF: &str = "linear/ask_perf";

/// Same tolerance and the same three reasons as the PSK rows carry.
pub const ORACLE_TOLERANCE_DB: f64 = 0.75;

/// Coherent OOK's exact bit error rate. Two equiprobable points at 0 and `A` with mean symbol
/// energy 1 put `A = √2`, so the decision distance is √2 against a per-component σ of √(N0/2) and
/// `BER = Q(√γ)` — exactly BPSK's 3 dB penalty, which is the textbook cost of keying a carrier on
/// and off instead of inverting it.
fn ook_ber(ebn0_db: f64) -> f64 {
    theory::q(10f64.powf(ebn0_db / 10.0).sqrt())
}

/// Unipolar M-ASK's Gray bit error rate. The levels are 0..M−1 scaled to mean Es = 1, i.e. spaced
/// `d = √(6/((M−1)(2M−1)))`, and each interior level has two neighbours: the standard
/// `2(M−1)/M · Q(d/2σ)` symbol form over log2(M) bits.
fn ask_ber(m: u32, ebn0_db: f64) -> f64 {
    let m_f = f64::from(m);
    let k = f64::from(m.ilog2());
    let g = 10f64.powf(ebn0_db / 10.0);
    // Mean symbol energy of 0..M−1 is (M−1)(2M−1)/6, so the normalised spacing squared is
    // 6/((M−1)(2M−1)); the decision distance is half of it against σ² = N0/2 = 1/(2·k·γ_b).
    let d2 = 6.0 / ((m_f - 1.0) * (2.0 * m_f - 1.0));
    2.0 * (m_f - 1.0) / m_f * theory::q((d2 * k * g / 2.0).sqrt()) / k
}

fn ask4_ber(ebn0_db: f64) -> f64 {
    ask_ber(4, ebn0_db)
}

fn pam4_ber(ebn0_db: f64) -> f64 {
    theory::mpam_ber(4, ebn0_db)
}

fn pam8_ber(ebn0_db: f64) -> f64 {
    theory::mpam_ber(8, ebn0_db)
}

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

/// The amplitude entry. The envelope row is commit-and-guard: no closed form describes magnitude
/// detection of a root-raised-cosine-shaped keyed carrier, and its distance from the coherent row
/// is the number the tier comparison records.
pub const MEASUREMENTS: &[Measurement] = &[
    oracle_row(
        OOK_COHERENT_AWGN,
        ook_coherent_link,
        OOK_COHERENT_GRID,
        0x00c0,
        "exact coherent OOK Q(√γ)",
        ook_ber,
        ORACLE_TOLERANCE_DB,
    ),
    Measurement::committed(
        OOK_ENVELOPE_AWGN,
        ook_envelope_link,
        OOK_ENVELOPE_GRID,
        0x00e5,
        FULL_CAP,
    ),
    oracle_row(
        PAM4_AWGN,
        pam4_link,
        PAM4_GRID,
        0x94a3,
        "Gray 4-PAM SER/2",
        pam4_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        PAM8_AWGN,
        pam8_link,
        PAM8_GRID,
        0x84a3,
        "Gray 8-PAM SER/3",
        pam8_ber,
        ORACLE_TOLERANCE_DB,
    ),
    oracle_row(
        ASK4_AWGN,
        ask4_link,
        ASK4_GRID,
        0x4a54,
        "Gray unipolar 4-ASK SER/2",
        ask4_ber,
        ORACLE_TOLERANCE_DB,
    ),
];
