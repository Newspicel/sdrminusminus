use num_complex::Complex;

use crate::linalg::{Cholesky, Eigen, HermitianEigen, LinalgError};

/// Weights that add several antennas into one signal, solved from the covariance between them.
///
/// Both answers here are closed-form: nothing adapts sample by sample, so a bank of receivers that
/// stays still keeps the same weights until the scene changes.
pub struct Combiner {
    order: usize,
    eigen: HermitianEigen,
    solution: Eigen,
    auxiliary: Cholesky,
    scratch: Vec<Complex<f32>>,
}

impl Combiner {
    pub fn new(order: usize) -> Result<Self, LinalgError> {
        if order < 2 {
            return Err(LinalgError::Order(order));
        }
        Ok(Self {
            order,
            eigen: HermitianEigen::new(order)?,
            solution: Eigen::default(),
            auxiliary: Cholesky::new(order - 1)?,
            scratch: vec![Complex::default(); order - 1],
        })
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    /// Maximum-ratio combining: the covariance's dominant eigenvector is, by construction, the set
    /// of weights that puts the most of one arrival and the least of the noise into the sum.
    ///
    /// The result is turned so lane zero comes out with zero phase, which leaves the beam's own
    /// phase steady from one solve to the next rather than free to rotate.
    pub fn diversity(&mut self, r: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        self.eigen.solve(r, &mut self.solution);
        let dominant = self.solution.vector(self.order - 1);
        let reference = dominant.first().copied().unwrap_or_default();
        let turn = if reference.norm() > f32::MIN_POSITIVE {
            reference.conj() / reference.norm()
        } else {
            Complex::new(1.0, 0.0)
        };
        out.clear();
        out.extend(dominant.iter().map(|value| value.conj() * turn.conj()));
    }

    /// Sidelobe cancelling: lane zero is the antenna pointed at what you want, the rest are
    /// pointed at what you do not, and the weights subtract the best least-squares estimate of the
    /// interference from the wanted lane.
    ///
    /// Lane zero keeps unit gain, so a signal the reference antennas cannot hear comes through
    /// untouched while one they can hear is nulled.
    pub fn cancel(
        &mut self,
        r: &[Complex<f32>],
        out: &mut Vec<Complex<f32>>,
    ) -> Result<(), LinalgError> {
        let n = self.order;
        let auxiliary = n - 1;
        let mut matrix = vec![Complex::<f32>::default(); auxiliary * auxiliary];
        for row in 0..auxiliary {
            for col in 0..auxiliary {
                matrix[row * auxiliary + col] = r[(row + 1) * n + (col + 1)];
            }
            self.scratch[row] = r[row + 1];
        }
        self.auxiliary.factor(&matrix)?;
        self.auxiliary.solve(&mut self.scratch);
        out.clear();
        out.push(Complex::new(1.0, 0.0));
        out.extend(self.scratch.iter().map(|weight| -weight));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::covariance::Covariance;

    fn covariance(lanes: &[Vec<Complex<f32>>]) -> Vec<Complex<f32>> {
        let mut estimator = Covariance::new(lanes.len());
        estimator.set_forward_backward(false);
        estimator.set_loading(1e-6);
        let views: Vec<&[Complex<f32>]> = lanes.iter().map(Vec::as_slice).collect();
        estimator.accumulate(&views);
        let mut matrix = Vec::new();
        estimator.matrix(&mut matrix);
        matrix
    }

    fn apply(lanes: &[Vec<Complex<f32>>], weights: &[Complex<f32>]) -> Vec<Complex<f32>> {
        (0..lanes[0].len())
            .map(|index| {
                lanes
                    .iter()
                    .zip(weights)
                    .map(|(lane, weight)| lane[index] * weight)
                    .sum()
            })
            .collect()
    }

    fn power(samples: &[Complex<f32>]) -> f32 {
        samples
            .iter()
            .map(num_complex::Complex::norm_sqr)
            .sum::<f32>()
            / samples.len() as f32
    }

    fn tone(len: usize, cycles: f32, amplitude: f32, phase: f32) -> Vec<Complex<f32>> {
        (0..len)
            .map(|index| {
                let angle = std::f32::consts::TAU * cycles * index as f32 / len as f32 + phase;
                Complex::from_polar(amplitude, angle)
            })
            .collect()
    }

    fn noise(len: usize, amplitude: f32, seed: u32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 8) as f32 / (1 << 24) as f32 - 0.5
        };
        (0..len)
            .map(|_| Complex::new(next() * amplitude, next() * amplitude))
            .collect()
    }

    #[test]
    fn combining_two_antennas_puts_the_signal_up_and_the_noise_down() {
        let len = 4_096;
        let signal = tone(len, 37.0, 1.0, 0.0);
        let turned = tone(len, 37.0, 1.0, 1.1);
        let lanes = vec![
            signal
                .iter()
                .zip(noise(len, 1.0, 7))
                .map(|(a, b)| a + b)
                .collect::<Vec<_>>(),
            turned
                .iter()
                .zip(noise(len, 1.0, 99))
                .map(|(a, b)| a + b)
                .collect::<Vec<_>>(),
        ];
        let mut combiner = Combiner::new(2).expect("two lanes");
        let mut weights = Vec::new();
        combiner.diversity(&covariance(&lanes), &mut weights);

        let combined = apply(&lanes, &weights);
        let one = power(&lanes[0]) / power(&noise(len, 1.0, 7));
        let both =
            power(&combined) / power(&apply(&[noise(len, 1.0, 7), noise(len, 1.0, 99)], &weights));
        assert!(
            both > one * 1.6,
            "two antennas should beat one by close to 3 dB: {one} then {both}"
        );
        assert!(
            weights[0].im.abs() < 1e-3,
            "lane zero sets the phase: {:?}",
            weights[0]
        );
    }

    #[test]
    fn a_reference_antenna_takes_the_interference_out() {
        let len = 8_192;
        let wanted = tone(len, 11.0, 0.3, 0.0);
        let interferer = tone(len, 53.0, 3.0, 0.0);
        let leak = Complex::from_polar(0.8, 2.2);
        let lanes = vec![
            wanted
                .iter()
                .zip(&interferer)
                .map(|(a, b)| a + b)
                .collect::<Vec<_>>(),
            interferer.iter().map(|sample| sample * leak).collect(),
        ];
        let mut combiner = Combiner::new(2).expect("two lanes");
        let mut weights = Vec::new();
        combiner
            .cancel(&covariance(&lanes), &mut weights)
            .expect("the reference lane carries power");
        let combined = apply(&lanes, &weights);

        let before = power(&interferer) / power(&wanted);
        let residue = combined
            .iter()
            .zip(&wanted)
            .map(|(out, want)| (out - want).norm_sqr())
            .sum::<f32>()
            / len as f32;
        let after = residue / power(&wanted);
        assert!(before > 90.0, "the interferer starts far louder: {before}");
        assert!(
            after < 1e-3,
            "what is left of the interferer should sit 30 dB under the wanted signal: {after}"
        );
    }

    #[test]
    fn an_array_of_one_has_nothing_to_combine() {
        assert!(Combiner::new(1).is_err());
    }
}
