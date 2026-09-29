use num_complex::Complex;
use sdrmm_modem::cpm::CpmMod;

use crate::rtty::{FIGS_CODE, FIGURES, LETTERS, LTRS_CODE, SPACE_CODE, cell_params};

const LEAD_IN_BITS: usize = 8;
const LEAD_OUT_BITS: usize = 4;

fn code_for(ch: char) -> Option<(u8, Option<bool>)> {
    let find = |row: &[char; 32]| row.iter().position(|&c| c == ch && c != '\0');
    match (find(&LETTERS), find(&FIGURES)) {
        (Some(code), Some(_)) => Some((code as u8, None)),
        (Some(code), None) => Some((code as u8, Some(false))),
        (None, Some(code)) => Some((code as u8, Some(true))),
        (None, None) => None,
    }
}

#[must_use]
pub fn ita2_codes(text: &str, unshift_on_space: bool) -> Vec<u8> {
    let mut codes = Vec::new();
    let mut figs = false;
    for ch in text.chars() {
        let Some((code, row)) = code_for(ch.to_ascii_uppercase()) else {
            continue;
        };
        if let Some(row) = row
            && row != figs
        {
            codes.push(if row { FIGS_CODE } else { LTRS_CODE });
            figs = row;
        }
        codes.push(code);
        if code == SPACE_CODE && unshift_on_space {
            figs = false;
        }
    }
    codes
}

#[must_use]
pub fn encode(text: &str, stop_bits: f64) -> Vec<bool> {
    encode_codes(&ita2_codes(text, true), stop_bits)
}

#[must_use]
pub fn encode_codes(codes: &[u8], stop_bits: f64) -> Vec<bool> {
    let stop_cells = (stop_bits * 2.0).round().max(2.0) as usize;
    let mut cells = Vec::with_capacity(codes.len() * (12 + stop_cells));
    for &code in codes {
        cells.extend_from_slice(&[false, false]);
        for i in 0..5 {
            let bit = (code >> i) & 1 == 1;
            cells.extend_from_slice(&[bit, bit]);
        }
        cells.extend(std::iter::repeat_n(true, stop_cells));
    }
    cells
}

#[must_use]
pub fn modulate(cells: &[bool], baud: f64, shift_hz: f64, rate: f64) -> Vec<Complex<f32>> {
    let mut keyed = vec![true; LEAD_IN_BITS * 2];
    keyed.extend_from_slice(cells);
    keyed.extend(std::iter::repeat_n(true, LEAD_OUT_BITS * 2));
    let symbols: Vec<u8> = keyed.iter().map(|&cell| u8::from(cell)).collect();
    let mut modulator = CpmMod::new(cell_params(baud, shift_hz, rate));
    let mut iq = Vec::new();
    modulator.modulate(&symbols, &mut iq);
    modulator.flush(&mut iq);
    iq
}

#[must_use]
pub fn transmission(
    text: &str,
    baud: f64,
    shift_hz: f64,
    stop_bits: f64,
    rate: f64,
) -> Vec<Complex<f32>> {
    modulate(&encode(text, stop_bits), baud, shift_hz, rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_letter_frames_as_start_five_data_lsb_first_and_stop() {
        let cells = encode_codes(&[0x03], 1.0);
        assert_eq!(
            cells,
            [
                false, false, true, true, true, true, false, false, false, false, false, false,
                true, true
            ]
        );
    }

    #[test]
    fn stop_length_is_carried_at_half_bit_resolution() {
        for (stop_bits, cells) in [(1.0, 14), (1.5, 15), (2.0, 16)] {
            assert_eq!(encode_codes(&[0x03], stop_bits).len(), cells, "{stop_bits}");
        }
    }

    #[test]
    fn shifts_are_inserted_only_when_the_row_changes() {
        let cells = encode("a1 2", 1.0);
        let (frames, _) = cells.as_chunks::<14>();
        let codes: Vec<u8> = frames
            .iter()
            .map(|frame| (0..5).map(|i| u8::from(frame[2 + 2 * i]) << i).sum::<u8>())
            .collect();
        assert_eq!(codes, [0x03, FIGS_CODE, 0x17, SPACE_CODE, FIGS_CODE, 0x13]);
    }

    #[test]
    fn modulation_is_unit_amplitude_and_symmetric_about_dc() {
        let iq = transmission("RY", 45.45, 170.0, 1.5, 8_000.0);
        assert!(!iq.is_empty());
        for s in &iq {
            assert!((s.norm() - 1.0).abs() < 1e-3, "magnitude {}", s.norm());
        }
        let turn = (iq[100] * iq[99].conj()).arg() as f64 * 8_000.0 / std::f64::consts::TAU;
        assert!((turn - 85.0).abs() < 1.0, "idle tone {turn} Hz");
    }
}
