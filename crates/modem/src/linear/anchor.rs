use num_complex::Complex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorError {
    TooFewAnchors(usize),
    LengthMismatch,
    NoEnergy,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhaseAnchor {
    pub gain: Complex<f64>,
    pub freq_cycles_per_symbol: f64,
    pub misfit: f64,
}

impl PhaseAnchor {
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

    #[must_use]
    pub fn correct(&self, index: usize, y: Complex<f32>) -> Complex<f32> {
        let theta = -std::f64::consts::TAU * self.freq_cycles_per_symbol * index as f64;
        let y = Complex::new(f64::from(y.re), f64::from(y.im));
        let z = y * Complex::new(theta.cos(), theta.sin()) / self.gain;
        Complex::new(z.re as f32, z.im as f32)
    }

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

    #[test]
    fn the_data_aided_gain_beats_blind_power_normalisation() {
        let table = tables::qam_square(16).unwrap();
        let mut rng = Rng::new(0x5ca1e);
        let n = 4_000;
        let sent: Vec<Complex<f32>> = (0..n)
            .map(|_| table.points()[(rng.next_u64() % 16) as usize])
            .collect();
        let mut received = sent.clone();
        Awgn::with_sigma((0.05f64).sqrt()).apply(&mut received, &mut rng);
        let blind_scale = (received
            .iter()
            .map(|s| f64::from(s.norm_sqr()))
            .sum::<f64>()
            / n as f64)
            .sqrt();
        assert!(
            (blind_scale - 1.1f64.sqrt()).abs() < 0.02,
            "blind scale {blind_scale}"
        );
        let fit = PhaseAnchor::fit_gain_only(&received[..64], &sent[..64]).unwrap();
        let aided = fit.gain.norm();
        assert!(
            (aided - 1.0).abs() < 0.03,
            "data-aided gain {aided}, blind {blind_scale}"
        );
        assert!((aided - 1.0).abs() < (blind_scale - 1.0).abs());
    }

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
        let error = (fit.freq_cycles_per_symbol - freq).abs();
        assert!(error < 5e-4, "slope error {error} cycles/symbol ({fit:?})");
        assert!((fit.gain.norm() - 1.0).abs() < 0.05, "{fit:?}");
        assert!(fit.misfit < 0.4, "{fit:?}");

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

    #[test]
    fn the_slope_estimate_wraps_where_the_docs_say_it_does() {
        let x = known_word(16, 0x2b0);
        let indices: Vec<usize> = (0..x.len()).collect();
        let y = impose(&x, Complex::new(1.0, 0.0), 0.3, 0);
        let fit = PhaseAnchor::fit(&indices, &y, &x).unwrap();
        assert!((fit.freq_cycles_per_symbol - 0.3).abs() < 1e-6, "{fit:?}");
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
