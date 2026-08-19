use num_complex::Complex;

use crate::linalg::{Cholesky, LinalgError, MAX_SOLVE_ORDER};

/// How the clutter canceller is sized: how far in delay the direct path and its reflections
/// reach, how many Doppler hypotheses to remove alongside them, and how long a stretch is treated
/// as stationary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EcaParams {
    pub delay_taps: usize,
    /// Doppler hypotheses either side of zero. Zero removes only stationary clutter, which is
    /// most of it; one or two catch slowly swaying scatterers.
    pub doppler_bins: usize,
    /// Samples per batch. Shorter batches follow clutter that changes, at the cost of cancelling
    /// some of the target with it.
    pub batch: usize,
    pub loading: f32,
}

impl Default for EcaParams {
    fn default() -> Self {
        Self {
            delay_taps: 32,
            doppler_bins: 0,
            batch: 16_384,
            loading: 1e-4,
        }
    }
}

impl EcaParams {
    #[must_use]
    pub const fn order(&self) -> usize {
        self.delay_taps * (2 * self.doppler_bins + 1)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        self.delay_taps > 0 && self.order() <= MAX_SOLVE_ORDER && self.batch >= self.delay_taps
    }
}

/// Extensive cancellation, batch flavour.
///
/// The direct path from the illuminator arrives at the surveillance antenna tens of decibels
/// above anything reflected off a target, and it arrives as a scaled, delayed copy of what the
/// reference antenna already has. Least squares over delayed — and optionally Doppler-shifted —
/// copies of the reference removes exactly that, and leaves whatever the reference cannot explain.
pub struct Eca {
    params: EcaParams,
    sample_rate: f64,
    gram: Vec<Complex<f32>>,
    cross: Vec<Complex<f32>>,
    basis: Vec<Complex<f32>>,
    chol: Cholesky,
    suppression_db: f32,
}

impl Eca {
    pub fn new(params: EcaParams, sample_rate: f64) -> Result<Self, LinalgError> {
        if !params.valid() {
            return Err(LinalgError::Order(params.order()));
        }
        let order = params.order();
        Ok(Self {
            params,
            sample_rate,
            gram: vec![Complex::default(); order * order],
            cross: vec![Complex::default(); order],
            basis: Vec::new(),
            chol: Cholesky::new(order)?,
            suppression_db: 0.0,
        })
    }

    #[must_use]
    pub const fn params(&self) -> &EcaParams {
        &self.params
    }

    /// How much energy the last pass removed, which is the number that says whether the
    /// canceller is working at all.
    #[must_use]
    pub const fn suppression_db(&self) -> f32 {
        self.suppression_db
    }

    /// Writes the part of `surveillance` the reference cannot account for into `out`.
    pub fn cancel(
        &mut self,
        reference: &[Complex<f32>],
        surveillance: &[Complex<f32>],
        out: &mut Vec<Complex<f32>>,
    ) {
        let len = reference.len().min(surveillance.len());
        out.clear();
        out.extend_from_slice(&surveillance[..len]);
        if len <= self.params.delay_taps {
            self.suppression_db = 0.0;
            return;
        }
        let before: f64 = surveillance[..len]
            .iter()
            .map(|s| f64::from(s.norm_sqr()))
            .sum();
        let batch = self.params.batch.max(self.params.delay_taps + 1);
        let mut start = 0;
        while start < len {
            let end = (start + batch).min(len);
            if end - start > self.params.order() {
                self.cancel_batch(reference, surveillance, start, end, out);
            }
            start = end;
        }
        let after: f64 = out.iter().map(|s| f64::from(s.norm_sqr())).sum();
        self.suppression_db = if after > 0.0 && before > 0.0 {
            (10.0 * (before / after).log10()) as f32
        } else {
            0.0
        };
    }

    fn cancel_batch(
        &mut self,
        reference: &[Complex<f32>],
        surveillance: &[Complex<f32>],
        start: usize,
        end: usize,
        out: &mut [Complex<f32>],
    ) {
        let order = self.params.order();
        let count = end - start;
        self.build_basis(reference, start, end);
        for row in 0..order {
            for col in row..order {
                let mut sum = Complex::default();
                for index in 0..count {
                    sum += self.basis[row * count + index].conj() * self.basis[col * count + index];
                }
                self.gram[row * order + col] = sum;
                self.gram[col * order + row] = sum.conj();
            }
        }
        let trace: f32 = (0..order).map(|i| self.gram[i * order + i].re).sum();
        let floor = (trace / order as f32) * self.params.loading;
        for i in 0..order {
            self.gram[i * order + i] += Complex::new(floor.max(f32::MIN_POSITIVE), 0.0);
        }
        for row in 0..order {
            let mut sum = Complex::default();
            for index in 0..count {
                sum += self.basis[row * count + index].conj() * surveillance[start + index];
            }
            self.cross[row] = sum;
        }
        if self.chol.factor(&self.gram).is_err() {
            return;
        }
        self.chol.solve(&mut self.cross);
        for index in 0..count {
            let mut estimate = Complex::default();
            for row in 0..order {
                estimate += self.cross[row] * self.basis[row * count + index];
            }
            out[start + index] = surveillance[start + index] - estimate;
        }
    }

    /// One row per delay tap and Doppler hypothesis, each a copy of the reference shifted the way
    /// that hypothesis says clutter reaches the surveillance antenna.
    fn build_basis(&mut self, reference: &[Complex<f32>], start: usize, end: usize) {
        let count = end - start;
        let order = self.params.order();
        self.basis.clear();
        self.basis.resize(order * count, Complex::default());
        let bins = self.params.doppler_bins as isize;
        let resolution = self.sample_rate / count as f64;
        let mut row = 0;
        for bin in -bins..=bins {
            let step = std::f64::consts::TAU * bin as f64 * resolution / self.sample_rate;
            for tap in 0..self.params.delay_taps {
                for index in 0..count {
                    let source = start + index;
                    let value = if source >= tap {
                        reference[source - tap]
                    } else {
                        Complex::default()
                    };
                    let phase = Complex::from_polar(1.0, (step * index as f64) as f32);
                    self.basis[row * count + index] = value * phase;
                }
                row += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn noise(len: usize, seed: u64, amplitude: f32) -> Vec<Complex<f32>> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                let mut next = || {
                    state ^= state >> 12;
                    state ^= state << 25;
                    state ^= state >> 27;
                    (state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u32 << 23) as f32
                        - 1.0
                };
                Complex::new(next() * amplitude, next() * amplitude)
            })
            .collect()
    }

    fn power(samples: &[Complex<f32>]) -> f64 {
        samples.iter().map(|s| f64::from(s.norm_sqr())).sum()
    }

    #[test]
    fn a_direct_path_is_pushed_far_below_the_target_that_hides_under_it() {
        let len = 8_192;
        let reference = noise(len, 0x51ED, 1.0);
        let direct = Complex::from_polar(20.0f32, 0.6);
        let mut surveillance: Vec<Complex<f32>> = reference
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let leak = if index >= 3 {
                    reference[index - 3] * Complex::from_polar(6.0f32, -1.2)
                } else {
                    Complex::default()
                };
                value * direct + leak
            })
            .collect();
        let echo: Vec<Complex<f32>> = (0..len)
            .map(|index| {
                let source = index.saturating_sub(40);
                reference[source]
                    * Complex::from_polar(0.02f32, 0.0)
                    * Complex::from_polar(1.0f32, TAU * 0.001 * index as f32)
            })
            .collect();
        for (slot, value) in surveillance.iter_mut().zip(&echo) {
            *slot += value;
        }

        let params = EcaParams {
            delay_taps: 16,
            doppler_bins: 0,
            batch: len,
            loading: 1e-6,
        };
        let mut eca = Eca::new(params, 1_000_000.0).expect("sized");
        let mut residual = Vec::new();
        eca.cancel(&reference, &surveillance, &mut residual);

        assert!(
            eca.suppression_db() > 40.0,
            "only {} dB of clutter removed",
            eca.suppression_db()
        );
        let leftover = power(&residual);
        assert!(
            leftover < 4.0 * power(&echo),
            "residual {leftover:.3e} is not down to the echo's own level {:.3e}",
            power(&echo)
        );
    }

    #[test]
    fn a_reference_that_explains_nothing_leaves_the_surveillance_lane_alone() {
        let reference = noise(4_096, 0xAAAA, 1.0);
        let surveillance = noise(4_096, 0xBBBB, 1.0);
        let mut eca = Eca::new(
            EcaParams {
                delay_taps: 8,
                doppler_bins: 0,
                batch: 4_096,
                loading: 1e-4,
            },
            1_000_000.0,
        )
        .expect("sized");
        let mut residual = Vec::new();
        eca.cancel(&reference, &surveillance, &mut residual);
        let kept = power(&residual) / power(&surveillance);
        assert!(kept > 0.9, "unrelated energy was cancelled away: {kept}");
    }

    #[test]
    fn an_order_the_solver_cannot_take_is_refused() {
        let params = EcaParams {
            delay_taps: 4_096,
            ..EcaParams::default()
        };
        assert!(!params.valid());
        assert!(Eca::new(params, 1e6).is_err());
    }

    #[test]
    fn a_block_shorter_than_the_filter_passes_through_untouched() {
        let reference = noise(8, 0x1, 1.0);
        let surveillance = noise(8, 0x2, 1.0);
        let mut eca = Eca::new(EcaParams::default(), 1e6).expect("sized");
        let mut residual = Vec::new();
        eca.cancel(&reference, &surveillance, &mut residual);
        assert_eq!(residual, surveillance);
        assert_eq!(eca.suppression_db(), 0.0);
    }
}
