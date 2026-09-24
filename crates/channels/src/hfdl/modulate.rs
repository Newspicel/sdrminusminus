use std::f64::consts::{PI, TAU};

use num_complex::Complex;
use sdrmm_dsp::SoftViterbi;

use super::{
    demod::SYMBOL_RATE,
    fec::{self, A, SEGMENT_SYMBOLS, SEQUENCE_LEN, Setting, T, TRAINING_LEN},
};

const PREKEY_SYMBOLS: usize = 448;

fn gray_inv(g: u32) -> u32 {
    g ^ (g >> 1) ^ (g >> 2)
}

fn bpsk(bit: u8) -> f64 {
    if bit == 1 { PI } else { 0.0 }
}

fn coded_air_chips(payload: &[u8], setting: &Setting) -> Vec<u8> {
    let mut bits: Vec<u8> = payload
        .iter()
        .flat_map(|&b| (0..8).map(move |i| (b >> i) & 1))
        .collect();
    assert!(bits.len() <= setting.payload_bits());
    bits.resize(setting.payload_bits(), 0);
    let mut chips = SoftViterbi::new(7, 0o133, 0o171).encode(&bits);
    if setting.rate_quarter {
        chips = chips.iter().flat_map(|&c| [c, c]).collect();
    }
    fec::interleave(&chips, setting)
}

pub fn burst_symbols(payload: &[u8], setting: &Setting) -> Vec<f64> {
    let air = coded_air_chips(payload, setting);
    let mut symbols = vec![0.0; PREKEY_SYMBOLS];
    for _ in 0..2 {
        symbols.extend(A.iter().map(|&b| bpsk(b)));
    }
    symbols.extend((0..SEQUENCE_LEN + TRAINING_LEN).map(|j| bpsk(fec::m1_chip(setting, j))));
    for _ in 0..9 {
        symbols.extend(T.iter().map(|&b| bpsk(b)));
    }
    let bits = setting.bits_per_symbol as usize;
    let mut labels = air.chunks_exact(bits).map(|group| {
        group
            .iter()
            .fold(0u32, |label, &chip| (label << 1) | u32::from(chip))
    });
    let mut data_index = 0;
    for _ in 0..setting.data_segments() {
        for _ in 0..SEGMENT_SYMBOLS {
            let label = labels.next().unwrap_or_default();
            let mut phase = TAU * f64::from(gray_inv(label)) / f64::from(1u32 << bits);
            if fec::scramble_flip(data_index) {
                phase += PI;
            }
            symbols.push(phase);
            data_index += 1;
        }
        symbols.extend(T.iter().map(|&b| bpsk(b)));
    }
    symbols
}

pub fn modulate(
    symbols: &[f64],
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let sps = sample_rate / SYMBOL_RATE;
    let n = (symbols.len() as f64 * sps).ceil() as usize;
    (0..n)
        .map(|k| {
            let symbol = ((k as f64 / sps) as usize).min(symbols.len() - 1);
            let phase = symbols[symbol] + TAU * freq_offset_hz * k as f64 / sample_rate;
            Complex::new(phase.cos() as f32, phase.sin() as f32) * amplitude
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hfdl::{fec::SETTINGS, pdu::build::acars_mpdu};

    #[test]
    fn symbols_match_xng() {
        let payload = acars_mpdu();
        for (ours, theirs) in SETTINGS.iter().zip(&xng_mode_hfdl::fec::SETTINGS) {
            if payload.len() > ours.payload_bytes() {
                continue;
            }
            assert_eq!(
                burst_symbols(&payload, ours),
                xng_mode_hfdl::modulate::burst_symbols(&payload, theirs)
            );
        }
    }
}
