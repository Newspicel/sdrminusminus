//! Chirp spread spectrum: a symbol is *which
//! cyclic shift* of one chirp was transmitted, read by multiplying the chirp back out and
//! transforming.
//!
//! **The identity that makes this entry acceptable against a closed form.** Dechirping symbol `s`
//! leaves `e^{j2πns/N}` — the `s`-th column of an `N`-point DFT — so the `N = 2^SF` cyclic shifts
//! of a chirp are *exactly* orthogonal, and a magnitude-argmax over the transform is *exactly*
//! noncoherent orthogonal detection of `M = N` equal-energy signals. That is the third member of
//! the identity phase 5 measured twice: M tones in one interval, M intervals at one tone, and now
//! M shifts of one chirp are one signalling set with one closed form
//! ([`theory::mfsk_noncoherent_ser`](crate::ber::theory::mfsk_noncoherent_ser)). The entry is held
//! to it at every spreading factor rather than commit-and-guarded, which is what §4.1 asks
//! wherever a closed form exists.
//!
//! **What the bandwidth buys is sensitivity.** A spreading factor spends `2^SF` chips on `SF`
//! bits, so the symbol grows exponentially while the payload grows linearly — and the
//! noncoherent orthogonal curve improves with `M`. SF12 needs about 2.4 dB less Eb/N0 than SF7
//! and takes 55 times as long to say the same thing; the committed rows are that trade as
//! numbers.
//!
//! **Framing is minimal and stated.** LoRa's preamble, sync word and header are protocol and out
//! of scope (§6): what is here is a run of known symbols keeping the burst's position *searched*
//! rather than known, exactly as the M-FSK entry frames its own bursts.
//!
//! **But the timing estimate is not the M-FSK entry's, and the reason is the waveform's own.** A
//! chirp turns a delay into a frequency shift — dechirping a window that starts `δ` samples early
//! leaves a peak `δ` bins away holding `(1 − δ/N)` of its amplitude — so the energy-maximisation
//! that serves the filterbank and the slot grid is nearly *flat* here, and reading the origin off
//! it decodes whole payloads at BER 0.3 with a perfect signal behind them (measured; the entry's
//! first draft did exactly that). The estimate comes from the *bin* the known preamble lands in
//! instead, which is one window rather than a search — see [`CssDemod::estimate_origin`], where the
//! same ambiguity's second consequence is also recorded: a carrier offset is absorbed into the same
//! number, which is why this entry's §4.3 CFO row is orders above its neighbours'.
//!
//! **Critically sampled.** One sample per chip, so the transform size *is* the symbol length.
//! An oversampled receiver would resolve a fractional chip offset the argmax cannot; that is a
//! second timing tier, not this one, and the §4.3 timing row measures what this one costs.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{constellation::demap::energy_llrs, soft::Llr};

/// Smallest spreading factor the entry defines. Below 7 the chirp is shorter than LoRa's
/// shortest and the entry stops being a parameterisation of anything fielded — but the engine
/// is not stopped there, because the low orders are where the closed form can be cross-checked
/// against the harness's *other* evaluation of it.
pub const MIN_SPREADING_FACTOR: u32 = 5;

/// Largest spreading factor. 12 is LoRa's ceiling and 4096 chips is already a symbol tens of
/// milliseconds long at any sensible bandwidth.
pub const MAX_SPREADING_FACTOR: u32 = 12;

/// One chirp waveform as data: the spreading factor, and nothing else. Bandwidth and sample rate
/// do not appear because at one sample per chip they are the same number, and the engine is
/// rate-free — a catalog row states the Hz so its limits axes read in physical units.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssParams {
    spreading_factor: u32,
}

impl CssParams {
    /// # Panics
    /// If `spreading_factor` is outside `MIN_SPREADING_FACTOR..=MAX_SPREADING_FACTOR`.
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

    /// Chips — and, critically sampled, samples — per symbol: `2^SF`.
    #[must_use]
    pub fn chips(&self) -> usize {
        1 << self.spreading_factor
    }

    /// The orthogonal alphabet size, which is the same number: every cyclic shift is a symbol.
    #[must_use]
    pub fn alphabet(&self) -> usize {
        self.chips()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> usize {
        self.spreading_factor as usize
    }

    /// Eb charged to a frame's own framing, in dB — the same closed form of the geometry the
    /// other spread entries use: `10·log₁₀((P + L)/L)` over symbols of equal energy.
    #[must_use]
    pub fn framing_overhead_db(preamble: usize, payload: usize) -> f64 {
        10.0 * ((preamble + payload) as f64 / payload as f64).log10()
    }

    /// The base up-chirp, `c[n] = exp(j2π(n²/(2N) − n/2))`, at unit amplitude.
    ///
    /// The `−n/2` term centres the sweep on zero frequency, which is what makes the waveform's
    /// occupied band the chip rate rather than twice it; the quadratic term is the sweep. Phases
    /// are reduced modulo one turn *before* the trigonometry, in `f64`, because at SF12 the raw
    /// argument reaches 2π·1024 and rounding it there would smear the transform's own peak.
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

/// The transmitter. Cold path: the base chirp is designed once at construction and every symbol
/// is that chirp times one complex exponential.
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
        // Unit symbol energy over `N` samples, so a CSS Eb/N0 is the same quantity as every
        // other entry's (crate-root convention).
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

    /// A whole burst: the known preamble's symbols, then the payload's. The burst's origin is
    /// `out.len()` as measured when this is called.
    pub fn frame(&self, preamble: &[u32], symbols: &[u32], out: &mut Vec<Complex<f32>>) {
        out.reserve((preamble.len() + symbols.len()) * self.params.chips());
        self.modulate(preamble, out);
        self.modulate(symbols, out);
    }

    /// Symbols alone, appended to `out`.
    pub fn modulate(&self, symbols: &[u32], out: &mut Vec<Complex<f32>>) {
        let n = self.params.chips();
        for &symbol in symbols {
            let shift = symbol as usize % n;
            for k in 0..n {
                // exp(j2πks/N) applied to the base chirp is the cyclic shift; forming it as an
                // index into a turn keeps the phase exact at every SF.
                let turns = (k * shift) as f64 / n as f64;
                let phase = std::f64::consts::TAU * (turns - turns.floor());
                let (sin, cos) = phase.sin_cos();
                let rotation = Complex::new(cos as f32, sin as f32);
                out.push(self.base[k] * rotation * self.amplitude);
            }
        }
    }
}

/// The receiver: dechirp, transform, argmax — plus the preamble-bin timing estimate the argmax
/// needs to be aligned at all.
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

    /// Per-shift energies of the symbol at `origin + symbol·N`, written into `out`, normalised so
    /// a noise-only bin reads mean N0 — the same normalisation the M-FSK filterbank carries, which
    /// is what lets both feed the one energy demapper and answer to the one oracle.
    ///
    /// The transform is `1/√N`-scaled in this direction only (rustfft is unnormalised), which is
    /// exactly the unitary convention that makes a bin's noise the input's.
    ///
    /// # Panics
    /// If `out` is not `N` long.
    pub fn energies(&mut self, iq: &[Complex<f32>], origin: usize, symbol: usize, out: &mut [f32]) {
        assert_eq!(
            out.len(),
            self.params.chips(),
            "one energy per cyclic shift"
        );
        self.fill(iq, origin, symbol);
        out.copy_from_slice(&self.energies);
    }

    /// The same, into the engine's own buffer — the form every method here uses, so the hot path
    /// never needs a caller-supplied slice or an allocation of its own.
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

    /// The burst's origin, read from the *bin* its known preamble lands in.
    ///
    /// **A chirp cannot tell a delay from a carrier offset, and that is what makes this estimator
    /// what it is rather than the energy maximisation the M-FSK and PPM engines use.** Dechirping a
    /// window that starts `δ` samples early leaves
    /// `c[k−δ]·conj(c[k]) = e^{−j2πkδ/N}` times a constant — a pure *frequency* shift, so the peak
    /// moves `δ` bins and keeps `(1 − δ/N)` of its amplitude. Energy concentration is therefore
    /// nearly flat in the origin: measured, a 32-sample error at SF7 costs the peak 2.5 dB, which
    /// any working SNR buries, and the entry's first draft picked its origin essentially at random
    /// and decoded whole payloads at BER 0.3.
    ///
    /// So the estimate is read where the information actually is: each preamble symbol decodes to
    /// `known − δ (mod N)`, so `δ` is the modal difference across the word — one estimate from one
    /// window, no search, robust to a symbol or two decoding wrong. The recoverable range is
    /// `[0, N)`, one symbol; beyond that the ambiguity is genuine.
    ///
    /// **A carrier offset is absorbed into the same number**, because the waveform offers no way to
    /// separate them — and the payload is then read through the same combined correction, so the
    /// entry tolerates far more carrier offset than any of its neighbours. That is the §4.3 CFO
    /// row, and it is a property of chirp spreading rather than of this implementation. (Resolving
    /// the two *is* possible and is what LoRa's down-chirp sync symbols are for: a down-chirp's
    /// peak moves the opposite way, so the sum and difference separate time from frequency. That
    /// is a second acquisition tier, not a parameter here.)
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

    /// `symbols` hard-decided shifts from `origin`, appended to `out`.
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

    /// Per-bit LLRs of `symbols` symbols from `origin`, appended in transmission order (bit `k`
    /// of the shift index at position `k`), through the crate's one energy demapper.
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

    /// N0 estimated from the transform, as the mean of every bin except each symbol's largest —
    /// the M-FSK engine's estimator, valid here for the same reason and with the same stated bias:
    /// under correct detection those bins hold noise alone.
    ///
    /// # Panics
    /// If `symbols` is zero: the estimator would divide no measurement by no sample and hand back
    /// a NaN variance, which `energy_llrs` would then spread over every LLR without a word (§8).
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

/// The argmax over transform bins. [`soft::argmax`](crate::soft::argmax) is the crate's one
/// definition of this decision, but it returns a `u8` and this alphabet reaches 4096; the tie
/// rule — later index wins — is kept identical, because two engines making the same decision
/// differently is how a catalog drifts.
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
    use super::*;
    use crate::ber::rng::Rng;

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

    /// The orthogonality the entry's closed-form acceptance rests on, measured rather than
    /// asserted: any two distinct shifts of the chirp have zero inner product, at every
    /// spreading factor the engine defines.
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

    /// The whole chain round-trips at every spreading factor, and the transmitter is constant
    /// envelope — the property that lets a chirp radio run its amplifier saturated.
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

    /// The timing estimate finds the origin the burst was sent at, at leads that are not whole
    /// symbols, and it does so *under noise* — which is the whole point of reading the preamble's
    /// bin rather than its energy.
    #[test]
    fn the_timing_estimate_finds_the_origin_the_burst_was_sent_at() {
        let params = CssParams::new(7);
        let n = params.chips();
        let preamble = payload(n, 8, 0x7157);
        let mut demod = CssDemod::new(params.clone());
        // Up to 63 of the 128-sample symbol — the last lead inside the stated unambiguous range.
        for lead in [0usize, 1, 17, 40, 63] {
            let mut wave = vec![Complex::new(0.0, 0.0); lead];
            CssMod::new(params.clone()).modulate(&preamble, &mut wave);
            wave.resize(wave.len() + 256, Complex::new(0.0, 0.0));
            add_noise(&mut wave, 0x7158 + lead as u64, 0.05);
            assert_eq!(demod.estimate_origin(&wave, &preamble), lead, "lead {lead}");
        }
        // Past half a symbol the neighbouring symbol owns the window, and the estimate reports
        // that rather than an offset — the stated limit, pinned so it cannot silently move.
        let mut wave = vec![Complex::new(0.0, 0.0); 100];
        CssMod::new(params).modulate(&preamble, &mut wave);
        wave.resize(wave.len() + 256, Complex::new(0.0, 0.0));
        assert_ne!(demod.estimate_origin(&wave, &preamble), 100);
    }

    /// The measurement behind that choice: peak *energy* barely moves with the origin, because a
    /// chirp trades a delay for a frequency shift rather than for lost energy. A 32-sample error
    /// at SF7 keeps three quarters of the amplitude — 2.5 dB — which is why an energy-maximising
    /// search over origins picks essentially at random once there is any noise at all.
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
        // (1 − 32/128)² = 0.5625 of the energy survives a whole 32-sample error…
        assert!(
            (misaligned / aligned - 0.5625).abs() < 0.05,
            "aligned {aligned}, 32 samples early {misaligned}"
        );
        // …and it survives in a *different bin*, which is where the estimate reads it.
        demod.energies(&wave, 0, 1, &mut energies);
        let moved = argmax_bin(&energies);
        demod.energies(&wave, lead, 1, &mut energies);
        let correct = argmax_bin(&energies);
        assert_eq!(correct, symbols[1]);
        assert_eq!(moved, (symbols[1] + n as u32 - lead as u32) % n as u32);
    }

    /// Sensitivity improves with the spreading factor — the entry's reason to exist, and the
    /// ordering the noncoherent orthogonal closed form predicts. Measured at one noise level as a
    /// raw symbol-error count, which is enough to see a monotone ordering without a sweep.
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
            // Es = 1 per symbol carrying SF bits, so a fixed N0 is a *rising* Eb/N0 as SF grows
            // by only 10·log10(SF₂/SF₁); the alphabet gain is what has to beat that.
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

    /// The noise estimate recovers a known N0, which is what turns transform energies into
    /// calibrated LLRs.
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

    /// LLR calibration, same property and same measurement as every other soft path in the
    /// crate.
    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let params = CssParams::new(6);
        let n = params.chips();
        let symbols = payload(n, 6_000, 0x11c6);
        let mut wave = Vec::new();
        CssMod::new(params.clone()).modulate(&symbols, &mut wave);
        // Es/N0 = 9 dB: M = 64 orthogonal signals need roughly that to be decodable at all, and
        // an LLR band structure only exists where the receiver is mostly right.
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

    /// The framing overhead is the same closed form of the geometry the other spread entries
    /// carry, so a shifted oracle is one constant rather than a per-row fudge.
    #[test]
    fn the_framing_overhead_is_the_symbol_count_ratio() {
        assert!(CssParams::framing_overhead_db(0, 64).abs() < 1e-12);
        let overhead = CssParams::framing_overhead_db(8, 256);
        assert!((overhead - 10.0 * (264.0f64 / 256.0).log10()).abs() < 1e-12);
        assert!((overhead - 0.1336).abs() < 1e-3, "{overhead}");
    }
}
