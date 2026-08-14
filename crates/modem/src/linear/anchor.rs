//! The known-symbol hook ( §3.4), linear form: "positions i..j carry known sequence
//! S", turned into a complex gain and a frequency offset by least squares over exactly those
//! positions. This is the *pilot-aided* arm of the coherent tier list, and it is what the CPM
//! engine's [`KnownSymbols`](crate::cpm::KnownSymbols) is for that engine — the same idea, in
//! the domain where amplitude and phase are one complex number instead of two real estimates.
//!
//! **Three things it fixes that a loop cannot.**
//!
//! - *The M-fold phase ambiguity.* A blind carrier loop locks to a rotation of the table, never
//!   to the table (see [`carrier`](super::carrier)). Known symbols name the right one.
//! - *Absolute amplitude.* The blind normaliser in [`demod`](super::demod) scales the symbol
//!   stream to unit mean *power*, which under AWGN is Es + N0, not Es — so the table it hands
//!   the demapper is shrunk by √(1 + 1/SNR): 4.6 % at 10 dB Es/N0, 1.5 % at 15. That is
//!   invisible to a constant-modulus slicer and steadily less so as a QAM table gains rings.
//!   A data-aided fit has no such bias, because it compares against what was actually sent.
//! - *Residual frequency.* Fitting a slope across the anchors, rather than a constant, removes
//!   the offset a short burst gives a loop no time to acquire.
//!
//! **The estimator.** With `r_k = y_k · conj(x_k)` at the known positions, a pure offset
//! `y = g·e^{j2πfk}·x` makes `r_k = g|x_k|²e^{j2πfk}`, so consecutive anchors' product
//! `r_{k+1}·conj(r_k)` has phase `2πf·Δk` free of the data. Averaging those angles is Kay's
//! estimator (*A fast and accurate single frequency estimator*, IEEE ASSP-37, 1989), and the
//! weighting is what makes it worth using: the parabolic window `w_k ∝ (k+1)(N−1−k)` is the one
//! that reaches the Cramér–Rao bound, where a flat average does not. The difference is not
//! academic: on 64 anchors at 10 dB a flat average scatters the slope far enough to turn the far
//! end of the very word it was fitted on by a large fraction of a turn, taking the gain estimate
//! with it. Each term additionally carries its own magnitude, so a weak anchor contributes as
//! little as its energy says it should. The gain then follows as the de-sloped average.
//!
//! **Give it a constant-modulus word.** Measured on 64 anchors at 10 dB: a QPSK word fits the
//! slope inside 5e-4 cycles/symbol, a 16-QAM word only to 3.1e-3 — because a 16-QAM inner point
//! carries a tenth of the mean energy and its phase is nearly unreadable at the SNR the outer
//! points are comfortable at. This is why standards put constant-modulus preambles in front of
//! QAM payloads, and an entry that hands this fit an amplitude-varying word gets the worse
//! number.
//!
//! **Unwrapping bounds the range, and that is stated rather than hidden**: `arg` of a
//! single-step product wraps past `|f·Δk| = ½`, so with contiguous anchors (Δk = 1) the fit
//! covers ±0.5 cycles/symbol — everything — and with anchors spread Δk apart it covers
//! ±1/(2Δk). Scattered pilots therefore need to be closer together than half the reciprocal of
//! the offset they must catch, which is the ordinary pilot-spacing rule stated for this fit.

use num_complex::Complex;

/// Why an anchor fit was refused. The estimate is only applied when it is believable: a fit
/// from noise that happened to match a pattern would corrupt exactly the symbols the anchor
/// exists to protect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorError {
    /// Fewer than two anchors: one names a phase but no slope, and this estimator's whole
    /// purpose is the pair. Use [`PhaseAnchor::fit_gain_only`] where a single anchor is all
    /// there is.
    TooFewAnchors(usize),
    /// The received and expected slices do not pair one to one.
    LengthMismatch,
    /// Every anchor landed at the origin, so there is no phase to read.
    NoEnergy,
}

/// A fitted complex gain and frequency offset, and the correction they define.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseAnchor {
    /// Complex gain: what the channel multiplied the transmitted point by, at index 0.
    pub gain: Complex<f64>,
    /// Residual carrier offset in cycles per symbol.
    pub freq_cycles_per_symbol: f64,
    /// RMS distance between the corrected anchors and what was sent, relative to the
    /// transmitted RMS — the fit's own residual, which is how a caller decides whether to
    /// believe it (the CPM hook's `misfit_bound`, in the units this domain has).
    pub misfit: f64,
}

impl PhaseAnchor {
    /// Fits gain and frequency from anchors at the given symbol indices. `indices`, `received`
    /// and `expected` pair up one to one; indices must ascend (they are positions in the symbol
    /// stream, and the slope is read across their gaps).
    ///
    /// # Errors
    /// [`AnchorError`] — see its variants.
    pub fn fit(
        indices: &[usize],
        received: &[Complex<f32>],
        expected: &[Complex<f32>],
    ) -> Result<Self, AnchorError> {
        if indices.len() != received.len() || indices.len() != expected.len() {
            return Err(AnchorError::LengthMismatch);
        }
        if indices.len() < 2 {
            return Err(AnchorError::TooFewAnchors(indices.len()));
        }
        let r: Vec<Complex<f64>> = received
            .iter()
            .zip(expected)
            .map(|(&y, &x)| {
                let y = Complex::new(f64::from(y.re), f64::from(y.im));
                let x = Complex::new(f64::from(x.re), f64::from(x.im));
                y * x.conj()
            })
            .collect();
        // Kay-weighted phase slope: the parabolic window over the anchor pairs, times each
        // pair's own magnitude, times the reciprocal of the index gap it spans.
        let pairs = r.len() - 1;
        let mut slope_acc = 0.0f64;
        let mut slope_weight = 0.0f64;
        for k in 1..r.len() {
            let gap = indices[k].saturating_sub(indices[k - 1]) as f64;
            if gap <= 0.0 {
                continue;
            }
            let product = r[k] * r[k - 1].conj();
            let position = (k - 1) as f64;
            let window = (position + 1.0) * (pairs as f64 - position);
            let weight = product.norm() * window;
            if weight > 0.0 {
                slope_acc += weight * (product.arg() / gap);
                slope_weight += weight;
            }
        }
        if slope_weight <= 0.0 {
            return Err(AnchorError::NoEnergy);
        }
        let freq = slope_acc / slope_weight / std::f64::consts::TAU;

        // De-slope, then the gain is the energy-weighted mean: g = Σ r_k e^{-j2πfk} / Σ|x_k|².
        let mut num = Complex::new(0.0f64, 0.0);
        let mut den = 0.0f64;
        for (k, (&index, &x)) in indices.iter().zip(expected).enumerate() {
            let theta = -std::f64::consts::TAU * freq * index as f64;
            num += r[k] * Complex::new(theta.cos(), theta.sin());
            den += f64::from(x.norm_sqr());
        }
        if den <= 0.0 || num.norm() <= 0.0 {
            return Err(AnchorError::NoEnergy);
        }
        let gain = num / den;

        let anchor = Self {
            gain,
            freq_cycles_per_symbol: freq,
            misfit: 0.0,
        };
        Ok(Self {
            misfit: anchor.residual(indices, received, expected),
            ..anchor
        })
    }

    /// The single-anchor case: gain only, no slope. A block whose known symbols are one
    /// contiguous word short enough that no offset accumulates across it wants this — and so
    /// does any caller with exactly one pilot.
    ///
    /// # Errors
    /// [`AnchorError::LengthMismatch`] or [`AnchorError::NoEnergy`].
    pub fn fit_gain_only(
        received: &[Complex<f32>],
        expected: &[Complex<f32>],
    ) -> Result<Self, AnchorError> {
        if received.len() != expected.len() {
            return Err(AnchorError::LengthMismatch);
        }
        let mut num = Complex::new(0.0f64, 0.0);
        let mut den = 0.0f64;
        for (&y, &x) in received.iter().zip(expected) {
            let y = Complex::new(f64::from(y.re), f64::from(y.im));
            let x = Complex::new(f64::from(x.re), f64::from(x.im));
            num += y * x.conj();
            den += x.norm_sqr();
        }
        if den <= 0.0 || num.norm() <= 0.0 {
            return Err(AnchorError::NoEnergy);
        }
        let indices: Vec<usize> = (0..received.len()).collect();
        let anchor = Self {
            gain: num / den,
            freq_cycles_per_symbol: 0.0,
            misfit: 0.0,
        };
        Ok(Self {
            misfit: anchor.residual(&indices, received, expected),
            ..anchor
        })
    }

    /// The correction at symbol index `k`: undo the slope, then the gain. Applying it to the
    /// anchors themselves returns the transmitted points up to noise, which is what
    /// [`Self::misfit`] measures.
    #[must_use]
    pub fn correct(&self, index: usize, y: Complex<f32>) -> Complex<f32> {
        let theta = -std::f64::consts::TAU * self.freq_cycles_per_symbol * index as f64;
        let y = Complex::new(f64::from(y.re), f64::from(y.im));
        let z = y * Complex::new(theta.cos(), theta.sin()) / self.gain;
        Complex::new(z.re as f32, z.im as f32)
    }

    /// [`Self::correct`] across a whole block in place, indices counted from `first_index`.
    pub fn correct_block(&self, first_index: usize, symbols: &mut [Complex<f32>]) {
        for (k, s) in symbols.iter_mut().enumerate() {
            *s = self.correct(first_index + k, *s);
        }
    }

    fn residual(
        &self,
        indices: &[usize],
        received: &[Complex<f32>],
        expected: &[Complex<f32>],
    ) -> f64 {
        let mut err = 0.0f64;
        let mut energy = 0.0f64;
        for ((&index, &y), &x) in indices.iter().zip(received).zip(expected) {
            let c = self.correct(index, y);
            err += f64::from((c - x).norm_sqr());
            energy += f64::from(x.norm_sqr());
        }
        if energy > 0.0 {
            (err / energy).sqrt()
        } else {
            f64::INFINITY
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ber::{
            impair::{Awgn, Impairment},
            rng::Rng,
        },
        constellation::tables,
    };

    fn known_word(n: usize, seed: u64) -> Vec<Complex<f32>> {
        let table = tables::qam_square(16).unwrap();
        let mut rng = Rng::new(seed);
        (0..n)
            .map(|_| table.points()[(rng.next_u64() % 16) as usize])
            .collect()
    }

    fn impose(
        x: &[Complex<f32>],
        gain: Complex<f32>,
        freq: f64,
        first: usize,
    ) -> Vec<Complex<f32>> {
        x.iter()
            .enumerate()
            .map(|(k, &s)| {
                let theta = std::f64::consts::TAU * freq * (first + k) as f64;
                s * gain * Complex::new(theta.cos() as f32, theta.sin() as f32)
            })
            .collect()
    }

    /// The noiseless fit is exact: gain and slope come back to f32 precision, and the residual
    /// says so.
    #[test]
    fn a_clean_fit_recovers_gain_and_frequency_exactly() {
        let x = known_word(24, 0xa9c);
        let gain = Complex::new(0.83f32, -0.51);
        let freq = 2.5e-3;
        let y = impose(&x, gain, freq, 0);
        let indices: Vec<usize> = (0..x.len()).collect();
        let fit = PhaseAnchor::fit(&indices, &y, &x).unwrap();
        assert!((fit.freq_cycles_per_symbol - freq).abs() < 1e-9, "{fit:?}");
        assert!((fit.gain.re - f64::from(gain.re)).abs() < 1e-6, "{fit:?}");
        assert!((fit.gain.im - f64::from(gain.im)).abs() < 1e-6, "{fit:?}");
        assert!(fit.misfit < 1e-5, "{fit:?}");
    }

    /// What the anchor is *for*: a block carrying a rotation and a wrong scale comes back on
    /// the table, at positions the fit never saw.
    #[test]
    fn the_correction_lands_a_whole_block_back_on_the_table() {
        let table = tables::qam_square(16).unwrap();
        let mut rng = Rng::new(0x8100);
        let payload: Vec<Complex<f32>> = (0..500)
            .map(|_| table.points()[(rng.next_u64() % 16) as usize])
            .collect();
        let word = known_word(32, 0x9ce);
        let gain = Complex::new(1.7f32, 0.9);
        let freq = -1.2e-3;
        let sent: Vec<Complex<f32>> = word.iter().chain(&payload).copied().collect();
        let received = impose(&sent, gain, freq, 0);
        let indices: Vec<usize> = (0..word.len()).collect();
        let fit = PhaseAnchor::fit(&indices, &received[..word.len()], &word).unwrap();
        let mut corrected = received.clone();
        fit.correct_block(0, &mut corrected);
        let worst = corrected
            .iter()
            .zip(&sent)
            .map(|(a, b)| f64::from((a - b).norm()))
            .fold(0.0, f64::max);
        assert!(worst < 1e-3, "worst residual {worst}");
    }

    /// Under noise the fit must still beat the blind alternative it exists to replace: scaling
    /// the block to unit mean power leaves the √(1 + 1/SNR) bias documented at the module head,
    /// and the data-aided fit does not.
    #[test]
    fn the_data_aided_gain_beats_blind_power_normalisation() {
        let table = tables::qam_square(16).unwrap();
        let mut rng = Rng::new(0x5ca1e);
        let n = 4_000;
        let sent: Vec<Complex<f32>> = (0..n)
            .map(|_| table.points()[(rng.next_u64() % 16) as usize])
            .collect();
        // Es/N0 = 10 dB: total noise variance 0.1 against unit mean symbol energy.
        let mut received = sent.clone();
        Awgn::with_sigma((0.05f64).sqrt()).apply(&mut received, &mut rng);
        let blind_scale = (received
            .iter()
            .map(|s| f64::from(s.norm_sqr()))
            .sum::<f64>()
            / n as f64)
            .sqrt();
        // The blind estimate is high by √(1 + N0/Es) = √1.1 ≈ 1.0488.
        assert!(
            (blind_scale - 1.1f64.sqrt()).abs() < 0.02,
            "blind scale {blind_scale}"
        );
        // Gain only: the honest counterpart, since there is no offset here to fit a slope to.
        let fit = PhaseAnchor::fit_gain_only(&received[..64], &sent[..64]).unwrap();
        let aided = fit.gain.norm();
        assert!(
            (aided - 1.0).abs() < 0.03,
            "data-aided gain {aided}, blind {blind_scale}"
        );
        assert!((aided - 1.0).abs() < (blind_scale - 1.0).abs());
    }

    /// The slope estimator under noise, which is what the Kay weighting is for: a real offset
    /// must come back accurately enough that de-sloping the *far end* of the fitted word is
    /// still correct — the failure mode a flat average has, where the slope's own scatter
    /// rotates the block it was fitted on.
    ///
    /// The known word is QPSK even though the payload is 16-QAM, and that is the finding rather
    /// than a convenience: on a 16-QAM word the same fit scatters by 3.1e-3 cycles/symbol, an
    /// order past the constant-modulus case, because a 16-QAM inner point carries a tenth of the
    /// mean energy and its phase is nearly unreadable at the SNR the outer points are fine at.
    /// Real standards put constant-modulus preambles in front of QAM payloads for exactly this
    /// reason, and an entry that hands this fit an amplitude-varying word gets the worse number.
    #[test]
    fn the_slope_survives_noise_well_enough_to_deslope_its_own_block() {
        let qpsk = tables::psk_rotated(4, std::f64::consts::FRAC_PI_4).unwrap();
        let mut rng = Rng::new(0x510e);
        let word: Vec<Complex<f32>> = (0..64)
            .map(|_| qpsk.points()[(rng.next_u64() % 4) as usize])
            .collect();
        let freq = 1.5e-3;
        let mut received = impose(&word, Complex::new(1.0, 0.0), freq, 0);
        Awgn::with_sigma((0.05f64).sqrt()).apply(&mut received, &mut rng);
        let indices: Vec<usize> = (0..word.len()).collect();
        let fit = PhaseAnchor::fit(&indices, &received, &word).unwrap();
        // A slope error of 1/(4·63) ≈ 4e-3 cycles/symbol would already turn the word's far end
        // by a quarter turn; the estimator must stay well inside that.
        let error = (fit.freq_cycles_per_symbol - freq).abs();
        assert!(error < 5e-4, "slope error {error} cycles/symbol ({fit:?})");
        assert!((fit.gain.norm() - 1.0).abs() < 0.05, "{fit:?}");
        assert!(fit.misfit < 0.4, "{fit:?}");

        // The same fit on an amplitude-varying word, to hold the documented contrast to a number.
        let qam = tables::qam_square(16).unwrap();
        let mut rng = Rng::new(0x510e);
        let word: Vec<Complex<f32>> = (0..64)
            .map(|_| qam.points()[(rng.next_u64() % 16) as usize])
            .collect();
        let mut received = impose(&word, Complex::new(1.0, 0.0), freq, 0);
        Awgn::with_sigma((0.05f64).sqrt()).apply(&mut received, &mut rng);
        let qam_fit = PhaseAnchor::fit(&indices, &received, &word).unwrap();
        assert!(
            (qam_fit.freq_cycles_per_symbol - freq).abs() > 4.0 * error,
            "the QAM word fitted as well as the constant-modulus one: {qam_fit:?}"
        );
    }

    /// The M-fold ambiguity, which is the anchor's headline job: a QPSK block rotated by a
    /// whole table symmetry is indistinguishable to any blind loop, and the fit undoes it.
    #[test]
    fn the_fit_resolves_a_whole_table_symmetry() {
        let table = tables::psk(4).unwrap();
        let word: Vec<Complex<f32>> = (0..16).map(|i| table.points()[i % 4]).collect();
        let quarter = Complex::new(0.0f32, 1.0);
        let received: Vec<Complex<f32>> = word.iter().map(|&s| s * quarter).collect();
        let indices: Vec<usize> = (0..word.len()).collect();
        let fit = PhaseAnchor::fit_gain_only(&received, &word).unwrap();
        assert!(
            (fit.gain.arg() - std::f64::consts::FRAC_PI_2).abs() < 1e-6,
            "{fit:?}"
        );
        for (k, (&y, &x)) in received.iter().zip(&word).enumerate() {
            assert!((fit.correct(indices[k], y) - x).norm() < 1e-6);
        }
    }

    /// The stated unwrapping bound: contiguous anchors cover any offset, and a gap of Δk halves
    /// the reach Δk times. Measured at exactly the edge on each side of it.
    #[test]
    fn the_slope_estimate_wraps_where_the_docs_say_it_does() {
        let x = known_word(16, 0x2b0);
        // Contiguous: 0.3 cycles/symbol is well inside ±0.5 and comes back.
        let indices: Vec<usize> = (0..x.len()).collect();
        let y = impose(&x, Complex::new(1.0, 0.0), 0.3, 0);
        let fit = PhaseAnchor::fit(&indices, &y, &x).unwrap();
        assert!((fit.freq_cycles_per_symbol - 0.3).abs() < 1e-6, "{fit:?}");
        // Spread four apart: the same offset is past ±1/8 and aliases, as documented.
        let spread: Vec<usize> = (0..x.len()).map(|k| 4 * k).collect();
        let y: Vec<Complex<f32>> = spread
            .iter()
            .zip(&x)
            .map(|(&k, &s)| {
                let theta = std::f64::consts::TAU * 0.3 * k as f64;
                s * Complex::new(theta.cos() as f32, theta.sin() as f32)
            })
            .collect();
        let fit = PhaseAnchor::fit(&spread, &y, &x).unwrap();
        assert!(
            (fit.freq_cycles_per_symbol - 0.3).abs() > 0.05,
            "a gap of 4 should alias 0.3 cycles/symbol, read {fit:?}"
        );
        // …and an offset inside the reduced reach still comes back.
        let y: Vec<Complex<f32>> = spread
            .iter()
            .zip(&x)
            .map(|(&k, &s)| {
                let theta = std::f64::consts::TAU * 0.05 * k as f64;
                s * Complex::new(theta.cos() as f32, theta.sin() as f32)
            })
            .collect();
        let fit = PhaseAnchor::fit(&spread, &y, &x).unwrap();
        assert!((fit.freq_cycles_per_symbol - 0.05).abs() < 1e-6, "{fit:?}");
    }

    #[test]
    fn degenerate_inputs_are_refused_rather_than_fitted() {
        let x = known_word(4, 0x33);
        assert_eq!(
            PhaseAnchor::fit(&[0], &x[..1], &x[..1]).unwrap_err(),
            AnchorError::TooFewAnchors(1)
        );
        assert_eq!(
            PhaseAnchor::fit(&[0, 1], &x[..2], &x[..3]).unwrap_err(),
            AnchorError::LengthMismatch
        );
        let zeros = vec![Complex::new(0.0f32, 0.0); 4];
        assert_eq!(
            PhaseAnchor::fit(&[0, 1, 2, 3], &zeros, &x).unwrap_err(),
            AnchorError::NoEnergy
        );
        assert_eq!(
            PhaseAnchor::fit_gain_only(&zeros, &zeros).unwrap_err(),
            AnchorError::NoEnergy
        );
    }
}
