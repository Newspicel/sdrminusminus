use num_complex::Complex;

use super::{Coding, DecodeError};
use crate::datv::dvbs2::ldpc::{Frame, Rate};

pub fn bit_permutation(coding: Coding) -> Result<Vec<usize>, DecodeError> {
    let mut map = vec![0; coding.frame.length()];
    bit_permutation_into(coding, &mut map)?;
    Ok(map)
}

pub(super) fn bit_permutation_into(coding: Coding, map: &mut [usize]) -> Result<(), DecodeError> {
    let coding = coding.validate()?;
    let length = coding.frame.length();
    if map.len() != length {
        return Err(DecodeError::Length);
    }
    let bits = coding.constellation.bits();
    let parity_interleaved = bits > 2 || matches!(coding.rate, Rate::R1_3 | Rate::R2_5);
    let twist = twists(coding);
    let mux = demux(coding);
    let columns = mux.len();
    let rows = length / columns;
    let information = coding.information();
    let q = (length - information) / 360;
    for di in 0..length {
        let column = di % columns;
        let row = di / columns;
        let u = if bits == 2 {
            di
        } else {
            column * rows + (row + rows - twist[column]) % rows
        };
        let original = if parity_interleaved && u >= information {
            let p = u - information;
            information + q * (p % 360) + p / 360
        } else {
            u
        };
        map[row * columns + mux[column]] = original;
    }
    Ok(())
}

fn twists(coding: Coding) -> &'static [usize] {
    match (coding.constellation.bits(), coding.frame) {
        (4, Frame::Normal) => &[0, 0, 2, 4, 4, 5, 7, 7],
        (6, Frame::Normal) => &[0, 0, 2, 2, 3, 4, 4, 5, 5, 7, 8, 9],
        (8, Frame::Normal) => &[0, 2, 2, 2, 2, 3, 7, 15, 16, 20, 22, 22, 27, 27, 28, 32],
        (4 | 8, Frame::Short) => &[0, 0, 0, 1, 7, 20, 20, 21],
        (6, Frame::Short) => &[0, 0, 0, 2, 2, 2, 3, 3, 3, 6, 7, 7],
        _ => &[0, 0],
    }
}

fn demux(coding: Coding) -> &'static [usize] {
    match (coding.constellation.bits(), coding.frame, coding.rate) {
        (2, _, _) => &[0, 1],
        (4, Frame::Normal, Rate::R3_5) => &[0, 5, 1, 2, 4, 7, 3, 6],
        (6, Frame::Normal, Rate::R3_5) => &[2, 7, 6, 9, 0, 3, 1, 8, 4, 11, 5, 10],
        (8, Frame::Normal, Rate::R3_5) => &[2, 11, 3, 4, 0, 9, 1, 8, 10, 13, 7, 14, 6, 15, 5, 12],
        (8, Frame::Normal, Rate::R2_3) => &[7, 2, 9, 0, 4, 6, 13, 3, 14, 10, 15, 5, 8, 12, 11, 1],
        (4, Frame::Short, Rate::R1_3) => &[6, 0, 3, 4, 5, 2, 1, 7],
        (6, Frame::Short, Rate::R1_3) => &[4, 2, 0, 5, 6, 1, 3, 7, 8, 9, 10, 11],
        (8, Frame::Short, Rate::R1_3) => &[4, 0, 1, 2, 5, 3, 6, 7],
        (4, Frame::Short, Rate::R2_5) => &[7, 5, 4, 0, 3, 1, 2, 6],
        (6, Frame::Short, Rate::R2_5) => &[4, 0, 1, 6, 2, 3, 5, 8, 7, 10, 9, 11],
        (8, Frame::Short, Rate::R2_5) => &[4, 0, 5, 1, 2, 3, 6, 7],
        (4, _, _) => &[7, 1, 4, 2, 5, 3, 6, 0],
        (6, _, _) => &[11, 7, 3, 10, 6, 2, 9, 5, 1, 8, 4, 0],
        (8, Frame::Short, _) => &[7, 3, 1, 5, 2, 6, 4, 0],
        (8, _, _) => &[15, 1, 13, 3, 8, 11, 9, 5, 10, 6, 4, 7, 12, 2, 14, 0],
        _ => &[],
    }
}

pub fn cell_permutation(cells: usize) -> Result<Vec<usize>, DecodeError> {
    let mut out = vec![0; cells];
    cell_permutation_into(&mut out)?;
    Ok(out)
}

pub(super) fn cell_permutation_into(out: &mut [usize]) -> Result<(), DecodeError> {
    let cells = out.len();
    if ![2025, 2700, 4050, 8100, 10800, 16200, 32400].contains(&cells) {
        return Err(DecodeError::Parameters);
    }
    let degree = cells.next_power_of_two().ilog2() as usize;
    let taps: &[usize] = match degree {
        11 => &[0, 3],
        12 => &[0, 2],
        13 => &[0, 1, 4, 6],
        14 => &[0, 1, 4, 5, 9, 11],
        _ => &[0, 1, 2, 12],
    };
    let mut state = 0;
    let mut next = 0;
    for i in 0..1 << degree {
        state = match i {
            0 | 1 => 0,
            2 => 1,
            _ => {
                state >> 1
                    | (taps.iter().fold(0, |bit, &tap| bit ^ (state >> tap & 1)) << (degree - 2))
            }
        };
        let address = state | (i % 2) << (degree - 1);
        if address < cells {
            out[next] = address;
            next += 1;
        }
    }
    Ok(())
}

pub fn cell_shift(cells: usize, block: usize) -> Result<usize, DecodeError> {
    if cells == 0
        || block >= 1023
        || ![2025, 2700, 4050, 8100, 10800, 16200, 32400].contains(&cells)
    {
        return Err(DecodeError::Parameters);
    }
    let degree = cells.next_power_of_two().ilog2();
    (0..cells.next_power_of_two())
        .map(|counter| counter.reverse_bits() >> (usize::BITS - degree))
        .filter(|&address| address < cells)
        .nth(block)
        .ok_or(DecodeError::Parameters)
}

pub fn time_deinterleave(
    input: &[Complex<f32>],
    output: &mut [Complex<f32>],
    cells: usize,
) -> Result<(), DecodeError> {
    if cells == 0
        || !cells.is_multiple_of(5)
        || !input.len().is_multiple_of(cells)
        || input.len() != output.len()
    {
        return Err(DecodeError::Length);
    }
    let rows = cells / 5;
    let columns = input.len() / rows;
    for (i, &value) in input.iter().enumerate() {
        output[(i % columns) * rows + i / columns] = value;
    }
    Ok(())
}

pub fn time_block_size(blocks: usize, count: usize, index: usize) -> Result<usize, DecodeError> {
    if count == 0 || count > 255 || index >= count || blocks > 1023 {
        return Err(DecodeError::Parameters);
    }
    Ok(blocks / count + usize::from(index >= count - blocks % count))
}
