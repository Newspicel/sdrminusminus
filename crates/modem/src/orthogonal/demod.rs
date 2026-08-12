//! The noncoherent M-FSK receiver: filterbank energies, a feedforward symbol-timing estimate,
//! and the argmax with its soft counterpart.
//!
//! **Timing is feedforward, one estimate per burst** — the same choice the linear engine made in
//! phase 4, for a reason that is sharper here: a noncoherent detector has no error signal a
//! tracking loop could ride. There is no phase to differentiate and no eye to open; what a
//! misaligned window costs is *energy split between two symbols*, and that is a quantity you
//! maximise over a burst rather than track through one. So the estimator is exactly that
//! maximisation ([`MfskDemod::estimate_offset`]): the offset whose windows collect the most
//! peak-tone energy across the burst.
//!
//! The estimate is over whole samples. A rect matched filter misaligned by δ samples keeps
//! `(1 − δ/N)²` of the symbol's energy, so the residual half-sample costs
//! `−20·log₁₀(1 − 1/(2N))` dB — 0.45 dB at the catalog's 10 samples per symbol, 0.002 dB at
//! FT8's 1920. That is the entry's timing story, and the §4.3 timing row measures it rather
//! than this comment asserting it.

use num_complex::Complex;

use super::{filterbank::ToneBank, params::MfskParams};
use crate::{
    constellation::demap::energy_llrs,
    soft::{Llr, argmax},
};

/// Stack scratch for one symbol's energies — sized by the plan's own ceiling
/// ([`MAX_TONES`](super::params::MAX_TONES)), which is what keeps every method here
/// allocation-free on the hot path (§4.2).
const SCRATCH: usize = super::params::MAX_TONES;

/// One noncoherent M-FSK receiver over one tone plan.
#[derive(Clone, Debug)]
pub struct MfskDemod {
    params: MfskParams,
    bank: ToneBank,
}

impl MfskDemod {
    #[must_use]
    pub fn new(params: MfskParams) -> Self {
        let bank = ToneBank::new(&params);
        Self { params, bank }
    }

    #[must_use]
    pub fn params(&self) -> &MfskParams {
        &self.params
    }

    #[must_use]
    pub fn m(&self) -> usize {
        self.params.m()
    }

    #[must_use]
    pub fn bits_per_symbol(&self) -> u32 {
        self.params.bits_per_symbol()
    }

    /// Per-tone energies of the symbol at `offset + symbol·sps`, written into `out`.
    ///
    /// # Panics
    /// If `out.len() != m`.
    pub fn energies(&self, iq: &[Complex<f32>], offset: usize, symbol: usize, out: &mut [f32]) {
        self.bank
            .energies(iq, offset + symbol * self.params.window(), out);
    }

    /// The feedforward burst timing estimate: the whole-sample offset in `0..sps` whose windows
    /// collect the most peak-tone energy over the first `symbols` symbols. Ties keep the
    /// earliest offset, so a burst already on the grid estimates 0.
    ///
    /// Zero allocation, `O(sps · symbols · M · sps)` — a burst-rate cost paid once, against the
    /// per-sample cost of a loop that would have no error signal to ride anyway.
    #[must_use]
    pub fn estimate_offset(&self, iq: &[Complex<f32>], symbols: usize) -> usize {
        let mut energies = [0.0f32; SCRATCH];
        let energies = &mut energies[..self.m()];
        let mut best = (0usize, f64::NEG_INFINITY);
        for offset in 0..self.params.window() {
            let mut score = 0.0f64;
            for symbol in 0..symbols {
                self.energies(iq, offset, symbol, energies);
                score += f64::from(energies.iter().copied().fold(0.0f32, f32::max));
            }
            if score > best.1 {
                best = (offset, score);
            }
        }
        best.0
    }

    /// `symbols` hard-decided symbols from `offset`, appended to `out`.
    pub fn demodulate(
        &self,
        iq: &[Complex<f32>],
        offset: usize,
        symbols: usize,
        out: &mut Vec<u8>,
    ) {
        let mut energies = [0.0f32; SCRATCH];
        let energies = &mut energies[..self.m()];
        out.reserve(symbols);
        for symbol in 0..symbols {
            self.energies(iq, offset, symbol, energies);
            out.push(argmax(energies));
        }
    }

    /// Per-bit LLRs of `symbols` symbols from `offset`, appended to `out` in transmission order
    /// (`bits_per_symbol` per symbol, bit k of the tone index at position k).
    ///
    /// `noise_var` is N0 in the filterbank's own normalisation — [`noise_var_from_energies`]
    /// measures it from the bank's output, or the §3.4 known-symbol hook supplies it.
    pub fn llrs(
        &self,
        iq: &[Complex<f32>],
        offset: usize,
        symbols: usize,
        noise_var: f64,
        out: &mut Vec<Llr>,
    ) {
        let bits = self.bits_per_symbol() as usize;
        let mut energies = [0.0f32; SCRATCH];
        let mut symbol_llrs = [Llr(0.0); SCRATCH];
        let energies = &mut energies[..self.m()];
        let symbol_llrs = &mut symbol_llrs[..bits];
        out.reserve(symbols * bits);
        for symbol in 0..symbols {
            self.energies(iq, offset, symbol, energies);
            energy_llrs(energies, noise_var, symbol_llrs);
            out.extend_from_slice(symbol_llrs);
        }
    }
}

/// N0 estimated from filterbank output: the mean of every bin *except* each symbol's largest.
///
/// Under correct detection those bins hold noise alone, and each is exponential with mean N0 in
/// the bank's normalisation — so the mean of `symbols·(M−1)` of them has relative accuracy
/// ~`1/√(symbols·(M−1))`. The estimate is biased *low* by exactly the wrong-tone case: when a
/// symbol errs, one discarded bin carried signal and one counted bin did too. That direction is
/// the honest one — it overstates LLR magnitudes only where the receiver is already deep in
/// error — and at the operating points a curve is measured over, the bias is the error rate
/// itself, ≲1e-2.
///
/// `energies` is a flat `symbols × M` block, as [`MfskDemod::energies`] fills one symbol at a
/// time.
///
/// # Panics
/// If `m` is zero or `energies.len()` is not a multiple of `m`.
#[must_use]
pub fn noise_var_from_energies(energies: &[f32], m: usize) -> f64 {
    assert!(m > 1, "a noise estimate needs a bin the signal is not in");
    assert!(
        !energies.is_empty() && energies.len().is_multiple_of(m),
        "energies must be a whole number of {m}-tone symbols"
    );
    let mut sum = 0.0f64;
    for symbol in energies.chunks_exact(m) {
        let peak = symbol.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        sum += symbol.iter().map(|&e| f64::from(e)).sum::<f64>() - f64::from(peak);
    }
    sum / (energies.len() - energies.len() / m) as f64
}

#[cfg(test)]
mod tests {
    use super::{
        super::modulator::{MfskMod, TonePhase},
        *,
    };
    use crate::ber::rng::Rng;

    const SPS: f64 = 10.0;

    fn params(m: usize) -> MfskParams {
        MfskParams::orthogonal(m, SPS)
    }

    fn modulate(params: &MfskParams, policy: TonePhase, symbols: &[u8]) -> Vec<Complex<f32>> {
        let mut m = MfskMod::new(params.clone(), policy);
        let mut out = Vec::new();
        m.modulate(symbols, &mut out);
        m.flush(&mut out);
        out
    }

    fn payload(m: usize, len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as usize % m) as u8
            })
            .collect()
    }

    /// Add complex AWGN of total variance `noise_var` in place.
    fn add_noise(wave: &mut [Complex<f32>], seed: u64, noise_var: f64) {
        let mut rng = Rng::new(seed);
        let sigma = (noise_var / 2.0).sqrt();
        for s in wave.iter_mut() {
            *s += Complex::new((rng.normal() * sigma) as f32, (rng.normal() * sigma) as f32);
        }
    }

    #[test]
    fn every_alphabet_round_trips_clean() {
        for m in [2usize, 4, 8, 16] {
            // The outer tones of a wide alphabet need room below Nyquist: at spacing 1 the
            // plan spans M−1 cycles per symbol, so the window has to be wider than that.
            let p = MfskParams::orthogonal(m, (2 * m) as f64);
            let symbols = payload(m, 200);
            let wave = modulate(&p, TonePhase::Continuous, &symbols);
            let mut out = Vec::new();
            MfskDemod::new(p).demodulate(&wave, 0, symbols.len(), &mut out);
            assert_eq!(out, symbols, "M = {m}");
        }
    }

    /// The entry's claim about its own detector: a noncoherent receiver cannot tell the two
    /// transmitter phase policies apart. Measured on noise, not asserted on a clean signal —
    /// a clean signal would pass on either policy for uninteresting reasons.
    #[test]
    fn both_phase_policies_decode_identically() {
        let p = params(4);
        let symbols = payload(4, 2_000);
        let demod = MfskDemod::new(p.clone());
        let errors = |policy| {
            let mut wave = modulate(&p, policy, &symbols);
            add_noise(&mut wave, 0x0f5c, 1.4);
            let mut out = Vec::new();
            demod.demodulate(&wave, 0, symbols.len(), &mut out);
            out.iter().zip(&symbols).filter(|(a, b)| a != b).count()
        };
        let (continuous, independent) = (
            errors(TonePhase::Continuous),
            errors(TonePhase::Independent),
        );
        // Both realisations see the same noise draw, so the counts are comparable directly;
        // ~100 errors expected, and a policy the detector *did* care about would differ by
        // far more than the ±30% two independent realisations can.
        assert!(continuous > 20 && independent > 20, "too clean to compare");
        let ratio = continuous as f64 / independent as f64;
        assert!(
            (0.7..1.4).contains(&ratio),
            "continuous {continuous} vs independent {independent} errors"
        );
    }

    /// The timing estimator finds the offset a burst was actually sent at — including the one
    /// case a tracking loop would never see, a burst that starts mid-sample-grid.
    #[test]
    fn the_timing_estimate_finds_the_offset_the_burst_was_sent_at() {
        let p = params(4);
        let symbols = payload(4, 64);
        let demod = MfskDemod::new(p.clone());
        for shift in [0usize, 1, 3, 7, 9] {
            let mut wave = vec![Complex::new(0.0, 0.0); shift];
            wave.extend(modulate(&p, TonePhase::Continuous, &symbols));
            assert_eq!(demod.estimate_offset(&wave, 32), shift, "shift {shift}");
            let mut out = Vec::new();
            demod.demodulate(&wave, shift, symbols.len(), &mut out);
            assert_eq!(out, symbols, "shift {shift}");
        }
    }

    /// A wrong offset must cost, or the estimator would be choosing between equals: half a
    /// symbol of misalignment splits every symbol's energy across two windows.
    #[test]
    fn a_half_symbol_offset_loses_most_of_the_energy() {
        let p = params(8);
        let symbols = payload(8, 64);
        let wave = modulate(&p, TonePhase::Continuous, &symbols);
        let demod = MfskDemod::new(p);
        let mut energies = [0.0f32; 8];
        let peak_at = |offset: usize, energies: &mut [f32]| {
            let mut sum = 0.0f64;
            for symbol in 0..60 {
                demod.energies(&wave, offset, symbol, energies);
                sum += f64::from(energies.iter().copied().fold(0.0f32, f32::max));
            }
            sum
        };
        let aligned = peak_at(0, &mut energies);
        let split = peak_at(5, &mut energies);
        assert!(split < 0.6 * aligned, "aligned {aligned}, split {split}");
    }

    /// The noise-variance estimate is what turns bank energies into calibrated LLRs, so it is
    /// measured against a known N0 rather than trusted.
    #[test]
    fn the_noise_estimate_recovers_a_known_n0() {
        let p = params(8);
        let symbols = payload(8, 3_000);
        let mut wave = modulate(&p, TonePhase::Continuous, &symbols);
        add_noise(&mut wave, 0x9711, 2.0);
        let demod = MfskDemod::new(p);
        let mut energies = vec![0.0f32; 8 * symbols.len()];
        for symbol in 0..symbols.len() {
            let at = symbol * 8;
            demod.energies(&wave, 0, symbol, &mut energies[at..at + 8]);
        }
        let estimate = noise_var_from_energies(&energies, 8);
        assert!(
            (estimate - 2.0).abs() < 0.1,
            "estimated N0 {estimate} vs 2.0"
        );
    }

    /// LLR calibration, the property the `Llr` type is a claim about: among bits the receiver
    /// reports at confidence |llr|, the fraction that are wrong must match `1/(1+e^|llr|)`.
    /// Measured in bands over a long run at a realistic operating point.
    #[test]
    fn llr_magnitudes_predict_their_own_error_rate() {
        let p = params(4);
        let symbols = payload(4, 40_000);
        let mut wave = modulate(&p, TonePhase::Continuous, &symbols);
        add_noise(&mut wave, 0x11cb, 1.0);
        let demod = MfskDemod::new(p);
        let mut llrs = Vec::new();
        demod.llrs(&wave, 0, symbols.len(), 1.0, &mut llrs);
        let mut bands = [(0u32, 0u32); 4];
        for (i, &llr) in llrs.iter().enumerate() {
            // Bit k of the tone index rides at position k, per `energy_llrs`' labelling.
            let sent = (symbols[i / 2] >> (i % 2)) & 1 == 1;
            let band = match llr.0.abs() {
                x if x < 1.0 => 0,
                x if x < 2.0 => 1,
                x if x < 4.0 => 2,
                _ => 3,
            };
            bands[band].0 += 1;
            if llr.bit() != sent {
                bands[band].1 += 1;
            }
        }
        for (band, &(count, wrong)) in bands.iter().enumerate() {
            assert!(count > 500, "band {band} saw only {count} bits");
            let measured = f64::from(wrong) / f64::from(count);
            let centre = [0.5f64, 1.5, 3.0, 5.0][band];
            let predicted = 1.0 / (1.0 + centre.exp());
            assert!(
                measured < predicted * 3.0 + 0.02,
                "band {band}: measured {measured}, max-log prediction {predicted}"
            );
        }
    }
}
