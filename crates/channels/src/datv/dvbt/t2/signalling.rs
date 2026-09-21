use num_complex::Complex;

use super::{Constellation, DecodeError, Frame, Rate, acquire::Preamble, bicm};
use crate::datv::dvbs2::{
    bb,
    bch::{Bch, BchScratch},
    ldpc::Ldpc,
};

mod fields;
pub use fields::{Plp, Post, Pre};

const PRE_SHORTEN: [usize; 9] = [7, 3, 6, 5, 2, 4, 1, 8, 0];
const PRE_PUNCTURE: [usize; 36] = [
    27, 13, 29, 32, 5, 0, 11, 21, 33, 20, 25, 28, 18, 35, 8, 3, 9, 31, 22, 24, 7, 14, 17, 4, 2, 26,
    16, 34, 19, 10, 12, 23, 1, 6, 30, 15,
];
const POST_SHORTEN: [[usize; 20]; 3] = [
    [
        18, 17, 16, 15, 14, 13, 12, 11, 4, 10, 9, 8, 3, 2, 7, 6, 5, 1, 19, 0,
    ],
    [
        18, 17, 16, 15, 14, 13, 12, 11, 4, 10, 9, 8, 7, 3, 2, 1, 6, 5, 19, 0,
    ],
    [
        18, 17, 16, 4, 15, 14, 13, 12, 3, 11, 10, 9, 2, 8, 7, 1, 6, 5, 19, 0,
    ],
];
const POST_PUNCTURE: [[usize; 25]; 3] = [
    [
        6, 4, 18, 9, 13, 8, 15, 20, 5, 17, 2, 24, 10, 22, 12, 3, 16, 23, 1, 14, 0, 21, 19, 7, 11,
    ],
    [
        6, 4, 13, 9, 18, 8, 15, 20, 5, 17, 2, 22, 24, 7, 12, 1, 16, 23, 14, 0, 21, 10, 19, 11, 3,
    ],
    [
        6, 15, 13, 10, 3, 17, 21, 8, 5, 19, 2, 23, 16, 24, 7, 18, 1, 12, 20, 0, 4, 14, 9, 11, 22,
    ],
];
const DEMUX16: [usize; 8] = [7, 1, 4, 2, 5, 3, 6, 0];
const DEMUX64: [usize; 12] = [11, 7, 3, 10, 6, 2, 9, 5, 1, 8, 4, 0];

struct Fec {
    ldpc: Ldpc,
    bch: Bch,
    scratch: BchScratch,
    information: usize,
}

impl Fec {
    fn new(rate: Rate) -> Result<Self, DecodeError> {
        let information = rate.information(Frame::Short);
        let bch = Bch::new(Frame::Short, 12, information - 168);
        Ok(Self {
            ldpc: Ldpc::new(rate, Frame::Short).ok_or(DecodeError::Parameters)?,
            scratch: bch.scratch(),
            bch,
            information,
        })
    }
}

pub struct Signalling {
    pre: Fec,
    post: Fec,
    llrs: Vec<f32>,
    demapped: Vec<f32>,
    omitted: Vec<bool>,
    word: Vec<bool>,
    decoded: Vec<bool>,
    points: [Vec<Complex<f32>>; 3],
}

impl Signalling {
    pub fn new() -> Result<Self, DecodeError> {
        Ok(Self {
            pre: Fec::new(Rate::R1_4)?,
            post: Fec::new(Rate::R1_2)?,
            llrs: vec![0.0; 16200],
            demapped: vec![0.0; 16200],
            omitted: vec![false; 16200],
            word: Vec::with_capacity(7200),
            decoded: vec![false; 262144 + 7032],
            points: [
                Constellation::Qpsk,
                Constellation::Qam16,
                Constellation::Qam64,
            ]
            .map(|c| (0..1 << c.bits()).map(|w| bicm::point(w, c)).collect()),
        })
    }

    pub fn pre(&mut self, cells: &[Complex<f32>], preamble: Preamble) -> Result<Pre, DecodeError> {
        if cells.len() != 1840 {
            return Err(DecodeError::Length);
        }
        self.demap(cells, 1)?;
        Self::decode(
            &mut self.pre,
            &self.demapped[..1840],
            &mut self.llrs,
            &mut self.omitted,
            &mut self.word,
            &mut self.decoded[..200],
            &PRE_SHORTEN,
            &PRE_PUNCTURE,
        )?;
        Pre::parse(&self.decoded[..200], preamble)
    }

    pub fn post(&mut self, cells: &[Complex<f32>], pre: Pre) -> Result<Post, DecodeError> {
        let (blocks, information, transmitted) = pre.post_shape()?;
        if cells.len() != pre.post_cells {
            return Err(DecodeError::Length);
        }
        let table = pre.modulation.saturating_sub(1) as usize;
        for block in 0..blocks {
            let n = transmitted / pre.bits();
            self.demap(&cells[block * n..(block + 1) * n], pre.bits())?;
            let decoded = &mut self.decoded[block * information..(block + 1) * information];
            Self::decode(
                &mut self.post,
                &self.demapped[..transmitted],
                &mut self.llrs,
                &mut self.omitted,
                &mut self.word,
                decoded,
                &POST_SHORTEN[table],
                &POST_PUNCTURE[table],
            )?;
            if pre.scrambled {
                bb::scramble(decoded);
            }
        }
        Post::parse(&self.decoded[..pre.post_info + 32], pre)
    }

    fn demap(&mut self, cells: &[Complex<f32>], bits: usize) -> Result<(), DecodeError> {
        if cells.len() * bits > self.demapped.len() {
            return Err(DecodeError::Length);
        }
        let columns = if bits > 2 { bits * 2 } else { 1 };
        let rows = cells.len() * bits / columns;
        if rows * columns != cells.len() * bits {
            return Err(DecodeError::Length);
        }
        for (i, &cell) in cells.iter().enumerate() {
            if !cell.norm_sqr().is_finite() {
                return Err(DecodeError::NonFinite);
            }
            let soft = if bits == 1 {
                [cell.re * 16.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
            } else {
                bicm::soften(cell, &self.points[bits / 2 - 1], bits, 0.05)
            };
            for (b, &llr) in soft[..bits].iter().enumerate() {
                let address = if bits <= 2 {
                    i * bits + b
                } else {
                    let demux = if bits == 4 {
                        &DEMUX16[..]
                    } else {
                        &DEMUX64[..]
                    };
                    let output = i % 2 * bits + b;
                    let column = demux
                        .iter()
                        .position(|&v| v == output)
                        .ok_or(DecodeError::Parameters)?;
                    column * rows + i / 2
                };
                self.demapped[address] = llr;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn decode(
        fec: &mut Fec,
        input: &[f32],
        llrs: &mut [f32],
        omitted: &mut [bool],
        word: &mut Vec<bool>,
        output: &mut [bool],
        shorten: &[usize],
        puncture: &[usize],
    ) -> Result<(), DecodeError> {
        let message = fec.information - 168;
        if output.is_empty()
            || output.len() > message
            || input.len() > output.len() + 16200 - message
        {
            return Err(DecodeError::Length);
        }
        omitted.fill(false);
        llrs.fill(0.0);
        shortening(&mut omitted[..message], output.len(), shorten)?;
        let punctured = output.len() + 16200 - message - input.len();
        if punctured >= 16200 - fec.information {
            return Err(DecodeError::Parameters);
        }
        for i in 0..punctured {
            let group = puncture[i / 360];
            omitted[fec.information + (i % 360) * puncture.len() + group] = true;
        }
        let mut source = input.iter();
        for i in 0..16200 {
            llrs[i] = if omitted[i] {
                if i < message { 32.0 } else { 0.0 }
            } else {
                *source.next().ok_or(DecodeError::Length)?
            };
        }
        if source.next().is_some() {
            return Err(DecodeError::Length);
        }
        word.clear();
        if fec.ldpc.decode_with_iterations(llrs, word, 100).is_none() {
            word.extend_from_slice(fec.ldpc.hard_information());
        }
        fec.bch
            .decode_with_scratch(word, &mut fec.scratch)
            .ok_or(DecodeError::Bch)?;
        let mut target = output.iter_mut();
        for i in 0..message {
            if !omitted[i] {
                *target.next().ok_or(DecodeError::Length)? = word[i];
            }
        }
        Ok(())
    }
}

fn shortening(mask: &mut [bool], information: usize, order: &[usize]) -> Result<(), DecodeError> {
    if information == 0 || information > mask.len() {
        return Err(DecodeError::Parameters);
    }
    let groups = if information <= 360 {
        order.len() - 1
    } else {
        (mask.len() - information) / 360
    };
    for &group in &order[..groups] {
        let end = ((group + 1) * 360).min(mask.len());
        mask[group * 360..end].fill(true);
    }
    let remaining = if groups == order.len() - 1 {
        360 - information
    } else {
        mask.len() - information - 360 * groups
    };
    if remaining > 0 {
        let end = ((order[groups] + 1) * 360).min(mask.len());
        mask[end - remaining..end].fill(true);
    }
    Ok(())
}

pub fn crc(bits: &[bool]) -> u32 {
    bits.iter().fold(u32::MAX, |mut state, &bit| {
        let feedback = bit ^ (state >> 31 != 0);
        state <<= 1;
        if feedback {
            state ^= 0x04c11db7;
        }
        state
    })
}

#[cfg(test)]
mod tests;
