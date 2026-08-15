use num_complex::Complex;

use super::{
    Measurement, Reference,
    mfsk::{bits_to_symbols, push_symbol_bits},
};
use crate::{
    ber::{sweep::Link, theory},
    ppm::{PpmDemod, PpmMod, SlotDetector},
};

pub const SLOT_RATE: f64 = 1_000_000.0;
pub const SLOT_SPS: f64 = 8.0;
pub const RATE: f64 = SLOT_RATE * SLOT_SPS;

pub const LEAD: usize = 2;
pub const SEARCH_SLOTS: usize = 4;
pub const PAYLOAD_SYMBOLS: usize = 2_048;

#[must_use]
pub fn unique_word(m: usize) -> Vec<u8> {
    super::orthogonal::unique_word(m)
}

#[must_use]
pub fn filler(m: usize, len: usize) -> Vec<u8> {
    super::orthogonal::filler(m, len)
}

#[must_use]
pub fn modulate(m: usize, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = PpmMod::new(m, SLOT_SPS, 0.0, 1.0);
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    out
}

#[must_use]
pub fn demod(m: usize, payload_symbols: usize, detector: SlotDetector) -> PpmDemod {
    PpmDemod::new(
        m,
        SLOT_SPS,
        0,
        LEAD + unique_word(m).len() + payload_symbols,
        0.0,
        detector,
    )
}

#[must_use]
pub fn link_sized(m: usize, payload_symbols: usize, detector: SlotDetector) -> Link {
    let bits_per_symbol = m.trailing_zeros() as usize;
    let word = unique_word(m);
    let tx_word = word.clone();
    let receiver = demod(m, payload_symbols, detector);
    let tier = match detector {
        SlotDetector::MatchedFilter => "matched filter",
        SlotDetector::Envelope => "envelope",
    };
    Link {
        label: format!(
            "{m}-PPM, {SLOT_RATE} slots/s at {RATE} Hz ({SLOT_SPS} samples/slot) -> {tier} + \
             argmax, feedforward sub-slot timing and searched word alignment, {LEAD}+{} symbol \
             overhead in Eb, release",
            word.len()
        ),
        bits_per_trial: payload_symbols * bits_per_symbol,
        modulate: Box::new(move |bits| {
            let mut symbols = filler(m, LEAD);
            symbols.extend_from_slice(&tx_word);
            symbols.extend(bits_to_symbols(bits, bits_per_symbol));
            modulate(m, &symbols)
        }),
        demodulate: Box::new(move |wave| {
            let scan_slots = (LEAD + word.len()) * m;
            let offset = receiver.estimate_offset(wave, 0, scan_slots);
            let tail = wave.get(offset..).unwrap_or(&[]);
            let first_slot = receiver.align(tail, &word, LEAD * m + SEARCH_SLOTS);
            let mut symbols = Vec::with_capacity(payload_symbols);
            receiver.demodulate(
                tail,
                first_slot + word.len() * m,
                payload_symbols,
                &mut symbols,
            );
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = symbols.get(k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

#[must_use]
pub fn ppm2_matched_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS, SlotDetector::MatchedFilter)
}

#[must_use]
pub fn ppm2_envelope_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS, SlotDetector::Envelope)
}

#[must_use]
pub fn ppm4_matched_link() -> Link {
    link_sized(4, PAYLOAD_SYMBOLS, SlotDetector::MatchedFilter)
}

pub const M2_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const M4_GRID: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const ENVELOPE_GRID: &[f64] = &[9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0];

pub const M2_SEED: u64 = 0x0bb2;
pub const M4_SEED: u64 = 0x0bb4;
pub const ENVELOPE_SEED: u64 = 0x0bbe;

pub const FULL_CAP: u64 = 3_000_000;

pub const ORACLE_TOLERANCE_DB: f64 = 0.4;

pub const M2_MATCHED_AWGN: &str = "ppm/ppm2_matched_awgn";
pub const M2_ENVELOPE_AWGN: &str = "ppm/ppm2_envelope_awgn";
pub const M4_MATCHED_AWGN: &str = "ppm/ppm4_matched_awgn";
pub const MATCHED_LIMITS: &str = "ppm/ppm2_matched_limits";
pub const ENVELOPE_LIMITS: &str = "ppm/ppm2_envelope_limits";
pub const PERF: &str = "ppm/ppm_perf";

fn m2_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(2, ebn0_db)
}

fn m4_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(4, ebn0_db)
}

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement {
        reference: Reference::Oracle {
            name: "exact noncoherent orthogonal 2-ary",
            ber: m2_ber,
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
        ..Measurement::committed(
            M2_MATCHED_AWGN,
            ppm2_matched_link,
            M2_GRID,
            M2_SEED,
            FULL_CAP,
        )
    },
    Measurement {
        reference: Reference::Oracle {
            name: "exact noncoherent orthogonal 4-ary",
            ber: m4_ber,
            tolerance_db: ORACLE_TOLERANCE_DB,
        },
        ..Measurement::committed(
            M4_MATCHED_AWGN,
            ppm4_matched_link,
            M4_GRID,
            M4_SEED,
            FULL_CAP,
        )
    },
    Measurement::committed(
        M2_ENVELOPE_AWGN,
        ppm2_envelope_link,
        ENVELOPE_GRID,
        ENVELOPE_SEED,
        FULL_CAP,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tier_round_trips_on_a_clean_channel() {
        for (m, detector) in [
            (2usize, SlotDetector::MatchedFilter),
            (2, SlotDetector::Envelope),
            (4, SlotDetector::MatchedFilter),
            (4, SlotDetector::Envelope),
        ] {
            let link = link_sized(m, 32, detector);
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 5 < 2).collect();
            let wave = (link.modulate)(&bits);
            assert_eq!((link.demodulate)(&wave), bits, "M = {m} {detector:?}");
        }
    }
}
