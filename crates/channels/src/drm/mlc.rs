use num_complex::Complex;
use sdrmm_dsp::{ConvCode, DAB_DISPERSAL, Prbs, Soft};

use super::coding::{LevelDecoder, MOTHER, Plan, Qam, encode_level, permutation};

const TARGET_SOFT: f32 = 24.0;
const MAX_SOFT: f32 = 127.0;

pub fn disperse(bits: &mut [bool]) {
    Prbs::new(DAB_DISPERSAL).apply_bits(bits);
}

fn metric(x: f32, levels: usize, level: usize, known: u8) -> f32 {
    let points = 1usize << levels;
    let mask = (1usize << level) - 1;
    let mut best = [f32::MAX; 2];
    for index in 0..points {
        if index & mask != usize::from(known) & mask {
            continue;
        }
        let value = (points - 1) as f32 - 2.0 * index as f32;
        let distance = (x - value) * (x - value);
        let bit = index >> level & 1;
        best[bit] = best[bit].min(distance);
    }
    best[0] - best[1]
}

pub struct MlcDecoder {
    level: LevelDecoder,
    code: ConvCode,
    llr: Vec<f32>,
    soft: Vec<Soft>,
    coded: Vec<Soft>,
    order: Vec<usize>,
    known: Vec<u8>,
    decoded: Vec<bool>,
    encoded: Vec<bool>,
}

impl Default for MlcDecoder {
    fn default() -> Self {
        Self {
            level: LevelDecoder::default(),
            code: ConvCode::new(&MOTHER),
            llr: Vec::new(),
            soft: Vec::new(),
            coded: Vec::new(),
            order: Vec::new(),
            known: Vec::new(),
            decoded: Vec::new(),
            encoded: Vec::new(),
        }
    }
}

impl MlcDecoder {
    fn parts(plan: &Plan) -> [(usize, usize); 2] {
        [
            (0, 2 * plan.higher_cells),
            (2 * plan.higher_cells, 2 * plan.lower_cells),
        ]
    }

    fn soft_level(&mut self, plan: &Plan, level: usize, cells: &[Complex<f32>], weights: &[f32]) {
        let scale = plan.qam.scale();
        let levels = plan.qam.levels();
        self.llr.clear();
        for (index, (cell, &weight)) in cells.iter().zip(weights).enumerate() {
            for (dimension, value) in [cell.re, cell.im].into_iter().enumerate() {
                let known = self.known[2 * index + dimension];
                self.llr
                    .push(weight * metric(value / scale, levels, level, known));
            }
        }
        let mean =
            self.llr.iter().map(|value| value.abs()).sum::<f32>() / self.llr.len().max(1) as f32;
        let gain = if mean > f32::EPSILON {
            TARGET_SOFT / mean
        } else {
            0.0
        };
        self.soft.clear();
        self.soft.extend(
            self.llr
                .iter()
                .map(|value| (value * gain).round().clamp(-MAX_SOFT, MAX_SOFT) as Soft),
        );
    }

    fn deinterleave(&mut self, plan: &Plan, level: usize) {
        self.coded.clear();
        self.coded.extend_from_slice(&self.soft);
        let Some(t) = plan.qam.interleaver(level) else {
            return;
        };
        for (start, length) in Self::parts(plan) {
            permutation(length, t, &mut self.order);
            for (index, &source) in self.order.iter().enumerate() {
                self.coded[start + source] = self.soft[start + index];
            }
        }
    }

    fn remember(&mut self, plan: &Plan, level: usize) {
        encode_level(
            &plan.levels[level],
            &self.decoded,
            &self.code,
            &mut self.encoded,
        );
        let interleaver = plan.qam.interleaver(level);
        for (start, length) in Self::parts(plan) {
            match interleaver {
                Some(t) => {
                    permutation(length, t, &mut self.order);
                    for (index, &source) in self.order.iter().enumerate() {
                        self.known[start + index] |=
                            u8::from(self.encoded[start + source]) << level;
                    }
                }
                None => {
                    for index in start..start + length {
                        self.known[index] |= u8::from(self.encoded[index]) << level;
                    }
                }
            }
        }
    }

    pub fn decode(
        &mut self,
        plan: &Plan,
        cells: &[Complex<f32>],
        weights: &[f32],
        out: &mut Vec<bool>,
    ) -> Option<()> {
        if cells.len() != plan.cells() || weights.len() != cells.len() {
            return None;
        }
        self.known.clear();
        self.known.resize(2 * cells.len(), 0);
        out.clear();
        out.resize(plan.bits(), false);
        for level in 0..plan.qam.levels() {
            self.soft_level(plan, level, cells, weights);
            self.deinterleave(plan, level);
            self.level
                .decode(&plan.levels[level], &self.coded, &mut self.decoded)?;
            plan.merge(level, &self.decoded, out);
            if level + 1 < plan.qam.levels() {
                self.remember(plan, level);
            }
        }
        disperse(out);
        Some(())
    }
}

#[must_use]
pub fn point(qam: Qam, bits: &[u8]) -> f32 {
    let levels = qam.levels();
    let index = bits
        .iter()
        .enumerate()
        .fold(0usize, |index, (level, &bit)| {
            index | usize::from(bit) << level
        });
    (((1usize << levels) - 1) as f32 - 2.0 * index as f32) * qam.scale()
}

#[cfg(any(test, feature = "synth"))]
pub fn encode(plan: &Plan, payload: &[bool]) -> Vec<Complex<f32>> {
    let code = ConvCode::new(&MOTHER);
    let mut bits = payload.to_vec();
    bits.resize(plan.bits(), false);
    disperse(&mut bits);
    let cells = plan.cells();
    let mut dimensions = vec![[0u8; 3]; 2 * cells];
    let mut level_bits = Vec::new();
    let mut coded = Vec::new();
    let mut order = Vec::new();
    for level in 0..plan.qam.levels() {
        plan.split(&bits, level, &mut level_bits);
        encode_level(&plan.levels[level], &level_bits, &code, &mut coded);
        for (start, length) in MlcDecoder::parts(plan) {
            match plan.qam.interleaver(level) {
                Some(t) => {
                    permutation(length, t, &mut order);
                    for (index, &source) in order.iter().enumerate() {
                        dimensions[start + index][level] = u8::from(coded[start + source]);
                    }
                }
                None => {
                    let span = start..start + length;
                    for (dimension, &bit) in dimensions[span.clone()].iter_mut().zip(&coded[span]) {
                        dimension[level] = u8::from(bit);
                    }
                }
            }
        }
    }
    let levels = plan.qam.levels();
    dimensions
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            Complex::new(
                point(plan.qam, &pair[0][..levels]),
                point(plan.qam, &pair[1][..levels]),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drm::coding::{R1_3, R2_3, higher_cells, rates_for};

    #[test]
    fn the_dispersal_sequence_starts_as_specified() {
        let mut bits = [false; 16];
        disperse(&mut bits);
        let expected = [0, 0, 0, 0, 0, 1, 1, 1, 1, 0, 1, 1, 1, 1, 1, 0];
        assert_eq!(bits.map(u8::from), expected);
    }

    #[test]
    fn mapping_follows_the_constellation_figures() {
        let a = Qam::Q64.scale();
        assert_eq!(point(Qam::Q64, &[0, 0, 0]), 7.0 * a);
        assert_eq!(point(Qam::Q64, &[1, 0, 0]), 5.0 * a);
        assert_eq!(point(Qam::Q64, &[1, 1, 0]), a);
        assert_eq!(point(Qam::Q64, &[0, 0, 1]), -a);
        assert_eq!(point(Qam::Q64, &[1, 1, 1]), -7.0 * a);
        let a = Qam::Q16.scale();
        assert_eq!(point(Qam::Q16, &[1, 0]), a);
        assert_eq!(point(Qam::Q16, &[0, 1]), -a);
        assert_eq!(point(Qam::Q4, &[1]), -Qam::Q4.scale());
    }

    fn noisy(cells: &[Complex<f32>], sigma: f32, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f32 / u32::MAX as f32) * 2.0 - 1.0
        };
        cells
            .iter()
            .map(|&cell| cell + Complex::new(next(), next()) * sigma)
            .collect()
    }

    #[test]
    fn multilevel_codes_survive_noise_with_unequal_protection() {
        for (qam, cells, protection) in [(Qam::Q64, 2959, (0, 1)), (Qam::Q16, 2337, (0, 1))] {
            let lower = rates_for(qam, false, protection.1).expect("rates");
            let higher = rates_for(qam, false, protection.0).expect("rates");
            let levels = qam.levels();
            let n1 = higher_cells(40, &higher[..levels]);
            let plan = Plan::protected(
                qam,
                n1,
                cells - n1,
                Some(&higher[..levels]),
                &lower[..levels],
            )
            .expect("plan");
            let payload: Vec<bool> = (0..plan.bits())
                .map(|i| (i * 13 + i / 7) % 3 == 0)
                .collect();
            let sent = encode(&plan, &payload);
            let received = noisy(&sent, 0.12, 9);
            let mut decoder = MlcDecoder::default();
            let mut out = Vec::new();
            decoder
                .decode(&plan, &received, &vec![1.0; cells], &mut out)
                .expect("decodable");
            assert_eq!(out, payload, "{qam:?}");
        }
        let plan = Plan::eep(Qam::Q16, 405, &[R1_3, R2_3]).expect("SDC");
        let payload: Vec<bool> = (0..plan.bits()).map(|i| i % 5 == 1).collect();
        let received = noisy(&encode(&plan, &payload), 0.1, 3);
        let mut out = Vec::new();
        MlcDecoder::default()
            .decode(&plan, &received, &vec![1.0; 405], &mut out)
            .expect("decodable");
        assert_eq!(out, payload);
    }
}
