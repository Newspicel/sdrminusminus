use num_complex::Complex;

use super::{
    Measurement, Reference,
    mfsk::{bits_to_symbols, push_symbol_bits},
};
use crate::{
    ber::{sweep::Link, theory},
    orthogonal::{MfskDemod, MfskMod, MfskParams, TonePhase},
};

pub const RATE: f64 = 48_000.0;
pub const BAUD: f64 = 4_800.0;
pub const SPS: f64 = 10.0;

pub const LEAD: usize = 16;
pub const SEARCH: usize = 8;
pub const PAYLOAD_SYMBOLS: usize = 2_048;

pub const UW: [u8; 24] = [
    5, 3, 1, 0, 0, 6, 1, 2, 7, 2, 1, 5, 6, 3, 5, 4, 5, 7, 7, 3, 6, 2, 6, 4,
];

#[must_use]
pub fn params(m: usize) -> MfskParams {
    MfskParams::orthogonal(m, SPS)
}

#[must_use]
pub fn unique_word(m: usize) -> Vec<u8> {
    UW.iter().map(|&s| s % m as u8).collect()
}

#[must_use]
pub fn filler(m: usize, len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % m as u32) as u8
        })
        .collect()
}

#[must_use]
pub fn modulate(m: usize, symbols: &[u8]) -> Vec<Complex<f32>> {
    let mut modulator = MfskMod::new(params(m), TonePhase::Continuous);
    let mut out = Vec::new();
    modulator.modulate(symbols, &mut out);
    modulator.flush(&mut out);
    out
}

#[must_use]
pub fn find_word(symbols: &[u8], lo: usize, hi: usize, word: &[u8]) -> Option<usize> {
    let last = hi.min(symbols.len().checked_sub(word.len())?);
    (lo..=last).min_by_key(|&at| {
        word.iter()
            .enumerate()
            .filter(|&(i, &s)| symbols[at + i] != s)
            .count()
    })
}

#[must_use]
pub fn link_sized(m: usize, payload_symbols: usize) -> Link {
    let bits_per_symbol = m.trailing_zeros() as usize;
    let word = unique_word(m);
    let tx_word = word.clone();
    let demod = MfskDemod::new(params(m));
    Link {
        label: format!(
            "orthogonal {m}-FSK, spacing 1 cycle/symbol, {BAUD} baud at {RATE} Hz ({SPS} sps), \
             continuous phase -> matched tone filterbank + feedforward burst timing, \
             {LEAD}+{} symbol overhead in Eb, release",
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
            let offset = demod.estimate_offset(wave, LEAD + word.len());
            let mut symbols = Vec::new();
            demod.demodulate(
                wave,
                offset,
                LEAD + SEARCH + word.len() + payload_symbols,
                &mut symbols,
            );
            let Some(at) = find_word(&symbols, 0, LEAD + SEARCH, &word) else {
                return Vec::new();
            };
            let mut bits = Vec::with_capacity(payload_symbols * bits_per_symbol);
            for k in 0..payload_symbols {
                let symbol = symbols.get(at + word.len() + k).copied().unwrap_or(0);
                push_symbol_bits(symbol, bits_per_symbol, &mut bits);
            }
            bits
        }),
    }
}

#[must_use]
pub fn mfsk2_link() -> Link {
    link_sized(2, PAYLOAD_SYMBOLS)
}

#[must_use]
pub fn mfsk4_link() -> Link {
    link_sized(4, PAYLOAD_SYMBOLS)
}

#[must_use]
pub fn mfsk8_link() -> Link {
    link_sized(8, PAYLOAD_SYMBOLS)
}

pub const M2_GRID: &[f64] = &[7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0];
pub const M4_GRID: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
pub const M8_GRID: &[f64] = &[4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0];

pub const M2_SEED: u64 = 0x0f52;
pub const M4_SEED: u64 = 0x0f54;
pub const M8_SEED: u64 = 0x0f58;

pub const FULL_CAP: u64 = 4_000_000;

pub const ORACLE_TOLERANCE_DB: f64 = 0.4;

pub const M2_AWGN: &str = "orthogonal/mfsk2_noncoherent_awgn";
pub const M4_AWGN: &str = "orthogonal/mfsk4_noncoherent_awgn";
pub const M8_AWGN: &str = "orthogonal/mfsk8_noncoherent_awgn";
pub const M4_LIMITS: &str = "orthogonal/mfsk4_limits";
pub const PERF: &str = "orthogonal/mfsk_perf";

fn m2_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(2, ebn0_db)
}

fn m4_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(4, ebn0_db)
}

fn m8_ber(ebn0_db: f64) -> f64 {
    theory::mfsk_noncoherent_ber(8, ebn0_db)
}

const fn oracle(name: &'static str, ber: fn(f64) -> f64) -> Reference {
    Reference::Oracle {
        name,
        ber,
        tolerance_db: ORACLE_TOLERANCE_DB,
    }
}

pub const MEASUREMENTS: &[Measurement] = &[
    Measurement {
        reference: oracle("exact noncoherent orthogonal 2-FSK", m2_ber),
        ..Measurement::committed(M2_AWGN, mfsk2_link, M2_GRID, M2_SEED, FULL_CAP)
    },
    Measurement {
        reference: oracle("exact noncoherent orthogonal 4-FSK", m4_ber),
        ..Measurement::committed(M4_AWGN, mfsk4_link, M4_GRID, M4_SEED, FULL_CAP)
    },
    Measurement {
        reference: oracle("exact noncoherent orthogonal 8-FSK", m8_ber),
        ..Measurement::committed(M8_AWGN, mfsk8_link, M8_GRID, M8_SEED, FULL_CAP)
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unique_word_stays_aperiodic_at_every_alphabet() {
        for (m, worst_allowed) in [(2usize, 14), (4, 9), (8, 6)] {
            let word = unique_word(m);
            let mut context = filler(m, LEAD);
            context.extend_from_slice(&word);
            context.extend(filler(m, 32));
            let worst = (0..=LEAD + 8)
                .filter(|&at| at != LEAD)
                .map(|at| {
                    word.iter()
                        .enumerate()
                        .filter(|&(i, &s)| context[at + i] == s)
                        .count()
                })
                .max()
                .unwrap_or(0);
            assert!(
                worst <= worst_allowed,
                "M = {m}: a shifted position agrees on {worst} of {} symbols",
                word.len()
            );
        }
    }

    #[test]
    fn every_alphabet_round_trips_on_a_clean_channel() {
        for m in [2usize, 4, 8] {
            let link = link_sized(m, 64);
            let bits: Vec<bool> = (0..link.bits_per_trial).map(|i| i % 3 == 0).collect();
            let wave = (link.modulate)(&bits);
            assert_eq!((link.demodulate)(&wave), bits, "M = {m}");
        }
    }
}
