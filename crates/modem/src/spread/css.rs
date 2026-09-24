use std::{f64::consts::TAU, sync::Arc};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use sdrmm_dsp::LoopFilter;

use crate::{constellation::demap::energy_llrs, soft::Llr};

mod acquire;
mod resample;
#[cfg(test)]
mod sync_tests;
mod window;

use acquire::Point;
use resample::Interpolator;

pub const TRACK_BW: f64 = 0.02;

const TRACK_DAMPING: f64 = std::f64::consts::FRAC_1_SQRT_2;

const TRACK_RANGE_BINS: f64 = 0.25;

pub const TIMING_BW: f64 = 0.05;

pub const MAX_CLOCK_PPM: f64 = 1_000.0;

const MIN_TIMING_RANGE: f64 = 0.5;

const SPLIT_WEIGHT: f64 = 6.0;

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
    fft: Arc<dyn Fft<f32>>,
    ifft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    reference: Vec<Complex<f32>>,
    stretch: f64,
    window: Vec<Complex<f32>>,
    source: Vec<Complex<f32>>,
    interpolator: Interpolator,
    dechirped: Vec<Complex<f32>>,
    bins: Vec<Complex<f32>>,
    energies: Vec<f32>,
    symbol_energies: Vec<f32>,
    symbol_llrs: Vec<Llr>,
    points: Vec<Point>,
    hypotheses: Vec<Complex<f64>>,
    offset_bins: f64,
    tracker: LoopFilter,
    timing: LoopFilter,
    anchor: Option<f64>,
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
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n);
        let ifft = planner.plan_fft_inverse(n);
        let scratch_len = fft
            .get_inplace_scratch_len()
            .max(ifft.get_inplace_scratch_len());
        let zero = Complex::new(0.0, 0.0);
        Self {
            scratch: vec![zero; scratch_len],
            reference: params.base_chirp().into_iter().map(|c| c.conj()).collect(),
            stretch: 1.0,
            window: vec![zero; n],
            source: vec![zero; resample::source_len(n)],
            interpolator: Interpolator::new(),
            dechirped: vec![zero; n],
            bins: vec![zero; n],
            energies: vec![0.0; n],
            symbol_energies: vec![0.0; n],
            symbol_llrs: vec![Llr(0.0); params.bits_per_symbol()],
            points: Vec::new(),
            hypotheses: vec![Complex::new(0.0, 0.0); n],
            offset_bins: 0.0,
            tracker: tracker(),
            timing: timing_loop(n),
            anchor: None,
            params,
            fft,
            ifft,
        }
    }

    #[must_use]
    pub fn offset_bins(&self) -> f64 {
        self.offset_bins
    }

    #[must_use]
    pub fn samples_per_symbol(&self) -> f64 {
        self.params.chips() as f64 + self.timing.freq_norm()
    }

    #[must_use]
    pub fn clock_ppm(&self) -> f64 {
        self.timing.freq_norm() / self.params.chips() as f64 * 1e6
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
        let start = self.position(origin, symbol);
        self.observe(iq, start);
        out.copy_from_slice(&self.symbol_energies);
    }

    fn position(&self, origin: usize, symbol: usize) -> f64 {
        let nominal = (origin + symbol * self.params.chips()) as f64;
        match self.anchor {
            None => nominal,
            Some(anchor) => {
                let period = self.samples_per_symbol();
                anchor + ((nominal - anchor) / period).round() * period
            }
        }
    }

    fn observe(&mut self, iq: &[Complex<f32>], start: f64) {
        self.load(iq, start, self.offset_bins, self.samples_per_symbol());
        self.dechirp();
    }

    fn peak(&self) -> f32 {
        self.energies.iter().copied().fold(0.0f32, f32::max)
    }

    fn decide(&mut self, iq: &[Complex<f32>], origin: usize, symbol: usize) -> u32 {
        let n = self.params.chips() as f64;
        let start = self.position(origin, symbol);
        self.observe(iq, start);
        let bin = argmax_bin(&self.energies);
        let value = self.symbol_of(f64::from(bin));
        let (tone, late) = self.resolve(f64::from(value) * self.stretch, value);
        let weight = self.timing_weight(value);
        let freq_error = (wrap(tone - f64::from(value) * self.stretch, n) + weight.min(1.0) * late)
            .clamp(-0.5, 0.5);
        let timing_error = (weight * late).clamp(-0.5, 0.5);
        self.offset_bins += self.tracker.advance(TAU * freq_error) / TAU;
        let step = self.timing.advance(TAU * timing_error) / TAU;
        self.anchor = Some(start + n + step);
        value
    }

    fn symbol_of(&self, tone: f64) -> u32 {
        let n = self.params.chips() as i64;
        ((tone / self.stretch).round() as i64).rem_euclid(n) as u32
    }

    fn resolve(&self, tone: f64, value: u32) -> (f64, f64) {
        let n = self.params.chips();
        let split = self.wrap_point(value);
        if split == 0 || split >= n {
            return (tone, 0.0);
        }
        let resolved = self.split_tone(tone, split);
        (resolved.tone, resolved.late)
    }

    fn timing_weight(&self, value: u32) -> f64 {
        let share = self.wrap_point(value) as f64 / self.params.chips() as f64;
        SPLIT_WEIGHT * share * (1.0 - share)
    }

    fn wrap_point(&self, value: u32) -> usize {
        let n = self.params.chips();
        let split = ((n as f64 - f64::from(value)) / self.stretch).round();
        split.clamp(0.0, n as f64) as usize
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
            let value = self.decide(iq, origin, symbol);
            out.push(value);
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
            let _ = self.decide(iq, origin, symbol);
            energy_llrs(&self.symbol_energies, noise_var, &mut self.symbol_llrs);
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
            let start = self.position(origin, symbol);
            self.observe(iq, start);
            let peak = f64::from(self.peak());
            sum += self.energies.iter().map(|&e| f64::from(e)).sum::<f64>() - peak;
        }
        sum / (symbols * (n - 1)) as f64
    }

    fn restart(&mut self) {
        self.offset_bins = 0.0;
        self.tracker = tracker();
        self.timing = timing_loop(self.params.chips());
        self.anchor = None;
    }
}

fn tracker() -> LoopFilter {
    LoopFilter::new(TRACK_BW, TRACK_DAMPING, TRACK_RANGE_BINS)
}

fn timing_loop(chips: usize) -> LoopFilter {
    let range = (chips as f64 * MAX_CLOCK_PPM * 1e-6).max(MIN_TIMING_RANGE);
    LoopFilter::new(TIMING_BW, TRACK_DAMPING, range)
}

fn wrap(value: f64, period: f64) -> f64 {
    value - (value / period).round() * period
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

    fn offset(wave: &mut [Complex<f32>], bins_per_symbol: f64, drift: f64, n: usize) {
        let mut phase = 0.0f64;
        for (k, s) in wave.iter_mut().enumerate() {
            let symbol = k as f64 / n as f64;
            let bins = bins_per_symbol + drift * symbol;
            phase += std::f64::consts::TAU * bins / n as f64;
            *s *= Complex::new(phase.cos() as f32, phase.sin() as f32);
        }
    }

    #[test]
    fn a_fractional_offset_and_its_drift_are_tracked() {
        let params = CssParams::new(9);
        let n = params.chips();
        let preamble = payload(n, 16, 0x0f0f);
        let symbols = payload(n, 400, 0x1f1f);
        let mut wave = Vec::new();
        CssMod::new(params.clone()).frame(&preamble, &symbols, &mut wave);
        offset(&mut wave, 0.37, 0.004, n);
        add_noise(&mut wave, 0x2f2f, 0.02);
        let mut demod = CssDemod::new(params);
        let origin = demod.estimate_origin(&wave, &preamble);
        assert_eq!(origin, 0);
        let mut got = Vec::new();
        demod.demodulate(&wave, preamble.len() * n, symbols.len(), &mut got);
        let errors = got.iter().zip(&symbols).filter(|(a, b)| a != b).count();
        assert_eq!(errors, 0, "offset read {}", demod.offset_bins());
        assert!((demod.offset_bins() - 2.03).abs() < 0.05);
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
