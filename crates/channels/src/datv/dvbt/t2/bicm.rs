use num_complex::Complex;

use super::{Coding, Constellation, DecodeError, interleave};
use crate::datv::dvbs2::{
    bb,
    bch::{Bch, BchScratch},
    ldpc::Ldpc,
};

pub struct Decoder {
    coding: Coding,
    cell_map: Vec<usize>,
    bit_map: Vec<usize>,
    points: Vec<Complex<f32>>,
    llrs: Vec<f32>,
    word: Vec<bool>,
    ldpc: Ldpc,
    bch: Bch,
    bch_scratch: BchScratch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub bits: usize,
    pub ldpc_iterations: usize,
    pub corrected_bits: usize,
}

impl Decoder {
    pub fn new(coding: Coding) -> Result<Self, DecodeError> {
        let coding = coding.validate()?;
        let bch = Bch::new(coding.frame, coding.correct(), coding.message());
        let bch_scratch = bch.scratch();
        let mut cell_map = interleave::cell_permutation(coding.cells())?;
        cell_map.reserve(coding.frame.length() / 2 - cell_map.len());
        let mut points = Vec::with_capacity(256);
        points.extend(
            (0..1 << coding.constellation.bits()).map(|word| point(word, coding.constellation)),
        );
        Ok(Self {
            coding,
            cell_map,
            bit_map: interleave::bit_permutation(coding)?,
            points,
            llrs: vec![0.0; coding.frame.length()],
            word: Vec::with_capacity(coding.information()),
            ldpc: Ldpc::with_addresses(coding.frame, coding.addresses()?)
                .ok_or(DecodeError::Parameters)?,
            bch,
            bch_scratch,
        })
    }

    pub fn configure(&mut self, coding: Coding) -> Result<(), DecodeError> {
        let coding = coding.validate()?;
        if coding.frame != self.coding.frame || coding.rate != self.coding.rate {
            return Err(DecodeError::Parameters);
        }
        if coding == self.coding {
            return Ok(());
        }
        self.cell_map.resize(coding.cells(), 0);
        interleave::cell_permutation_into(&mut self.cell_map)?;
        interleave::bit_permutation_into(coding, &mut self.bit_map)?;
        self.points.clear();
        self.points.extend(
            (0..1 << coding.constellation.bits()).map(|word| point(word, coding.constellation)),
        );
        self.coding = coding;
        Ok(())
    }

    pub fn decode(
        &mut self,
        cells: &[Complex<f32>],
        block: usize,
        noise_variance: f32,
        output: &mut [bool],
    ) -> Result<Decoded, DecodeError> {
        if cells.len() != self.coding.cells() || output.len() < self.coding.message() {
            return Err(DecodeError::Length);
        }
        if !noise_variance.is_finite() || noise_variance <= 0.0 {
            return Err(DecodeError::Parameters);
        }
        if cells.iter().any(|p| !p.norm_sqr().is_finite()) {
            return Err(DecodeError::NonFinite);
        }
        let shift = interleave::cell_shift(cells.len(), block)?;
        let rotation = Complex::from_polar(1.0, -self.coding.constellation.rotation());
        let bits = self.coding.constellation.bits();
        for i in 0..cells.len() {
            let p = cells[(self.cell_map[i] + shift) % cells.len()];
            let p = if self.coding.rotated {
                let next = cells[(self.cell_map[(i + 1) % cells.len()] + shift) % cells.len()];
                Complex::new(p.re, next.im) * rotation
            } else {
                p
            };
            let soft = soften(p, &self.points, bits, noise_variance);
            for (bit, &llr) in soft[..bits].iter().enumerate() {
                self.llrs[self.bit_map[i * bits + bit]] = llr;
            }
        }
        self.word.clear();
        let iterations = self
            .ldpc
            .decode(&self.llrs, &mut self.word)
            .ok_or(DecodeError::Ldpc)?;
        let corrected = self
            .bch
            .decode_with_scratch(&mut self.word, &mut self.bch_scratch)
            .ok_or(DecodeError::Bch)?;
        let length = self.coding.message();
        output[..length].copy_from_slice(&self.word[..length]);
        bb::scramble(&mut output[..length]);
        Ok(Decoded {
            bits: length,
            ldpc_iterations: iterations,
            corrected_bits: corrected,
        })
    }
}

pub fn point(word: usize, constellation: Constellation) -> Complex<f32> {
    let bits = constellation.bits();
    let dimension = bits / 2;
    let component = |axis| {
        let bit = |position| word >> (bits - 1 - 2 * position - axis) & 1;
        let mut amplitude = 1;
        for position in (1..dimension).rev() {
            amplitude = (1 << (dimension - position)) + (1 - 2 * bit(position) as i32) * amplitude;
        }
        (1 - 2 * bit(0) as i32) as f32 * amplitude as f32
    };
    let scale = ((2 * ((1 << bits) - 1)) as f32 / 3.0).sqrt();
    Complex::new(component(0), component(1)) / scale
}

pub(super) fn soften(
    sample: Complex<f32>,
    points: &[Complex<f32>],
    bits: usize,
    variance: f32,
) -> [f32; 8] {
    let mut distances = [[f32::INFINITY; 2]; 8];
    let dimension = bits / 2;
    for level in 0..1 << dimension {
        let word = (0..dimension).fold(0, |word, bit| word | ((level >> bit & 1) << (2 * bit + 1)));
        let amplitude = points[word].re;
        let real = (sample.re - amplitude).powi(2);
        let imaginary = (sample.im - amplitude).powi(2);
        for bit in 0..dimension {
            let value = level >> (dimension - 1 - bit) & 1;
            distances[2 * bit][value] = distances[2 * bit][value].min(real);
            distances[2 * bit + 1][value] = distances[2 * bit + 1][value].min(imaginary);
        }
    }
    let mut soft = [0.0; 8];
    for (out, distance) in soft[..bits].iter_mut().zip(&distances) {
        *out = ((distance[1] - distance[0]) / variance).clamp(-32.0, 32.0);
    }
    soft
}
