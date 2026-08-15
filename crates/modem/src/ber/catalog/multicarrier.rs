use std::sync::LazyLock;

use num_complex::Complex;

use super::{
    Measurement, Reference, Tier,
    linear::{bits_to_labels, labels_to_bits, table},
    ofdm::{LEAD, TAIL},
};
use crate::{
    ber::{sweep::Link, theory},
    constellation::{Constellation, tables},
    multicarrier::{
        FbmcDemod, FbmcMod, FbmcParams, GfdmDemod, GfdmDetector, GfdmMod, GfdmParams, OtfsGrid,
        OtfsPrecoder, UfmcDemod, UfmcMod, UfmcParams,
    },
    ofdm::{OfdmDemod, OfdmMod, OfdmParams},
};

pub const SYMBOLS: usize = 16;

pub const FBMC_SYMBOLS: usize = 32;
pub const FBMC_GUARD: usize = 4;

pub const GFDM_BLOCKS: usize = 16;

#[must_use]
pub fn gfdm_params() -> GfdmParams {
    let mut params = GfdmParams::new(16, 5, 0.5);
    params.cp = 8;
    params
}

pub static GFDM_OVERHEAD_DB: LazyLock<f64> = LazyLock::new(|| {
    let params = gfdm_params();
    10.0 * (params.samples() as f64 / params.block() as f64).log10()
});

pub static UFMC_OVERHEAD_DB: LazyLock<f64> =
    LazyLock::new(|| UfmcParams::reference().overhead_db());

pub static FBMC_OVERHEAD_DB: LazyLock<f64> =
    LazyLock::new(|| 10.0 * ((FBMC_SYMBOLS + 2 * FBMC_GUARD) as f64 / FBMC_SYMBOLS as f64).log10());

pub static OTFS_OVERHEAD_DB: LazyLock<f64> =
    LazyLock::new(|| OfdmParams::wifi_like().framing_overhead_db(SYMBOLS));

fn qpsk() -> Constellation {
    table("multicarrier qpsk", tables::qam_square(4))
}

fn points_by_label(table: &Constellation) -> Vec<Complex<f32>> {
    let mut by_label = vec![Complex::new(0.0, 0.0); table.len()];
    for (&label, &point) in table.labels().iter().zip(table.points()) {
        by_label[label as usize] = point;
    }
    by_label
}

#[must_use]
pub fn gfdm_link(detector: GfdmDetector) -> Link {
    let params = gfdm_params();
    let constellation = qpsk();
    let by_label = points_by_label(&constellation);
    let bits_per_symbol = constellation.bits_per_symbol();
    let points = GFDM_BLOCKS * params.block();
    let modulator = GfdmMod::new(params);
    let demodulator = GfdmDemod::new(params, detector);
    let tier = match detector {
        GfdmDetector::ZeroForcing => "zero forcing (A⁻¹)",
        GfdmDetector::Matched => "matched filter (Aᴴ)",
    };
    Link {
        label: format!(
            "gfdm qpsk uncoded, {}×{} block, roll-off {}, {}-sample prefix, {GFDM_BLOCKS} blocks, \
             {tier}",
            params.subcarriers, params.subsymbols, params.rolloff, params.cp
        ),
        bits_per_trial: points * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let points: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            let mut wave = Vec::with_capacity(GFDM_BLOCKS * params.samples());
            modulator.modulate(&points, &mut wave);
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demodulator = demodulator.clone();
            let mut points = Vec::with_capacity(points);
            demodulator.demodulate(wave, &mut points);
            let labels: Vec<u32> = points
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn ufmc_link() -> Link {
    let params = UfmcParams::reference();
    let constellation = qpsk();
    let by_label = points_by_label(&constellation);
    let bits_per_symbol = constellation.bits_per_symbol();
    let points = SYMBOLS * params.points();
    let modulator = UfmcMod::new(params);
    let demodulator = UfmcDemod::new(params);
    Link {
        label: format!(
            "ufmc qpsk uncoded, {}-point transform, {} subbands of {}, {}-tap prototype, \
             {SYMBOLS} symbols, zero-pad-to-2N receiver",
            params.fft, params.subbands, params.per_subband, params.filter_len
        ),
        bits_per_trial: points * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let points: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            let mut wave = Vec::with_capacity(SYMBOLS * params.samples());
            modulator.modulate(&points, &mut wave);
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demodulator = demodulator.clone();
            let mut points = Vec::with_capacity(points);
            demodulator.demodulate(wave, &mut points);
            let labels: Vec<u32> = points
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn fbmc_link() -> Link {
    let params = FbmcParams::reference();
    let constellation = qpsk();
    let by_label = points_by_label(&constellation);
    let bits_per_symbol = constellation.bits_per_symbol();
    let payload = FBMC_SYMBOLS * params.allocated;
    let total = (FBMC_SYMBOLS + 2 * FBMC_GUARD) * params.allocated;
    let modulator = FbmcMod::new(params);
    let demodulator = FbmcDemod::new(params);
    Link {
        label: format!(
            "fbmc/oqam qpsk uncoded, {}-subcarrier bank, {} allocated, PHYDYAS K=4, \
             {FBMC_SYMBOLS} payload + {FBMC_GUARD} guard symbols each end",
            params.subcarriers, params.allocated
        ),
        bits_per_trial: payload * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let mut points = vec![Complex::new(0.0, 0.0); total];
            let guard = FBMC_GUARD * params.allocated;
            for (slot, label) in points[guard..guard + payload]
                .iter_mut()
                .zip(bits_to_labels(bits, bits_per_symbol))
            {
                *slot = by_label[label as usize];
            }
            let mut filler = 0u32;
            let (head, rest) = points.split_at_mut(guard);
            let tail = &mut rest[payload..];
            for slot in head.iter_mut().chain(tail.iter_mut()) {
                filler = filler.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *slot = by_label[(filler >> 28) as usize % by_label.len()];
            }
            let mut wave = Vec::new();
            modulator.modulate(&points, &mut wave);
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demodulator = demodulator.clone();
            let mut points = Vec::with_capacity(total);
            demodulator.demodulate(wave, FBMC_SYMBOLS + 2 * FBMC_GUARD, &mut points);
            let guard = FBMC_GUARD * params.allocated;
            let labels: Vec<u32> = points[guard..guard + payload]
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn otfs_link() -> Link {
    let params = OfdmParams::wifi_like();
    let grid = OtfsGrid::new(params.data_subcarriers(), SYMBOLS);
    let constellation = qpsk();
    let by_label = points_by_label(&constellation);
    let bits_per_symbol = constellation.bits_per_symbol();
    let modulator = OfdmMod::new(params.clone());
    let mut demod = OfdmDemod::new(params.clone())
        .with_pilot_tracking(false)
        .with_window_backoff(0);
    demod.genie(
        LEAD + params.data_offset(),
        &vec![Complex::new(1.0, 0.0); params.map().occupied().len()],
        1.0,
    );
    let precoder = OtfsPrecoder::new(grid);
    Link {
        label: format!(
            "otfs qpsk uncoded, {}×{} delay–Doppler grid over CP-OFDM {}-point/{}-prefix, \
             genie origin, carrier, channel and phase",
            grid.delay,
            grid.doppler,
            params.fft(),
            params.cp()
        ),
        bits_per_trial: grid.points() * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let mut precoder = precoder.clone();
            let dd: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            let mut tf = vec![Complex::new(0.0, 0.0); dd.len()];
            precoder.spread(&dd, &mut tf);
            let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
            modulator.frame(&tf, &mut wave);
            wave.resize(wave.len() + TAIL, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            let mut precoder = OtfsPrecoder::new(grid);
            let mut tf = Vec::with_capacity(grid.points());
            demod.demodulate(wave, SYMBOLS, &mut tf);
            if tf.len() != grid.points() {
                return Vec::new();
            }
            let mut dd = vec![Complex::new(0.0, 0.0); grid.points()];
            precoder.despread(&tf, &mut dd);
            let labels: Vec<u32> = dd.iter().map(|&p| constellation.hard_slice(p)).collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn ofdm_reference_link() -> Link {
    let params = OfdmParams::wifi_like();
    let constellation = qpsk();
    let by_label = points_by_label(&constellation);
    let bits_per_symbol = constellation.bits_per_symbol();
    let data = params.data_subcarriers();
    let modulator = OfdmMod::new(params.clone());
    let mut demod = OfdmDemod::new(params.clone())
        .with_pilot_tracking(false)
        .with_window_backoff(0);
    demod.genie(
        LEAD + params.data_offset(),
        &vec![Complex::new(1.0, 0.0); params.map().occupied().len()],
        1.0,
    );
    Link {
        label: format!(
            "qpsk-ofdm uncoded, CP-OFDM {}-point/{}-prefix, {SYMBOLS} data symbols, genie",
            params.fft(),
            params.cp()
        ),
        bits_per_trial: SYMBOLS * data * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut modulator = modulator.clone();
            let points: Vec<Complex<f32>> = bits_to_labels(bits, bits_per_symbol)
                .into_iter()
                .map(|label| by_label[label as usize])
                .collect();
            let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
            modulator.frame(&points, &mut wave);
            wave.resize(wave.len() + TAIL, Complex::new(0.0, 0.0));
            wave
        }),
        demodulate: Box::new(move |wave| {
            let mut demod = demod.clone();
            let mut points = Vec::with_capacity(SYMBOLS * data);
            demod.demodulate(wave, SYMBOLS, &mut points);
            let labels: Vec<u32> = points
                .iter()
                .map(|&p| constellation.hard_slice(p))
                .collect();
            labels_to_bits(&labels, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn gfdm_zf_link() -> Link {
    gfdm_link(GfdmDetector::ZeroForcing)
}

#[must_use]
pub fn gfdm_mf_link() -> Link {
    gfdm_link(GfdmDetector::Matched)
}

#[must_use]
pub fn gfdm_amplification_db() -> f64 {
    let demod = GfdmDemod::new(gfdm_params(), GfdmDetector::ZeroForcing);
    let amplification = demod.amplification();
    let mean = f64::from(amplification.iter().sum::<f32>()) / amplification.len() as f64;
    10.0 * mean.log10()
}

fn ufmc_oracle(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - *UFMC_OVERHEAD_DB)
}

fn fbmc_oracle(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - *FBMC_OVERHEAD_DB)
}

fn otfs_oracle(ebn0_db: f64) -> f64 {
    theory::qpsk_ber(ebn0_db - *OTFS_OVERHEAD_DB)
}

pub const GRID: &[f64] = &[0.0, 2.0, 4.0, 6.0, 8.0, 9.0, 10.0, 11.0];

pub const MF_GRID: &[f64] = &[0.0, 2.0, 4.0, 6.0, 8.0];

pub const CAP: u64 = 4_000_000;

pub const GFDM_ZF_SEED: u64 = 0x9f_d0;
pub const GFDM_MF_SEED: u64 = 0x9f_d1;
pub const UFMC_SEED: u64 = 0x0_fc5e;
pub const FBMC_SEED: u64 = 0x0_fb3c;
pub const OTFS_SEED: u64 = 0x0_07f5;

const fn oracle(
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
            max_trial_bits: CAP,
        },
        smoke_points: super::SMOKE_POINTS,
        reference: Reference::Oracle {
            name,
            ber,
            tolerance_db: 0.4,
        },
    }
}

pub const GFDM: &[Measurement] = &[
    Measurement::committed(
        "multicarrier/gfdm_zf_awgn",
        gfdm_zf_link,
        GRID,
        GFDM_ZF_SEED,
        CAP,
    ),
    Measurement::committed(
        "multicarrier/gfdm_mf_awgn",
        gfdm_mf_link,
        MF_GRID,
        GFDM_MF_SEED,
        CAP,
    ),
];

pub const UFMC: &[Measurement] = &[oracle(
    "multicarrier/ufmc_awgn",
    ufmc_link,
    GRID,
    UFMC_SEED,
    "Gray QPSK + the filter tail's overhead",
    ufmc_oracle,
)];

pub const FBMC: &[Measurement] = &[oracle(
    "multicarrier/fbmc_awgn",
    fbmc_link,
    GRID,
    FBMC_SEED,
    "Gray QPSK + the guard symbols' overhead",
    fbmc_oracle,
)];

pub const OTFS: &[Measurement] = &[oracle(
    "multicarrier/otfs_awgn",
    otfs_link,
    GRID,
    OTFS_SEED,
    "Gray QPSK + the carrier frame's overhead",
    otfs_oracle,
)];

pub const GFDM_LIMITS: &str = "multicarrier/gfdm_zf_limits";
pub const OTFS_LIMITS: &str = "multicarrier/otfs_limits";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_overhead_is_a_closed_form_of_its_geometry() {
        assert!((*GFDM_OVERHEAD_DB - 10.0 * (88.0f64 / 80.0).log10()).abs() < 1e-12);
        assert!((*GFDM_OVERHEAD_DB - 0.4139).abs() < 1e-3);
        assert!((*UFMC_OVERHEAD_DB - 10.0 * (160.0f64 / 128.0).log10()).abs() < 1e-12);
        assert!((*UFMC_OVERHEAD_DB - 0.9691).abs() < 1e-3);
        assert!((*FBMC_OVERHEAD_DB - 10.0 * (40.0f64 / 32.0).log10()).abs() < 1e-12);
        assert!((*FBMC_OVERHEAD_DB - 0.9691).abs() < 1e-3);
        assert!(
            (*OTFS_OVERHEAD_DB - OfdmParams::wifi_like().framing_overhead_db(SYMBOLS)).abs()
                < 1e-12
        );
    }

    #[test]
    fn the_inverse_states_its_own_cost() {
        let db = gfdm_amplification_db();
        println!("gfdm zero-forcing mean amplification: {db:.4} dB");
        assert!((0.3..1.0).contains(&db), "mean amplification {db} dB");
    }

    #[test]
    fn every_link_states_the_bits_it_carries() {
        assert_eq!(gfdm_zf_link().bits_per_trial, GFDM_BLOCKS * 80 * 2);
        assert_eq!(ufmc_link().bits_per_trial, SYMBOLS * 48 * 2);
        assert_eq!(fbmc_link().bits_per_trial, FBMC_SYMBOLS * 48 * 2);
        assert_eq!(otfs_link().bits_per_trial, SYMBOLS * 48 * 2);
        assert_eq!(
            ofdm_reference_link().bits_per_trial,
            otfs_link().bits_per_trial,
            "the OTFS comparison must carry the same payload as the entry"
        );
    }
}
