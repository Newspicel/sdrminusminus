use std::sync::LazyLock;

use num_complex::Complex;
use sdrmm_modem::{
    constellation::{Constellation, tables},
    ofdm::{ChannelEstimator, OfdmDemod, OfdmMod, OfdmParams},
};

use super::{
    Measurement, Reference, Tier,
    linear::{bits_to_labels, labels_to_bits, table},
};
use crate::ber::{sweep::Link, theory};

pub const RATE: f64 = 20_000_000.0;

pub const SYMBOLS: usize = 64;

pub const LEAD: usize = 32;

pub const TAIL: usize = 256;

pub const SEARCH: usize = 192;

pub const FULL_CAP: u64 = 3_000_000;

pub static OVERHEAD_DB: LazyLock<f64> =
    LazyLock::new(|| OfdmParams::wifi_like().framing_overhead_db(SYMBOLS));

pub static DMT_OVERHEAD_DB: LazyLock<f64> =
    LazyLock::new(|| OfdmParams::dmt_like().framing_overhead_db(SYMBOLS));

fn points_by_label(table: &Constellation) -> Vec<Complex<f32>> {
    let mut by_label = vec![Complex::new(0.0, 0.0); table.len()];
    for (index, &label) in table.labels().iter().enumerate() {
        by_label[label as usize] = table.points()[index];
    }
    by_label
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Receiver {
    Acquire(ChannelEstimator),
    Genie,
}

#[must_use]
pub fn link_sized(
    name: &str,
    params: OfdmParams,
    constellation: Constellation,
    receiver: Receiver,
    symbols: usize,
) -> Link {
    let bits_per_symbol = constellation.bits_per_symbol();
    let data = params.data_subcarriers();
    let by_label = points_by_label(&constellation);
    let modulator = OfdmMod::new(params.clone());
    let mut demod = match receiver {
        Receiver::Acquire(estimator) => OfdmDemod::new(params.clone()).with_estimator(estimator),
        Receiver::Genie => OfdmDemod::new(params.clone())
            .with_pilot_tracking(false)
            .with_window_backoff(0),
    };
    if receiver == Receiver::Genie {
        demod.genie(
            LEAD + params.data_offset(),
            &vec![Complex::new(1.0, 0.0); params.map().occupied().len()],
            1.0,
        );
    }
    let tier = match receiver {
        Receiver::Acquire(ChannelEstimator::LongTraining) => {
            "preamble search -> coarse+fine CFO -> long-training LS estimate -> one-tap EQ -> \
             pilot tracking"
        }
        Receiver::Acquire(ChannelEstimator::ShortComb) => {
            "preamble search -> coarse+fine CFO -> short-training comb estimate + interpolation \
             -> one-tap EQ -> pilot tracking"
        }
        Receiver::Genie => "genie origin, carrier, channel and phase -> one-tap EQ",
    };
    let label = format!(
        "{name} uncoded, CP-OFDM {}-point/{}-prefix, {data}+{} subcarriers at {} MHz, {symbols} \
         data symbols, {tier}",
        params.fft(),
        params.cp(),
        params.map().pilots().len(),
        RATE / 1e6
    );
    Link {
        label,
        bits_per_trial: symbols * data * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
            let points: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            modulator.frame(&points, &mut wave);
            wave.resize(wave.len() + TAIL, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            if receiver != Receiver::Genie && demod.acquire(wave, SEARCH).is_none() {
                return Vec::new();
            }
            let mut points = Vec::with_capacity(symbols * data);
            demod.demodulate(wave, symbols, &mut points);
            let labels: Vec<u32> = points
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

fn wifi_link(name: &str, constellation: Constellation, receiver: Receiver) -> Link {
    link_sized(
        name,
        OfdmParams::wifi_like(),
        constellation,
        receiver,
        SYMBOLS,
    )
}

#[must_use]
pub fn bpsk_link() -> Link {
    wifi_link(
        "bpsk-ofdm",
        table("ofdm bpsk", tables::pam(2)),
        Receiver::Acquire(ChannelEstimator::LongTraining),
    )
}

#[must_use]
pub fn qpsk_link() -> Link {
    wifi_link(
        "qpsk-ofdm",
        table("ofdm qpsk", tables::qam_square(4)),
        Receiver::Acquire(ChannelEstimator::LongTraining),
    )
}

#[must_use]
pub fn qam16_link() -> Link {
    wifi_link(
        "16qam-ofdm",
        table("ofdm 16-qam", tables::qam_square(16)),
        Receiver::Acquire(ChannelEstimator::LongTraining),
    )
}

#[must_use]
pub fn qam64_link() -> Link {
    wifi_link(
        "64qam-ofdm",
        table("ofdm 64-qam", tables::qam_square(64)),
        Receiver::Acquire(ChannelEstimator::LongTraining),
    )
}

#[must_use]
pub fn bpsk_genie_link() -> Link {
    wifi_link(
        "bpsk-ofdm genie",
        table("ofdm bpsk", tables::pam(2)),
        Receiver::Genie,
    )
}

#[must_use]
pub fn qpsk_genie_link() -> Link {
    wifi_link(
        "qpsk-ofdm genie",
        table("ofdm qpsk", tables::qam_square(4)),
        Receiver::Genie,
    )
}

#[must_use]
pub fn qam16_genie_link() -> Link {
    wifi_link(
        "16qam-ofdm genie",
        table("ofdm 16-qam", tables::qam_square(16)),
        Receiver::Genie,
    )
}

#[must_use]
pub fn qam64_genie_link() -> Link {
    wifi_link(
        "64qam-ofdm genie",
        table("ofdm 64-qam", tables::qam_square(64)),
        Receiver::Genie,
    )
}

#[must_use]
pub fn dmt_genie_link() -> Link {
    link_sized(
        "qpsk-dmt genie",
        OfdmParams::dmt_like(),
        table("dmt qpsk", tables::qam_square(4)),
        Receiver::Genie,
        SYMBOLS,
    )
}

#[must_use]
pub fn qpsk_comb_link() -> Link {
    wifi_link(
        "qpsk-ofdm comb",
        table("ofdm qpsk", tables::qam_square(4)),
        Receiver::Acquire(ChannelEstimator::ShortComb),
    )
}

#[must_use]
pub fn dmt_link() -> Link {
    link_sized(
        "qpsk-dmt",
        OfdmParams::dmt_like(),
        table("dmt qpsk", tables::qam_square(4)),
        Receiver::Acquire(ChannelEstimator::LongTraining),
        SYMBOLS,
    )
}

pub const BPSK_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const QPSK_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0];
pub const QAM16_GRID: &[f64] = &[12.0, 13.0, 14.0, 15.0, 16.0, 17.0];
pub const QAM64_GRID: &[f64] = &[16.0, 17.0, 18.0, 19.0, 20.0, 21.0, 22.0];
pub const COMB_GRID: &[f64] = &[6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const DMT_GRID: &[f64] = &[12.0, 13.0, 14.0, 15.0, 16.0, 17.0];

pub const BPSK_GENIE_GRID: &[f64] = &[6.0, 7.0, 8.0, 9.0, 10.0, 11.0];
pub const QPSK_GENIE_GRID: &[f64] = &[6.0, 7.0, 8.0, 9.0, 10.0, 11.0];
pub const QAM16_GENIE_GRID: &[f64] = &[10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
pub const QAM64_GENIE_GRID: &[f64] = &[14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0];
pub const DMT_GENIE_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0, 14.0];

pub const BPSK_SEED: u64 = 0x0fd2;
pub const QPSK_SEED: u64 = 0x0fd4;
pub const QAM16_SEED: u64 = 0x0fd6;
pub const QAM64_SEED: u64 = 0x0fd8;
pub const COMB_SEED: u64 = 0x0fdc;
pub const DMT_SEED: u64 = 0x0fde;
pub const BPSK_GENIE_SEED: u64 = 0x0fe2;
pub const QPSK_GENIE_SEED: u64 = 0x0fe4;
pub const QAM16_GENIE_SEED: u64 = 0x0fe6;
pub const QAM64_GENIE_SEED: u64 = 0x0fe8;
pub const DMT_GENIE_SEED: u64 = 0x0fea;

pub const BPSK_AWGN: &str = "ofdm/bpsk_awgn";
pub const QPSK_AWGN: &str = "ofdm/qpsk_awgn";
pub const QAM16_AWGN: &str = "ofdm/qam16_awgn";
pub const QAM64_AWGN: &str = "ofdm/qam64_awgn";
pub const BPSK_GENIE_AWGN: &str = "ofdm/bpsk_genie_awgn";
pub const QPSK_GENIE_AWGN: &str = "ofdm/qpsk_genie_awgn";
pub const QAM16_GENIE_AWGN: &str = "ofdm/qam16_genie_awgn";
pub const QAM64_GENIE_AWGN: &str = "ofdm/qam64_genie_awgn";
pub const QPSK_COMB_AWGN: &str = "ofdm/qpsk_comb_awgn";
pub const DMT_AWGN: &str = "ofdm/dmt_qpsk_awgn";
pub const DMT_GENIE_AWGN: &str = "ofdm/dmt_qpsk_genie_awgn";
pub const LIMITS: &str = "ofdm/qpsk_limits";
pub const COMB_LIMITS: &str = "ofdm/qpsk_comb_limits";
pub const PERF: &str = "ofdm/ofdm_perf";

pub const ORACLE_TOLERANCE_DB: f64 = 0.75;

fn bpsk_ber(ebn0_db: f64) -> f64 {
    theory::bpsk_ber(ebn0_db - *OVERHEAD_DB)
}

fn qpsk_ber(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - *OVERHEAD_DB)
}

fn qam16_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(16, ebn0_db - *OVERHEAD_DB)
}

fn qam64_ber(ebn0_db: f64) -> f64 {
    theory::mqam_ber(64, ebn0_db - *OVERHEAD_DB)
}

fn dmt_ber(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - *DMT_OVERHEAD_DB)
}

const fn oracle_row(
    stem: &'static str,
    link: fn() -> Link,
    grid: &'static [f64],
    seed: u64,
    name: &'static str,
    ber: fn(f64) -> f64,
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
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
    }
}

pub const MODULATIONS: &[Measurement] = &[
    Measurement::committed(BPSK_AWGN, bpsk_link, BPSK_GRID, BPSK_SEED, FULL_CAP),
    Measurement::committed(QPSK_AWGN, qpsk_link, QPSK_GRID, QPSK_SEED, FULL_CAP),
    Measurement::committed(QAM16_AWGN, qam16_link, QAM16_GRID, QAM16_SEED, FULL_CAP),
    Measurement::committed(QAM64_AWGN, qam64_link, QAM64_GRID, QAM64_SEED, FULL_CAP),
];

pub const GENIE: &[Measurement] = &[
    oracle_row(
        BPSK_GENIE_AWGN,
        bpsk_genie_link,
        BPSK_GENIE_GRID,
        BPSK_GENIE_SEED,
        "exact ½·erfc(√γ) + the frame's own overhead",
        bpsk_ber,
    ),
    oracle_row(
        QPSK_GENIE_AWGN,
        qpsk_genie_link,
        QPSK_GENIE_GRID,
        QPSK_GENIE_SEED,
        "exact Gray QPSK + the frame's own overhead",
        qpsk_ber,
    ),
    oracle_row(
        QAM16_GENIE_AWGN,
        qam16_genie_link,
        QAM16_GENIE_GRID,
        QAM16_GENIE_SEED,
        "Gray square 16-QAM + the frame's own overhead",
        qam16_ber,
    ),
    oracle_row(
        QAM64_GENIE_AWGN,
        qam64_genie_link,
        QAM64_GENIE_GRID,
        QAM64_GENIE_SEED,
        "Gray square 64-QAM + the frame's own overhead",
        qam64_ber,
    ),
];

pub const ESTIMATION: &[Measurement] = &[Measurement::committed(
    QPSK_COMB_AWGN,
    qpsk_comb_link,
    COMB_GRID,
    COMB_SEED,
    FULL_CAP,
)];

pub const DMT: &[Measurement] = &[
    Measurement::committed(DMT_AWGN, dmt_link, DMT_GRID, DMT_SEED, FULL_CAP),
    oracle_row(
        DMT_GENIE_AWGN,
        dmt_genie_link,
        DMT_GENIE_GRID,
        DMT_GENIE_SEED,
        "exact Gray QPSK + the DMT frame's own overhead",
        dmt_ber,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_overhead_covers_every_order() {
        let params = OfdmParams::wifi_like();
        let want = 10.0 * f64::log10((260.0 + 65.0 * 64.0) / (48.0 * 64.0));
        assert!((*OVERHEAD_DB - want).abs() < 1e-12, "{}", *OVERHEAD_DB);
        assert!((*OVERHEAD_DB - 1.580).abs() < 5e-3, "{}", *OVERHEAD_DB);
        let structural = 10.0 * f64::log10(65.0 / 48.0);
        assert!((structural - 1.317).abs() < 5e-3, "{structural}");
        assert!((*OVERHEAD_DB - structural - 0.263).abs() < 5e-3);
        assert!((params.framing_overhead_db(SYMBOLS) - *OVERHEAD_DB).abs() < 1e-12);
        assert!(
            (*DMT_OVERHEAD_DB - *OVERHEAD_DB - 3.0103).abs() < 1e-3,
            "{}",
            *DMT_OVERHEAD_DB
        );
    }

    #[test]
    fn every_chain_round_trips_on_a_clean_channel() {
        for (name, link) in [
            ("bpsk", bpsk_link()),
            ("qpsk", qpsk_link()),
            ("16-qam", qam16_link()),
            ("64-qam", qam64_link()),
            ("qpsk genie", qpsk_genie_link()),
            ("qpsk comb", qpsk_comb_link()),
            ("dmt", dmt_link()),
        ] {
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 7 < 3).collect();
            let wave = (link.modulate)(&bits);
            let decoded = (link.demodulate)(&wave);
            let errors = bits
                .iter()
                .enumerate()
                .filter(|(i, b)| decoded.get(*i) != Some(b))
                .count();
            assert_eq!(errors, 0, "{name}: {errors} of {} bits", bits.len());
        }
    }

    #[test]
    fn the_genie_row_transmits_the_same_waveform_as_the_acquiring_one() {
        let bits: Vec<bool> = (0..6144).map(|i| i % 5 < 2).collect();
        let a = (qpsk_link().modulate)(&bits);
        let b = (qpsk_genie_link().modulate)(&bits);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(x, y)| (x - y).norm() < 1e-9));
        assert_eq!(a.len(), LEAD + 320 + SYMBOLS * 80 + TAIL);
    }
}
