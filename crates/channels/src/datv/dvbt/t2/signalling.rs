use num_complex::Complex;

use super::{
    Constellation, DecodeError, Frame, Rate,
    acquire::Preamble,
    bicm,
    en302755::{
        demux,
        l1::{POST_PADDING_ORDER, POST_PUNCTURING_ORDER, PRE_PADDING_ORDER, PRE_PUNCTURING_ORDER},
    },
};
use crate::datv::dvbs2::{
    bb,
    bch::{Bch, BchScratch},
    ldpc::Ldpc,
};

mod fields;
pub use fields::{Plp, Post, Pre};

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
            &PRE_PADDING_ORDER,
            &PRE_PUNCTURING_ORDER,
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
                &POST_PADDING_ORDER[table],
                &POST_PUNCTURING_ORDER[table],
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
                        &demux::QAM16[..]
                    } else {
                        &demux::QAM64[..]
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
