use num_complex::Complex;

use crate::fft::FftPair;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("correction spectrum has {got} bins, expected {expected}")]
pub struct SpectrumLength {
    pub expected: usize,
    pub got: usize,
}

pub struct FastConvolver {
    fft: FftPair,
    len: usize,
    taps: usize,
    response: Vec<Complex<f32>>,
    history: Vec<Complex<f32>>,
    work: Vec<Complex<f32>>,
    pending: Vec<Complex<f32>>,
}

impl FastConvolver {
    #[must_use]
    pub fn new(fft_len: usize, taps: usize) -> Self {
        let len = fft_len.max(1);
        let taps = taps.clamp(1, len);
        let hop = len - taps + 1;
        let mut convolver = Self {
            fft: FftPair::new(len),
            len,
            taps,
            response: vec![Complex::default(); len],
            history: vec![Complex::default(); taps - 1],
            work: vec![Complex::default(); len],
            pending: Vec::with_capacity(hop),
        };
        convolver.set_delay((taps - 1) / 2);
        convolver
    }

    #[must_use]
    pub const fn hop(&self) -> usize {
        self.len - self.taps + 1
    }

    #[must_use]
    pub const fn fft_len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn taps(&self) -> usize {
        self.taps
    }

    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    pub fn set_response(&mut self, spectrum: &[Complex<f32>]) -> Result<(), SpectrumLength> {
        if spectrum.len() != self.len {
            return Err(SpectrumLength {
                expected: self.len,
                got: spectrum.len(),
            });
        }
        let scale = 1.0 / self.len as f32;
        for (bin, value) in self.response.iter_mut().zip(spectrum) {
            *bin = value * scale;
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        self.history.fill(Complex::default());
        self.pending.clear();
    }

    pub fn push(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        let hop = self.hop();
        let mut rest = input;
        while !rest.is_empty() {
            let take = (hop - self.pending.len()).min(rest.len());
            let (head, tail) = rest.split_at(take);
            self.pending.extend_from_slice(head);
            rest = tail;
            if self.pending.len() == hop {
                self.convolve_block(out);
            }
        }
    }

    fn convolve_block(&mut self, out: &mut Vec<Complex<f32>>) {
        let keep = self.taps - 1;
        self.work[..keep].copy_from_slice(&self.history);
        self.work[keep..].copy_from_slice(&self.pending);
        self.history.copy_from_slice(&self.work[self.len - keep..]);
        self.pending.clear();
        self.fft.forward(&mut self.work);
        for (bin, response) in self.work.iter_mut().zip(&self.response) {
            *bin *= response;
        }
        self.fft.inverse(&mut self.work);
        out.extend_from_slice(&self.work[keep..]);
    }

    fn set_delay(&mut self, delay: usize) {
        let scale = 1.0 / self.len as f64;
        for (bin, value) in self.response.iter_mut().enumerate() {
            let angle =
                -std::f64::consts::TAU * ((bin * delay) % self.len) as f64 / self.len as f64;
            *value = Complex::new((angle.cos() * scale) as f32, (angle.sin() * scale) as f32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::array_sync::signals::Gaussian;

    fn spectrum_of(taps: &[Complex<f32>], len: usize) -> Vec<Complex<f32>> {
        let mut spectrum = vec![Complex::default(); len];
        spectrum[..taps.len()].copy_from_slice(taps);
        FftPair::new(len).forward(&mut spectrum);
        spectrum
    }

    fn direct(taps: &[Complex<f32>], input: &[Complex<f32>]) -> Vec<Complex<f32>> {
        (0..input.len())
            .map(|n| {
                taps.iter()
                    .enumerate()
                    .filter(|(k, _)| *k <= n)
                    .map(|(k, tap)| tap * input[n - k])
                    .sum()
            })
            .collect()
    }

    fn push_ragged(
        convolver: &mut FastConvolver,
        input: &[Complex<f32>],
        sizes: &[usize],
    ) -> Vec<Complex<f32>> {
        let mut output = Vec::new();
        let mut block = Vec::new();
        let mut at = 0;
        for &size in sizes.iter().cycle() {
            let end = (at + size).min(input.len());
            block.clear();
            convolver.push(&input[at..end], &mut block);
            output.extend_from_slice(&block);
            at = end;
            if at == input.len() {
                break;
            }
        }
        output
    }

    #[test]
    fn fast_convolver_matches_direct_convolution() {
        let mut noise = Gaussian::new(3);
        let taps = noise.block(129);
        let input = noise.block(20_000);
        let mut convolver = FastConvolver::new(4096, taps.len());
        convolver.set_response(&spectrum_of(&taps, 4096)).unwrap();
        let output = push_ragged(&mut convolver, &input, &[1, 700, 3968, 5000, 13]);
        let expected = direct(&taps, &input);
        assert_eq!(output.len(), input.len() - convolver.pending());
        let error: f32 = output
            .iter()
            .zip(&expected)
            .map(|(a, b)| (a - b).norm_sqr())
            .sum();
        let power: f32 = expected[..output.len()].iter().map(Complex::norm_sqr).sum();
        assert!((error / power).sqrt() < 1e-5, "{}", (error / power).sqrt());
    }

    #[test]
    fn fast_convolver_outputs_as_many_samples_as_it_is_given() {
        let mut convolver = FastConvolver::new(4096, 129);
        assert_eq!(convolver.hop(), 3968);
        let input = Gaussian::new(9).block(4 * 3968 + 11);
        let mut given = 0;
        let mut produced = 0;
        let mut block = Vec::new();
        for size in [0, 1, 3966, 2, 4000, 7000, 914] {
            block.clear();
            convolver.push(&input[given..given + size], &mut block);
            given += size;
            produced += block.len();
            assert_eq!(block.len() % convolver.hop(), 0);
            assert_eq!(produced + convolver.pending(), given);
            assert!(convolver.pending() < convolver.hop());
        }
        assert_eq!(given, input.len());
        assert_eq!(produced, 4 * 3968);
    }

    #[test]
    fn a_new_convolver_delays_by_half_its_taps() {
        let mut convolver = FastConvolver::new(256, 33);
        let input = Gaussian::new(5).block(convolver.hop() * 3);
        let output = push_ragged(&mut convolver, &input, &[50]);
        for (index, value) in output.iter().enumerate().skip(16) {
            assert!((value - input[index - 16]).norm() < 1e-5);
        }
        assert!(output[..16].iter().all(|value| value.norm() < 1e-5));
    }

    #[test]
    fn a_spectrum_of_the_wrong_length_is_refused() {
        let mut convolver = FastConvolver::new(64, 9);
        let before = push_ragged(&mut convolver, &Gaussian::new(4).block(56), &[56]);
        assert_eq!(
            convolver.set_response(&[Complex::new(1.0, 0.0); 63]),
            Err(SpectrumLength {
                expected: 64,
                got: 63
            })
        );
        convolver.reset();
        let after = push_ragged(&mut convolver, &Gaussian::new(4).block(56), &[56]);
        assert_eq!(before, after);
    }

    #[test]
    fn a_reset_forgets_history_and_pending_input() {
        let mut noise = Gaussian::new(11);
        let taps = noise.block(17);
        let input = noise.block(900);
        let mut convolver = FastConvolver::new(128, taps.len());
        convolver.set_response(&spectrum_of(&taps, 128)).unwrap();
        let first = push_ragged(&mut convolver, &input, &[37]);
        convolver.push(&noise.block(55), &mut Vec::new());
        convolver.reset();
        assert_eq!(convolver.pending(), 0);
        let second = push_ragged(&mut convolver, &input, &[37]);
        assert_eq!(first, second);
    }
}
