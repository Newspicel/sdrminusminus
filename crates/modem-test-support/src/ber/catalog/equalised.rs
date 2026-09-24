use num_complex::Complex;
use sdrmm_modem::{
    constellation::tables,
    linear::{
        CarrierLoop, EqualiserConfig, LinearBurstDemod, LinearDemod, LinearMod, LinearParams,
        LinearTiming, PhaseDetector,
    },
};

use crate::ber::{
    catalog::{
        Measurement,
        linear::{
            FILLER_SEED, FULL_CAP, PAYLOAD_SYMBOLS, POWER_SYMBOLS, SPS, UW, bits_to_labels,
            decode_coherent, frame, params, rrc, unique_word,
        },
    },
    sweep::Link,
};

pub const ECHOES: &[(usize, f64, f64)] = &[
    (0, 1.0, 0.0),
    (SPS, 0.35, 1.0),
    (SPS + SPS / 2, 0.2, -2.0),
    (3 * SPS, 0.1, 0.5),
];

pub const CARRIER_BW: f64 = 0.003;

pub const QPSK_MULTIPATH_EQ: &str = "linear/qpsk_multipath_eq";
pub const QAM16_MULTIPATH_EQ: &str = "linear/qam16_multipath_eq";
pub const QAM64_MULTIPATH_EQ: &str = "linear/qam64_multipath_eq";
pub const QAM16_TRACKED_MULTIPATH_EQ: &str = "linear/qam16_tracked_multipath_eq";

pub const QPSK_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
pub const QAM16_GRID: &[f64] = &[8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const QAM64_GRID: &[f64] = &[13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0];

#[must_use]
pub fn through_echoes(wave: &[Complex<f32>]) -> Vec<Complex<f32>> {
    let power: f64 = ECHOES.iter().map(|&(_, a, _)| a * a).sum();
    let taps: Vec<(usize, Complex<f32>)> = ECHOES
        .iter()
        .map(|&(delay, a, phase)| {
            let tap = Complex::from_polar(a / power.sqrt(), phase);
            (delay, Complex::new(tap.re as f32, tap.im as f32))
        })
        .collect();
    (0..wave.len())
        .map(|n| {
            taps.iter()
                .filter_map(|&(delay, h)| n.checked_sub(delay).map(|i| wave[i] * h))
                .sum()
        })
        .collect()
}

fn carrier() -> Option<CarrierLoop> {
    Some(CarrierLoop::new(
        PhaseDetector::DecisionDirected,
        CARRIER_BW,
    ))
}

fn transmit(tx: &LinearParams, bits: &[bool]) -> Vec<Complex<f32>> {
    let table = tx.constellation();
    let payload = bits_to_labels(bits, tx.bits_per_symbol());
    let uw = unique_word(table, UW, FILLER_SEED);
    through_echoes(&LinearMod::transmission(tx, &frame(table, &uw, &payload)))
}

#[must_use]
pub fn burst_link(label: &str, params: LinearParams, equaliser: Option<EqualiserConfig>) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| transmit(&tx, bits)),
        demodulate: Box::new(move |wave| {
            let mut demod = LinearBurstDemod::new(&params, &rx, POWER_SYMBOLS, carrier());
            if let Some(config) = equaliser {
                match demod.with_equaliser(config) {
                    Ok(equalised) => demod = equalised,
                    Err(_) => return Vec::new(),
                }
            }
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(params.constellation(), &symbols, bits_per_symbol)
        }),
    }
}

#[must_use]
pub fn tracked_link(label: &str, params: LinearParams, equaliser: Option<EqualiserConfig>) -> Link {
    let bits_per_symbol = params.bits_per_symbol();
    let tx = params.clone();
    let rx = rrc();
    Link {
        label: label.to_string(),
        bits_per_trial: PAYLOAD_SYMBOLS * bits_per_symbol,
        modulate: Box::new(move |bits| transmit(&tx, bits)),
        demodulate: Box::new(move |wave| {
            let mut demod = LinearDemod::new(&params, &rx, LinearTiming::CONTINUOUS, carrier());
            if let Some(config) = equaliser {
                match demod.with_equaliser(config) {
                    Ok(equalised) => demod = equalised,
                    Err(_) => return Vec::new(),
                }
            }
            let mut symbols = Vec::new();
            demod.process(wave, &mut symbols);
            decode_coherent(params.constellation(), &symbols, bits_per_symbol)
        }),
    }
}

fn label(name: &str, tier: &str) -> String {
    format!(
        "{name} uncoded through a four-ray echo (3 symbols), {tier}: T/2 equaliser \
         (8 symbols, blind stop-and-go then decision-directed) -> decision-directed Costas \
         (bw {CARRIER_BW}) -> unique-word anchor, RRC α=0.35 span 8, 8 sps, 48 kHz 6000 baud"
    )
}

#[must_use]
pub fn qpsk_link() -> Link {
    burst_link(
        &label("qpsk", "burst tier"),
        params(tables::qam_square(4), 0.0, false),
        Some(EqualiserConfig::DEFAULT),
    )
}

#[must_use]
pub fn qam16_link() -> Link {
    burst_link(
        &label("16-qam", "burst tier"),
        params(tables::qam_square(16), 0.0, false),
        Some(EqualiserConfig::DEFAULT),
    )
}

#[must_use]
pub fn qam64_link() -> Link {
    burst_link(
        &label("64-qam", "burst tier"),
        params(tables::qam_square(64), 0.0, false),
        Some(EqualiserConfig::DEFAULT),
    )
}

#[must_use]
pub fn qam16_tracked_link() -> Link {
    tracked_link(
        &label("16-qam", "tracking tier"),
        params(tables::qam_square(16), 0.0, false),
        Some(EqualiserConfig::DEFAULT),
    )
}

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement::committed(QPSK_MULTIPATH_EQ, qpsk_link, QPSK_GRID, 0xe904, FULL_CAP),
    Measurement::committed(QAM16_MULTIPATH_EQ, qam16_link, QAM16_GRID, 0xe916, FULL_CAP),
    Measurement::committed(QAM64_MULTIPATH_EQ, qam64_link, QAM64_GRID, 0xe964, FULL_CAP),
    Measurement::committed(
        QAM16_TRACKED_MULTIPATH_EQ,
        qam16_tracked_link,
        QAM16_GRID,
        0xe917,
        FULL_CAP,
    ),
];
