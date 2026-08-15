use num_complex::Complex;
use sdrmm_modem::cpm::CpmMod;

use crate::{
    navtex::{ccir_for, cpm_params},
    testgen::rtty::ita2_codes,
};

const BAUD: f64 = 100.0;
const CHAR_BITS: usize = 7;
const FEC_SLOTS: usize = 5;

const ALPHA: u8 = 0x0F;
const REP: u8 = 0x66;

const PHASING_PAIRS: usize = 12;

const IDLE_TAIL_PAIRS: usize = 4;

#[must_use]
pub fn encode(text: &str) -> Vec<u8> {
    ita2_codes(text, false)
        .into_iter()
        .filter_map(ccir_for)
        .collect()
}

#[must_use]
pub fn slots(codes: &[u8], phasing_pairs: usize) -> Vec<u8> {
    let base = 2 * phasing_pairs;
    let len = if codes.is_empty() {
        base
    } else {
        base + 2 * codes.len() + FEC_SLOTS - 1 + 2 * IDLE_TAIL_PAIRS
    };
    let mut slots: Vec<u8> = (0..len)
        .map(|i| if i.is_multiple_of(2) { REP } else { ALPHA })
        .collect();
    for (k, &code) in codes.iter().enumerate() {
        slots[base + 2 * k] = code;
        slots[base + 2 * k + FEC_SLOTS] = code;
    }
    slots
}

#[must_use]
pub fn bits(slots: &[u8]) -> Vec<bool> {
    let mut out = Vec::with_capacity(slots.len() * CHAR_BITS);
    for &code in slots {
        for i in 0..CHAR_BITS {
            out.push((code >> i) & 1 == 1);
        }
    }
    out
}

#[must_use]
pub fn modulate(bits: &[bool], rate: f64) -> Vec<Complex<f32>> {
    let symbols: Vec<u8> = bits.iter().map(|&bit| u8::from(bit)).collect();
    let mut modulator = CpmMod::new(cpm_params(rate));
    let mut iq = Vec::new();
    modulator.modulate(&symbols, &mut iq);
    modulator.flush(&mut iq);
    iq
}

#[must_use]
pub fn transmission(text: &str, rate: f64) -> Vec<Complex<f32>> {
    modulate(&bits(&slots(&encode(text), PHASING_PAIRS)), rate)
}

#[must_use]
pub fn phasing(seconds: f64, rate: f64) -> Vec<Complex<f32>> {
    let pairs = (seconds * BAUD / (2.0 * CHAR_BITS as f64)).max(1.0) as usize;
    modulate(&bits(&slots(&[], pairs)), rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encoded_character_is_four_of_seven() {
        for code in encode("ZCZC DA07 GALE 25 KT") {
            assert_eq!(code.count_ones(), 4, "{code:#04x}");
        }
    }

    #[test]
    fn each_character_is_repeated_five_slots_later() {
        let codes = encode("NAUTICAL");
        let stream = slots(&codes, 2);
        let base = 4;
        for (k, &code) in codes.iter().enumerate() {
            assert_eq!(stream[base + 2 * k], code, "dx copy {k}");
            assert_eq!(stream[base + 2 * k + FEC_SLOTS], code, "rx copy {k}");
        }
        assert_eq!(stream[..base], [REP, ALPHA, REP, ALPHA]);
    }

    #[test]
    fn modulation_is_unit_amplitude_at_the_standard_shift() {
        let iq = transmission("ZCZC DA07 TEST NNNN", 8_000.0);
        assert!(!iq.is_empty());
        for s in &iq {
            assert!((s.norm() - 1.0).abs() < 1e-3, "magnitude {}", s.norm());
        }
    }
}
