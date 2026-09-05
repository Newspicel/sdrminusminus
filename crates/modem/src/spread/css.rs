use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{constellation::demap::energy_llrs, soft::Llr};

pub const MIN_SPREADING_FACTOR: u32 = 5;

pub const MAX_SPREADING_FACTOR: u32 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssParams {
    spreading_factor: u32,
}

impl CssParams {
    #[must_use]
    pub fn new(spreading_factor: u32) -> Self {
        assert!(
            (MIN_SPREADING_FACTOR..=MAX_SPREADING_FACTOR).contains(&spreading_factor),
            "spreading factor {spreading_factor} is outside \
             {MIN_SPREADING_FACTOR}..={MAX_SPREADING_FACTOR}"
        );
        Self { spreading_factor }
    }

    #[must_use]
    pub fn spreading_factor(&self) -> u32 {
        self.spreading_factor
    }

    #[must_use]
    pub fn chips(&self) -> usize {
        1 << self.spreading_factor
    }

    #[must_use]
    pub fn alphabet(&self) -> usize {
        self.chips()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.spreading_factor as usize
    }

    #[must_use]
    pub fn framing_overhead_db(preamble: usize, payload: usize) -> f64 {
        10.0 * ((preamble + payload) as f64 / payload as f64).log10()
    }

    #[must_use]
    pub fn base_chirp(&self) -> Vec<Complex<f32>> {
        let n = self.chips();
        (0..n)
            .map(|k| {
                let k = k as f64;
                let turns = k * k / (2.0 * n as f64) - k / 2.0;
                let phase = std::f64::consts::TAU * (turns - turns.floor());
                let (sin, cos) = phase.sin_cos();
                Complex::new(cos as f32, sin as f32)
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct CssMod {
    params: CssParams,
    base: Vec<Complex<f32>>,
    amplitude: f32,
}

impl CssMod {
    #[must_use]
    pub fn new(params: CssParams) -> Self {
        let base = params.base_chirp();
        let amplitude = (params.chips() as f32).sqrt().recip();
        Self {
            params,
            base,
            amplitude,
        }
    }

    #[must_use]
    pub fn params(&self) -> &CssParams {
        &self.params
    }

    pub fn frame(&self, preamble: &[u32], symbols: &[u32], out: &mut Vec<Complex<f32>>) {
        out.reserve((preamble.len() + symbols.len()) * self.params.chips());
        self.modulate(preamble, out);
        self.modulate(symbols, out);
    }

    pub fn modulate(&self, symbols: &[u32], out: &mut Vec<Complex<f32>>) {
        let n = self.params.chips();
        for &symbol in symbols {
            let shift = symbol as usize % n;
            for k in 0..n {
                let turns = (k * shift) as f64 / n as f64;
                let phase = std::f64::consts::TAU * (turns - turns.floor());
                let (sin, cos) = phase.sin_cos();
                let rotation = Complex::new(cos as f32, sin as f32);
                out.push(self.base[k] * rotation * self.amplitude);
            }
        }
    }
}

#[derive(Clone)]
pub struct CssDemod {
    params: CssParams,
    conjugate: Vec<Complex<f32>>,
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    bins: Vec<Complex<f32>>,
    energies: Vec<f32>,
    symbol_llrs: Vec<Llr>,
    votes: Vec<u32>,
}

impl std::fmt::Debug for CssDemod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CssDemod")
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl CssDemod {
    #[must_use]
    pub fn new(params: CssParams) -> Self {
        let n = params.chips();
        let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
        let conjugate = params.base_chirp().into_iter().map(|c| c.conj()).collect();
        Self {
            scratch: vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()],
            bins: vec![Complex::new(0.0, 0.0); n],
            energies: vec![0.0; n],
            symbol_llrs: vec![Llr(0.0); params.bits_per_symbol()],
            votes: vec![0; n],
            params,
            conjugate,
            fft,
        }
    }

    #[must_use]
    pub fn params(&self) -> &CssParams {
        &self.params
    }

    pub fn energies(&mut self, iq: &[Complex<f32>], origin: usize, symbol: usize, out: &mut [f32]) {
        assert_eq!(
            out.len(),
            self.params.chips(),
            "one energy per cyclic shift"
        );
        self.fill(iq, origin, symbol);
        out.copy_from_slice(&self.energies);
    }

    fn fill(&mut self, iq: &[Complex<f32>], origin: usize, symbol: usize) {
        let n = self.params.chips();
        let at = origin + symbol * n;
        for k in 0..n {
            let y = iq.get(at + k).copied().unwrap_or(Complex::new(0.0, 0.0));
            self.bins[k] = y * self.conjugate[k];
        }
        self.fft
            .process_with_scratch(&mut self.bins, &mut self.scratch);
        let scale = (n as f32).recip();
        for (slot, bin) in self.energies.iter_mut().zip(&self.bins) {
            *slot = bin.norm_sqr() * scale;
        }
    }

    fn peak(&self) -> f32 {
        self.energies.iter().copied().fold(0.0f32, f32::max)
    }

    pub fn estimate_origin(&mut self, iq: &[Complex<f32>], preamble: &[u32]) -> usize {
        let n = self.params.chips();
        self.votes.fill(0);
        for (k, &known) in preamble.iter().enumerate() {
            self.fill(iq, 0, k);
            let decoded = argmax_bin(&self.energies);
            let shift = (known + n as u32 - decoded % n as u32) % n as u32;
            self.votes[shift as usize] += 1;
        }
        let mut best = 0usize;
        for (shift, &count) in self.votes.iter().enumerate() {
            if count > self.votes[best] {
                best = shift;
            }
        }
        best
    }

    pub fn demodulate(
        &mut self,
        iq: &[Complex<f32>],
        origin: usize,
        symbols: usize,
        out: &mut Vec<u32>,
    ) {
        out.reserve(symbols);
        for symbol in 0..symbols {
            self.fill(iq, origin, symbol);
            out.push(argmax_bin(&self.energies));
        }
    }

    pub fn llrs(
        &mut self,
        iq: &[Complex<f32>],
        origin: usize,
        symbols: usize,
        noise_var: f64,
        out: &mut Vec<Llr>,
    ) {
        out.reserve(symbols * self.params.bits_per_symbol());
        for symbol in 0..symbols {
            self.fill(iq, origin, symbol);
            energy_llrs(&self.energies, noise_var, &mut self.symbol_llrs);
            out.extend_from_slice(&self.symbol_llrs);
        }
    }

    pub fn noise_var(&mut self, iq: &[Complex<f32>], origin: usize, symbols: usize) -> f64 {
        assert!(
            symbols > 0,
            "a variance needs at least one symbol to measure"
        );
        let n = self.params.chips();
        let mut sum = 0.0f64;
        for symbol in 0..symbols {
            self.fill(iq, origin, symbol);
            let peak = f64::from(self.peak());
            sum += self.energies.iter().map(|&e| f64::from(e)).sum::<f64>() - peak;
        }
        sum / (symbols * (n - 1)) as f64
    }
}

fn argmax_bin(energies: &[f32]) -> u32 {
    let mut best = 0usize;
    for (k, &e) in energies.iter().enumerate() {
        if e >= energies[best] {
            best = k;
        }
    }
    best as u32
}

#[cfg(test)]
mod tests {
    use sdrmm_modem_test_support::ber::rng::Rng;

    use super::*;

    fn payload(n: usize, count: usize, seed: u32) -> Vec<u32> {
        let mut state = seed | 1;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state % n as u32
            })
            .collect()
    }

    fn add_noise(wave: &mut [Complex<f32>], seed: u64, noise_var: f64) {
        let mut rng = Rng::new(seed);
        let sigma = (noise_var / 2.0).sqrt();
        for s in wave.iter_mut() {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
    }

    #[test]
    fn distinct_shifts_are_orthogonal_at_every_spreading_factor() {
        for sf in MIN_SPREADING_FACTOR..=10 {
            let params = CssParams::new(sf);
            let n = params.chips();
            let modulator = CssMod::new(params);
            let symbol_of = |s: u32| {
                let mut out = Vec::new();
                modulator.modulate(&[s], &mut out);
                out
            };
            let a = symbol_of(0);
            let energy: f64 = a.iter().map(|c| f64::from(c.norm_sqr())).sum();
            assert!((energy - 1.0).abs() < 1e-5, "SF{sf} energy {energy}");
            for shift in [1u32, 3, (n / 2) as u32, (n - 1) as u32] {
                let b = symbol_of(shift);
                let inner: Complex<f64> = a
                    .iter()
                    .zip(&b)
                    .map(|(&x, &y)| {
                        Complex::new(f64::from(x.re), f64::from(x.im))
                            * Complex::new(f64::from(y.re), -f64::from(y.im))
                    })
                    .sum();
                assert!(
                    inner.norm() < 1e-4,
                    "SF{sf} shift {shift}: inner product {inner}"
                );
            }
        }
    }

    #[test]
    fn every_spreading_factor_round_trips_and_stays_constant_envelope() {
        for sf in MIN_SPREADING_FACTOR..=MAX_SPREADING_FACTOR {
            let params = CssParams::new(sf);
            let n = params.chips();
            let symbols = payload(n, 24, 0xc55 + sf);
            let mut wave = Vec::new();
            CssMod::new(params.clone()).modulate(&symbols, &mut wave);
            let amplitude = (n as f32).sqrt().recip();
            for (k, s) in wave.iter().enumerate() {
                assert!(
                    (s.norm() - amplitude).abs() < 1e-5,
                    "SF{sf} sample {k} modulus {}",
                    s.norm()
                );
            }
            let mut got = Vec::new();
            CssDemod::new(params).demodulate(&wave, 0, symbols.len(), &mut got);
            assert_eq!(got, symbols, "SF{sf}");
        }
    }

    #[test]
    fn the_timing_estimate_finds_the_origin_the_burst_was_sent_at() {
        let params = CssParams::new(7);
        let n = params.chips();
        let preamble = payload(n, 8, 0x7157);
        let mut demod = CssDemod::new(params.clone());
        for lead in [0usize, 1, 17, 40, 63] {
            let mut wave = vec![Complex::new(0.0, 0.0); lead];
            CssMod::new(params.clone()).modulate(&preamble, &mut wave);
            wave.resize(wave.len() + 256, Complex::new(0.0, 0.0));
            add_noise(&mut wave, 0x7158 + lead as u64, 0.05);
            assert_eq!(demod.estimate_origin(&wave, &preamble), lead, "lead {lead}");
        }
        let mut wave = vec![Complex::new(0.0, 0.0); 100];
        CssMod::new(params).modulate(&preamble, &mut wave);
        wave.resize(wave.len() + 256, Complex::new(0.0, 0.0));
        assert_ne!(demod.estimate_origin(&wave, &preamble), 100);
    }

    #[test]
    fn a_timing_error_moves_a_chirps_peak_rather_than_shrinking_it() {
        let params = CssParams::new(7);
        let n = params.chips();
        let symbols = payload(n, 8, 0x0e11);
        let lead = 32usize;
        let mut wave = vec![Complex::new(0.0, 0.0); lead];
        CssMod::new(params.clone()).modulate(&symbols, &mut wave);
        wave.resize(wave.len() + 256, Complex::new(0.0, 0.0));
        let mut demod = CssDemod::new(params);
        let mut energies = vec![0.0f32; n];
        let peak_at = |demod: &mut CssDemod, origin: usize, energies: &mut Vec<f32>| {
            demod.energies(&wave, origin, 1, energies);
            energies.iter().copied().fold(0.0f32, f32::max)
        };
        let aligned = peak_at(&mut demod, lead, &mut energies);
        let misaligned = peak_at(&mut demod, 0, &mut energies);
        assert!(
            (misaligned / aligned - 0.5625).abs() < 0.05,
            "aligned {aligned}, 32 samples early {misaligned}"
        );
        demod.energies(&wave, 0, 1, &mut energies);
        let moved = argmax_bin(&energies);
        demod.energies(&wave, lead, 1, &mut energies);
        let correct = argmax_bin(&energies);
        assert_eq!(correct, symbols[1]);
        assert_eq!(moved, (symbols[1] + n as u32 - lead as u32) % n as u32);
    }

    #[test]
    fn sensitivity_improves_with_the_spreading_factor() {
        let mut previous = usize::MAX;
        for sf in [7u32, 8, 9, 10] {
            let params = CssParams::new(sf);
            let n = params.chips();
            let count = 4_000 / sf as usize;
            let symbols = payload(n, count, 0x5e05 + sf);
            let mut wave = Vec::new();
            CssMod::new(params.clone()).modulate(&symbols, &mut wave);
            add_noise(&mut wave, 0x5e06 + u64::from(sf), 0.45);
            let mut got = Vec::new();
            CssDemod::new(params).demodulate(&wave, 0, symbols.len(), &mut got);
            let errors = got.iter().zip(&symbols).filter(|(a, b)| a != b).count();
            assert!(errors > 0, "SF{sf} too clean to order");
            assert!(
                errors < previous,
                "SF{sf}: {errors} symbol errors, SF{} had {previous}",
                sf - 1
            );
            previous = errors;
        }
    }

    #[test]
    fn the_noise_estimate_recovers_a_known_n0() {
        let params = CssParams::new(7);
        let n = params.chips();
        let symbols = payload(n, 300, 0x0e51);
        let mut wave = Vec::new();
        CssMod::new(params.clone()).modulate(&symbols, &mut wave);
        add_noise(&mut wave, 0x0e52, 0.5);
        let estimate = CssDemod::new(params).noise_var(&wave, 0, symbols.len());
        assert!((estimate - 0.5).abs() < 0.02, "estimated N0 {estimate}");
    }

    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let params = CssParams::new(6);
        let n = params.chips();
        let symbols = payload(n, 6_000, 0x11c6);
        let mut wave = Vec::new();
        CssMod::new(params.clone()).modulate(&symbols, &mut wave);
        add_noise(&mut wave, 0x11c7, 0.125);
        let mut demod = CssDemod::new(params.clone());
        let mut llrs = Vec::new();
        demod.llrs(&wave, 0, symbols.len(), 0.125, &mut llrs);
        let bits = params.bits_per_symbol();
        let mut bands = [(0u32, 0u32); 3];
        for (i, &llr) in llrs.iter().enumerate() {
            let sent = (symbols[i / bits] >> (i % bits)) & 1 == 1;
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
    fn the_framing_overhead_is_the_symbol_count_ratio() {
        assert!(CssParams::framing_overhead_db(0, 64).abs() < 1e-12);
        let overhead = CssParams::framing_overhead_db(8, 256);
        assert!((overhead - 10.0 * (264.0f64 / 256.0).log10()).abs() < 1e-12);
        assert!((overhead - 0.1336).abs() < 1e-3, "{overhead}");
    }
}
