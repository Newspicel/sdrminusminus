use std::f64::consts::TAU;

use num_complex::Complex;

use super::{
    demod::{BAUD, PHASING},
    symbol::{LEADING_DX_PHASING, RX_DELAY, zero_count},
};

pub const SHIFT_HZ: f64 = 85.0;
const DOT_PAIRS: usize = 40;
const FIRST_RX_PHASING: i32 = 111;

pub fn symbol_to_bits(value: i32, out: &mut Vec<u8>) {
    let value = value as u8;
    out.extend((0..7).map(|bit| (value >> bit) & 1));
    let check = zero_count(value);
    out.extend([(check >> 2) & 1, (check >> 1) & 1, check & 1]);
}

pub fn frame_bits(data_symbols: &[i32]) -> Vec<u8> {
    let length = LEADING_DX_PHASING + data_symbols.len();
    let mut bits = Vec::with_capacity(length * 20);
    for index in 0..length {
        let dx = index
            .checked_sub(LEADING_DX_PHASING)
            .map_or(i32::from(PHASING), |data| data_symbols[data]);
        symbol_to_bits(dx, &mut bits);
        symbol_to_bits(i32::from(PHASING), &mut bits);
    }
    bits
}

pub fn m493_bits(data_symbols: &[i32]) -> Vec<u8> {
    let rx_phasing = LEADING_DX_PHASING + RX_DELAY;
    let length = rx_phasing + data_symbols.len();
    let mut bits = Vec::with_capacity(length * 20);
    for index in 0..length {
        let dx = match index.checked_sub(LEADING_DX_PHASING) {
            None => i32::from(PHASING),
            Some(data) => data_symbols
                .get(data)
                .copied()
                .unwrap_or(i32::from(PHASING)),
        };
        let rx = match index.checked_sub(rx_phasing) {
            None => FIRST_RX_PHASING - index as i32,
            Some(data) => data_symbols[data],
        };
        symbol_to_bits(dx, &mut bits);
        symbol_to_bits(rx, &mut bits);
    }
    bits
}

pub fn modulate_iq(
    bits: &[u8],
    sample_rate: f64,
    freq_offset_hz: f64,
    shift_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    let samples_per_bit = sample_rate / BAUD;
    let mut iq = Vec::with_capacity((bits.len() as f64 * samples_per_bit) as usize + 1);
    let mut phase = 0.0f64;
    for (index, &bit) in bits.iter().enumerate() {
        let freq = if bit != 0 {
            freq_offset_hz + shift_hz
        } else {
            freq_offset_hz - shift_hz
        };
        let end = (((index + 1) as f64) * samples_per_bit).round() as usize;
        while iq.len() < end {
            phase += TAU * freq / sample_rate;
            iq.push(Complex::new(phase.cos() as f32, phase.sin() as f32) * amplitude);
        }
    }
    iq
}

fn with_dots(frame: Vec<u8>) -> Vec<u8> {
    let mut bits: Vec<u8> = (0..DOT_PAIRS).flat_map(|_| [1, 0]).collect();
    bits.extend(frame);
    bits
}

pub fn call_iq(
    data_symbols: &[i32],
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    modulate_iq(
        &with_dots(frame_bits(data_symbols)),
        sample_rate,
        freq_offset_hz,
        SHIFT_HZ,
        amplitude,
    )
}

pub fn m493_call_iq(
    data_symbols: &[i32],
    sample_rate: f64,
    freq_offset_hz: f64,
    amplitude: f32,
) -> Vec<Complex<f32>> {
    modulate_iq(
        &with_dots(m493_bits(data_symbols)),
        sample_rate,
        freq_offset_hz,
        SHIFT_HZ,
        amplitude,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsc::{
        decode_from_bits,
        symbol::{ERASURE, decode_bitstream, deinterleave_dx_rx},
    };

    const CALL: &[i32] = &[
        112, 112, 25, 58, 5, 99, 70, 107, 4, 52, 60, 13, 7, 12, 52, 109, 127, 52, 127, 127,
    ];

    #[test]
    fn frame_bits_round_trip() {
        assert!(decode_from_bits(&frame_bits(CALL)).ecc_ok());
        assert!(decode_from_bits(&m493_bits(CALL)).ecc_ok());
    }

    #[test]
    fn m493_rx_stream_repeats_every_data_symbol() {
        let mut chars = decode_bitstream(&m493_bits(CALL));
        for dx in chars.iter_mut().step_by(2) {
            *dx = ERASURE;
        }
        let symbols = deinterleave_dx_rx(&chars, LEADING_DX_PHASING, RX_DELAY);
        assert_eq!(&symbols[..CALL.len()], CALL);
    }
}
