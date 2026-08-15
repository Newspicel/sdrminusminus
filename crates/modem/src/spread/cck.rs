use std::f32::consts::FRAC_1_SQRT_2;

use num_complex::Complex;

use super::chip::{ChipShaper, find_burst};
use crate::{linear::PhaseAnchor, soft::Llr};

pub const CHIPS: usize = 8;

pub const MAX_WORDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CckMode {
    Bits4,
    Bits8,
}

impl CckMode {
    #[must_use]
    pub fn bits_per_symbol(self) -> usize {
        match self {
            Self::Bits4 => 4,
            Self::Bits8 => 8,
        }
    }

    #[must_use]
    pub fn words(self) -> usize {
        1 << (self.bits_per_symbol() - 2)
    }

    #[must_use]
    pub fn alphabet(self) -> u32 {
        1 << self.bits_per_symbol()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Codebook {
    mode: CckMode,
    words: Vec<[Complex<f32>; CHIPS]>,
}

fn quadrant(index: u32) -> Complex<f32> {
    match index & 3 {
        0 => Complex::new(1.0, 0.0),
        1 => Complex::new(0.0, 1.0),
        2 => Complex::new(-1.0, 0.0),
        _ => Complex::new(0.0, -1.0),
    }
}

impl Codebook {
    #[must_use]
    pub fn new(mode: CckMode) -> Self {
        let words = (0..mode.words() as u32)
            .map(|index| Self::word(mode, index))
            .collect();
        Self { mode, words }
    }

    fn word(mode: CckMode, index: u32) -> [Complex<f32>; CHIPS] {
        let (p2, p3, p4) = match mode {
            CckMode::Bits8 => (
                quadrant(index & 3),
                quadrant((index >> 2) & 3),
                quadrant((index >> 4) & 3),
            ),
            CckMode::Bits4 => (
                quadrant(1 + 2 * (index & 1)),
                quadrant(0),
                quadrant(2 * ((index >> 1) & 1)),
            ),
        };
        [
            p2 * p3 * p4,
            p3 * p4,
            p2 * p4,
            -p4,
            p2 * p3,
            p3,
            -p2,
            Complex::new(1.0, 0.0),
        ]
    }

    #[must_use]
    pub fn mode(&self) -> CckMode {
        self.mode
    }

    #[must_use]
    pub fn words(&self) -> &[[Complex<f32>; CHIPS]] {
        &self.words
    }

    pub fn chips(&self, label: u32, symbol: usize, out: &mut [Complex<f32>; CHIPS]) {
        let word = &self.words[(label >> 2) as usize % self.words.len()];
        let rotation = quadrant(label + 2 * (symbol as u32 & 1)) * (CHIPS as f32).sqrt().recip();
        for (slot, &chip) in out.iter_mut().zip(word) {
            *slot = chip * rotation;
        }
    }

    #[must_use]
    pub fn reference(&self, label: u32, symbol: usize) -> Complex<f32> {
        quadrant(label + 2 * (symbol as u32 & 1))
    }

    pub fn correlate(&self, chips: &[Complex<f32>; CHIPS], out: &mut [Complex<f32>]) {
        assert!(
            out.len() >= self.words.len(),
            "the bank has {} words; {} slots cannot hold it",
            self.words.len(),
            out.len()
        );
        let scale = (CHIPS as f64).sqrt().recip();
        for (slot, word) in out.iter_mut().zip(&self.words) {
            let mut re = 0.0f64;
            let mut im = 0.0f64;
            for (&y, &w) in chips.iter().zip(word) {
                re += f64::from(y.re) * f64::from(w.re) + f64::from(y.im) * f64::from(w.im);
                im += f64::from(y.im) * f64::from(w.re) - f64::from(y.re) * f64::from(w.im);
            }
            *slot = Complex::new((re * scale) as f32, (im * scale) as f32);
        }
    }

    #[must_use]
    pub fn decide(&self, bank: &[Complex<f32>], symbol: usize) -> u32 {
        let mut best = (0u32, f32::NEG_INFINITY);
        for label in 0..self.mode.alphabet() {
            let metric = self.metric(bank, label, symbol);
            if metric > best.1 {
                best = (label, metric);
            }
        }
        best.0
    }

    #[must_use]
    pub fn metric(&self, bank: &[Complex<f32>], label: u32, symbol: usize) -> f32 {
        let r = bank[(label >> 2) as usize % self.words.len()];
        match (label + 2 * (symbol as u32 & 1)) & 3 {
            0 => r.re,
            1 => r.im,
            2 => -r.re,
            _ => -r.im,
        }
    }

    pub fn llrs(&self, bank: &[Complex<f32>], symbol: usize, noise_var: f64, out: &mut [Llr]) {
        let bits = self.mode.bits_per_symbol();
        assert_eq!(out.len(), bits, "one LLR slot per codeword bit");
        assert!(
            noise_var.is_finite() && noise_var > 0.0,
            "noise_var is a measured variance; {noise_var} is not one"
        );
        let mut max0 = [f32::NEG_INFINITY; 8];
        let mut max1 = [f32::NEG_INFINITY; 8];
        for label in 0..self.mode.alphabet() {
            let metric = self.metric(bank, label, symbol);
            for k in 0..bits {
                if (label >> k) & 1 == 1 {
                    max1[k] = max1[k].max(metric);
                } else {
                    max0[k] = max0[k].max(metric);
                }
            }
        }
        let scale = 2.0 / noise_var;
        for (k, slot) in out.iter_mut().enumerate() {
            *slot = Llr((f64::from(max1[k] - max0[k]) * scale) as f32);
        }
    }
}

#[derive(Clone, Debug)]
pub struct CckParams {
    codebook: Codebook,
    shaper: ChipShaper,
    search_group_symbols: usize,
}

impl CckParams {
    #[must_use]
    pub fn new(mode: CckMode, shaper: ChipShaper, search_group_symbols: usize) -> Self {
        assert!(search_group_symbols > 0, "a search group spans ≥ 1 symbol");
        Self {
            codebook: Codebook::new(mode),
            shaper,
            search_group_symbols,
        }
    }

    #[must_use]
    pub fn codebook(&self) -> &Codebook {
        &self.codebook
    }

    #[must_use]
    pub fn shaper(&self) -> &ChipShaper {
        &self.shaper
    }

    #[must_use]
    pub fn mode(&self) -> CckMode {
        self.codebook.mode()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.codebook.mode().bits_per_symbol()
    }

    #[must_use]
    pub fn symbol_samples(&self) -> usize {
        CHIPS * self.shaper.sps()
    }

    #[must_use]
    pub fn framing_overhead_db(preamble: usize, payload: usize) -> f64 {
        10.0 * ((preamble + payload) as f64 / payload as f64).log10()
    }
}

#[derive(Clone, Debug)]
pub struct CckMod {
    params: CckParams,
    chips: Vec<Complex<f32>>,
}

impl CckMod {
    #[must_use]
    pub fn new(params: CckParams) -> Self {
        Self {
            params,
            chips: Vec::new(),
        }
    }

    #[must_use]
    pub fn params(&self) -> &CckParams {
        &self.params
    }

    pub fn frame(&mut self, preamble: &[u32], labels: &[u32], out: &mut Vec<Complex<f32>>) {
        self.chips.clear();
        self.push_labels(preamble, 0);
        self.push_labels(labels, preamble.len());
        self.params.shaper.render(&self.chips, out);
    }

    fn push_labels(&mut self, labels: &[u32], first_symbol: usize) {
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in labels.iter().enumerate() {
            self.params
                .codebook
                .chips(label, first_symbol + k, &mut word);
            self.chips.extend_from_slice(&word);
        }
    }

    #[must_use]
    pub fn preamble_chips(&self, preamble: &[u32]) -> Vec<Complex<f32>> {
        let mut chips = Vec::with_capacity(preamble.len() * CHIPS);
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in preamble.iter().enumerate() {
            self.params.codebook.chips(label, k, &mut word);
            chips.extend_from_slice(&word);
        }
        chips
    }
}

#[derive(Clone, Debug)]
pub struct CckDemod {
    params: CckParams,
    filtered: Vec<Complex<f32>>,
    known: Vec<Complex<f32>>,
    fitted: Vec<Complex<f32>>,
    expected: Vec<Complex<f32>>,
    acquisition: Option<super::dsss::Acquisition>,
}

impl CckDemod {
    #[must_use]
    pub fn new(params: CckParams) -> Self {
        Self {
            params,
            filtered: Vec::new(),
            known: Vec::new(),
            fitted: Vec::new(),
            expected: Vec::new(),
            acquisition: None,
        }
    }

    #[must_use]
    pub fn params(&self) -> &CckParams {
        &self.params
    }

    #[must_use]
    pub fn acquisition(&self) -> Option<super::dsss::Acquisition> {
        self.acquisition
    }

    pub fn acquire(
        &mut self,
        wave: &[Complex<f32>],
        preamble: &[u32],
        search: usize,
    ) -> Option<super::dsss::Acquisition> {
        self.acquisition = None;
        if preamble.len() < 2 {
            return None;
        }
        self.params.shaper.matched(wave, &mut self.filtered);

        self.known.clear();
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for (k, &label) in preamble.iter().enumerate() {
            self.params.codebook.chips(label, k, &mut word);
            self.known.extend_from_slice(&word);
        }
        let group_chips = self.params.search_group_symbols * CHIPS;
        let origin = find_burst(
            &self.params.shaper,
            &self.filtered,
            &self.known,
            group_chips,
            search,
        )?;

        self.fitted.clear();
        self.expected.clear();
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        for (k, &label) in preamble.iter().enumerate() {
            self.bank_at(origin, k, bank);
            self.fitted.push(bank[(label >> 2) as usize % bank.len()]);
            self.expected.push(self.params.codebook.reference(label, k));
        }
        let anchor = PhaseAnchor::fit_gain_only(&self.fitted, &self.expected).ok()?;
        anchor.correct_block(0, &mut self.fitted);
        let noise_var =
            crate::constellation::demap::noise_var_from_known(&self.fitted, &self.expected);
        let acquisition = super::dsss::Acquisition {
            origin,
            anchor,
            noise_var,
        };
        self.acquisition = Some(acquisition);
        Some(acquisition)
    }

    pub fn bank(&self, symbol: usize, out: &mut [Complex<f32>]) {
        let Some(acquisition) = self.acquisition else {
            out.fill(Complex::new(0.0, 0.0));
            return;
        };
        self.bank_at(acquisition.origin, symbol, out);
        for slot in out.iter_mut() {
            *slot = acquisition.anchor.correct(symbol, *slot);
        }
    }

    fn bank_at(&self, origin: usize, symbol: usize, out: &mut [Complex<f32>]) {
        let mut chips = [Complex::new(0.0, 0.0); CHIPS];
        self.params
            .shaper
            .block(&self.filtered, origin, symbol * CHIPS, &mut chips);
        self.params.codebook.correlate(&chips, out);
    }

    pub fn demodulate(&self, preamble_symbols: usize, symbols: usize, out: &mut Vec<u32>) {
        if self.acquisition.is_none() {
            return;
        }
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        out.reserve(symbols);
        for k in 0..symbols {
            let index = preamble_symbols + k;
            self.bank(index, bank);
            out.push(self.params.codebook.decide(bank, index));
        }
    }

    pub fn llrs(&self, preamble_symbols: usize, symbols: usize, out: &mut Vec<Llr>) {
        let Some(acquisition) = self.acquisition else {
            return;
        };
        let bits = self.params.bits_per_symbol();
        let mut bank = [Complex::new(0.0, 0.0); MAX_WORDS];
        let bank = &mut bank[..self.params.codebook.words().len()];
        let mut symbol_llrs = [Llr(0.0); 8];
        let symbol_llrs = &mut symbol_llrs[..bits];
        let noise_var = acquisition.noise_var.max(f64::MIN_POSITIVE);
        out.reserve(symbols * bits);
        for k in 0..symbols {
            let index = preamble_symbols + k;
            self.bank(index, bank);
            self.params
                .codebook
                .llrs(bank, index, noise_var, symbol_llrs);
            out.extend_from_slice(symbol_llrs);
        }
    }
}

pub const CHIP_AMPLITUDE: f32 = FRAC_1_SQRT_2 * 0.5;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ber::rng::Rng;

    const SPS: usize = 4;
    const LEAD: usize = 41;
    const PREAMBLE: usize = 32;

    fn params(mode: CckMode) -> CckParams {
        CckParams::new(mode, ChipShaper::root_raised_cosine(SPS, 0.35, 8), 8)
    }

    fn labels(mode: CckMode, count: usize, seed: u32) -> Vec<u32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state % mode.alphabet()
            })
            .collect()
    }

    fn transmit(p: &CckParams, preamble: &[u32], payload: &[u32]) -> Vec<Complex<f32>> {
        let mut wave = vec![Complex::new(0.0, 0.0); LEAD];
        CckMod::new(p.clone()).frame(preamble, payload, &mut wave);
        wave.resize(wave.len() + 128, Complex::new(0.0, 0.0));
        wave
    }

    #[test]
    fn the_generated_codebook_is_constant_modulus_distinct_and_factors_phi1_out() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let book = Codebook::new(mode);
            assert_eq!(book.words().len(), mode.words());
            for (i, word) in book.words().iter().enumerate() {
                for (c, chip) in word.iter().enumerate() {
                    assert!(
                        (chip.norm() - 1.0).abs() < 1e-6,
                        "{mode:?} word {i} chip {c} modulus {}",
                        chip.norm()
                    );
                }
                for (j, other) in book.words().iter().enumerate().skip(i + 1) {
                    let same = word.iter().zip(other).all(|(a, b)| (a - b).norm() < 1e-6);
                    assert!(!same, "{mode:?} words {i} and {j} coincide");
                }
            }
            let mut a = [Complex::new(0.0, 0.0); CHIPS];
            let mut b = [Complex::new(0.0, 0.0); CHIPS];
            let base = 2u32 << 2;
            book.chips(base, 0, &mut a);
            book.chips(base + 1, 0, &mut b);
            let ratio = b[0] / a[0];
            for c in 1..CHIPS {
                assert!((b[c] / a[c] - ratio).norm() < 1e-5, "{mode:?} chip {c}");
            }
            assert!((ratio - Complex::new(0.0, 1.0)).norm() < 1e-5);
        }
    }

    #[test]
    fn the_codebook_is_a_block_code_with_the_distance_that_earns_its_rate() {
        let book = Codebook::new(CckMode::Bits8);
        let chips_of = |label: u32| {
            let mut out = [Complex::new(0.0, 0.0); CHIPS];
            book.chips(label, 0, &mut out);
            out
        };
        let d2 = |x: u32, y: u32| -> f64 {
            chips_of(x)
                .iter()
                .zip(&chips_of(y))
                .map(|(&p, &q)| f64::from((p - q).norm_sqr()))
                .sum()
        };
        let mut d2_min = f64::INFINITY;
        for x in 0..256u32 {
            for y in 0..256u32 {
                if x != y {
                    d2_min = d2_min.min(d2(x, y));
                }
            }
        }
        assert!((d2_min - 1.0).abs() < 1e-5, "d²_min {d2_min}");

        for x in 0..256u32 {
            let neighbours = (0..256u32)
                .filter(|&y| y != x && (d2(x, y) - d2_min).abs() < 1e-5)
                .count();
            assert_eq!(
                neighbours, 24,
                "word {x} has {neighbours} nearest neighbours"
            );
        }

        assert!(
            (d2(0, 1) - 2.0).abs() < 1e-5,
            "φ1 quarter turn d² {}",
            d2(0, 1)
        );
    }

    #[test]
    fn a_codeword_carries_unit_energy() {
        let book = Codebook::new(CckMode::Bits8);
        let mut word = [Complex::new(0.0, 0.0); CHIPS];
        for label in [0u32, 1, 37, 200, 255] {
            book.chips(label, 0, &mut word);
            let energy: f64 = word.iter().map(|c| f64::from(c.norm_sqr())).sum();
            assert!((energy - 1.0).abs() < 1e-6, "label {label} energy {energy}");
            assert!((word[0].norm() - CHIP_AMPLITUDE).abs() < 1e-6);
        }
    }

    #[test]
    fn the_bank_recovers_a_clean_codeword_and_its_phase() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let book = Codebook::new(mode);
            let mut word = [Complex::new(0.0, 0.0); CHIPS];
            let mut bank = vec![Complex::new(0.0, 0.0); book.words().len()];
            for label in 0..mode.alphabet() {
                for symbol in 0..2 {
                    book.chips(label, symbol, &mut word);
                    book.correlate(&word, &mut bank);
                    assert_eq!(book.decide(&bank, symbol), label, "{mode:?} label {label}");
                    let winner = bank[(label >> 2) as usize % bank.len()];
                    let want = book.reference(label, symbol);
                    assert!((winner - want).norm() < 1e-5, "{mode:?} label {label}");
                }
            }
        }
    }

    #[test]
    fn both_rates_round_trip_and_find_their_own_origin() {
        for mode in [CckMode::Bits4, CckMode::Bits8] {
            let p = params(mode);
            let preamble = labels(mode, PREAMBLE, 0x0cc4);
            let payload = labels(mode, 400, 0x0cc8);
            let wave = transmit(&p, &preamble, &payload);
            let mut demod = CckDemod::new(p);
            let acquisition = demod
                .acquire(&wave, &preamble, LEAD + 48)
                .unwrap_or_else(|| panic!("{mode:?}: no acquisition"));
            assert_eq!(acquisition.origin, LEAD, "{mode:?}");
            let mut got = Vec::new();
            demod.demodulate(PREAMBLE, payload.len(), &mut got);
            assert_eq!(got, payload, "{mode:?}");
        }
    }

    #[test]
    fn the_decision_is_joint_over_word_and_phase() {
        let book = Codebook::new(CckMode::Bits8);
        let mut bank = vec![Complex::new(0.0, 0.0); book.words().len()];
        bank[3] = Complex::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
        bank[5] = Complex::new(0.95, 0.0);
        assert!(bank[3].norm() > bank[5].norm());
        let decided = book.decide(&bank, 0);
        assert_eq!(decided >> 2, 5, "decided word {}", decided >> 2);
        assert_eq!(decided & 3, 0);
    }

    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let mode = CckMode::Bits8;
        let p = params(mode);
        let preamble = labels(mode, 128, 0x11c4);
        let payload = labels(mode, 12_000, 0x11c5);
        let mut wave = transmit(&p, &preamble, &payload);
        let mut rng = Rng::new(0x11c6);
        let sigma = (0.08f64 / 2.0).sqrt();
        for s in wave.iter_mut() {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
        let mut demod = CckDemod::new(p);
        demod.acquire(&wave, &preamble, LEAD + 48).unwrap();
        let mut llrs = Vec::new();
        demod.llrs(preamble.len(), payload.len(), &mut llrs);

        let bits = mode.bits_per_symbol();
        let mut bands = [(0u32, 0u32); 3];
        for (i, &llr) in llrs.iter().enumerate() {
            let sent = (payload[i / bits] >> (i % bits)) & 1 == 1;
            let band = match llr.0.abs() {
                x if x < 1.0 => 0,
                x if x < 3.0 => 1,
                _ => 2,
            };
            bands[band].0 += 1;
            if llr.bit() != sent {
                bands[band].1 += 1;
            }
        }
        for (band, &(count, wrong)) in bands.iter().enumerate() {
            assert!(count > 300, "band {band} saw only {count} bits");
            let measured = f64::from(wrong) / f64::from(count);
            let predicted = 1.0 / (1.0 + [0.5f64, 2.0, 4.0][band].exp());
            assert!(
                measured < predicted * 3.0 + 0.02,
                "band {band}: measured {measured}, predicted {predicted}"
            );
        }
    }

    #[test]
    fn a_demodulator_with_no_acquisition_produces_nothing() {
        let demod = CckDemod::new(params(CckMode::Bits8));
        let mut labels = Vec::new();
        let mut llrs = Vec::new();
        demod.demodulate(PREAMBLE, 50, &mut labels);
        demod.llrs(PREAMBLE, 50, &mut llrs);
        assert!(labels.is_empty() && llrs.is_empty());
    }
}
