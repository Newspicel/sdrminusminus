use num_complex::Complex;

use crate::linalg::MAX_ORDER;

/// The spatial covariance every direction finder starts from.
///
/// Accumulation is incremental so a processor can keep folding blocks in as they arrive, and the
/// two corrections that make the estimate usable — forward–backward averaging and diagonal
/// loading — are applied when the matrix is read rather than baked into the running sum.
pub struct Covariance {
    order: usize,
    sum: Vec<Complex<f32>>,
    samples: f64,
    forward_backward: bool,
    loading: f32,
}

impl Covariance {
    #[must_use]
    pub fn new(order: usize) -> Self {
        let order = order.clamp(1, MAX_ORDER);
        Self {
            order,
            sum: vec![Complex::default(); order * order],
            samples: 0.0,
            forward_backward: true,
            loading: 1e-3,
        }
    }

    #[must_use]
    pub const fn order(&self) -> usize {
        self.order
    }

    #[must_use]
    pub const fn samples(&self) -> f64 {
        self.samples
    }

    /// Whether `R` is averaged with its own reversed conjugate. Valid for arrays whose element
    /// positions are symmetric about their centre, which covers the uniform circular and linear
    /// geometries; it halves the snapshots a correlated source needs before it resolves.
    pub const fn set_forward_backward(&mut self, on: bool) {
        self.forward_backward = on;
    }

    /// A floor added to the diagonal as a fraction of the average diagonal power. Without it a
    /// short average is singular and the noise subspace is whatever rounding produced.
    pub const fn set_loading(&mut self, fraction: f32) {
        self.loading = fraction;
    }

    pub fn reset(&mut self) {
        self.sum.fill(Complex::default());
        self.samples = 0.0;
    }

    /// Fades what has already been accumulated, so a moving bearing is followed rather than
    /// averaged away.
    pub fn decay(&mut self, factor: f32) {
        for value in &mut self.sum {
            *value *= factor;
        }
        self.samples *= f64::from(factor);
    }

    pub fn accumulate(&mut self, lanes: &[&[Complex<f32>]]) {
        let n = self.order.min(lanes.len());
        let count = lanes
            .iter()
            .take(n)
            .map(|lane| lane.len())
            .min()
            .unwrap_or(0);
        if count == 0 {
            return;
        }
        for row in 0..n {
            for col in row..n {
                let mut sum = Complex::default();
                for (left, right) in lanes[row][..count].iter().zip(&lanes[col][..count]) {
                    sum += left * right.conj();
                }
                self.sum[row * self.order + col] += sum;
                if row != col {
                    self.sum[col * self.order + row] += sum.conj();
                }
            }
        }
        self.samples += count as f64;
    }

    /// Writes the corrected covariance into `out`, row-major.
    pub fn matrix(&self, out: &mut Vec<Complex<f32>>) {
        let n = self.order;
        out.clear();
        out.resize(n * n, Complex::default());
        if self.samples <= 0.0 {
            for i in 0..n {
                out[i * n + i] = Complex::new(1.0, 0.0);
            }
            return;
        }
        let scale = 1.0 / self.samples as f32;
        for (slot, value) in out.iter_mut().zip(&self.sum) {
            *slot = value * scale;
        }
        if self.forward_backward {
            for row in 0..n {
                for col in 0..n {
                    let mirrored = self.sum[(n - 1 - row) * n + (n - 1 - col)].conj() * scale;
                    out[row * n + col] = (out[row * n + col] + mirrored) * 0.5;
                }
            }
        }
        let trace: f32 = (0..n).map(|i| out[i * n + i].re).sum();
        let floor = (trace / n as f32) * self.loading;
        for i in 0..n {
            out[i * n + i] += Complex::new(floor.max(f32::MIN_POSITIVE), 0.0);
            out[i * n + i].im = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    fn snapshots(phases: [f32; 3], len: usize) -> Vec<Vec<Complex<f32>>> {
        (0..3)
            .map(|lane| {
                (0..len)
                    .map(|k| Complex::from_polar(1.0, TAU * 0.01 * k as f32 + phases[lane]))
                    .collect()
            })
            .collect()
    }

    fn borrowed(lanes: &[Vec<Complex<f32>>]) -> Vec<&[Complex<f32>]> {
        lanes.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn one_source_leaves_the_phase_differences_in_the_off_diagonals() {
        let lanes = snapshots([0.0, 0.7, 1.4], 512);
        let mut covariance = Covariance::new(3);
        covariance.set_loading(0.0);
        covariance.accumulate(&borrowed(&lanes));
        let mut r = Vec::new();
        covariance.matrix(&mut r);
        assert!((r[0].re - 1.0).abs() < 1e-3, "{:?}", r[0]);
        assert!((r[1].arg() + 0.7).abs() < 1e-3, "{:?}", r[1]);
        assert!((r[2].arg() + 1.4).abs() < 1e-3, "{:?}", r[2]);
    }

    #[test]
    fn the_matrix_stays_hermitian_with_every_correction_on() {
        let lanes = snapshots([0.0, 0.7, 1.4], 256);
        let mut covariance = Covariance::new(3);
        covariance.accumulate(&borrowed(&lanes));
        let mut r = Vec::new();
        covariance.matrix(&mut r);
        for row in 0..3 {
            assert!(r[row * 3 + row].im.abs() < 1e-6);
            for col in 0..3 {
                assert!(
                    (r[row * 3 + col] - r[col * 3 + row].conj()).norm() < 1e-5,
                    "({row},{col}) breaks the symmetry"
                );
            }
        }
    }

    #[test]
    fn loading_lifts_a_rank_one_matrix_off_the_floor() {
        let lanes = snapshots([0.0, 0.7, 1.4], 128);
        let mut covariance = Covariance::new(3);
        covariance.set_forward_backward(false);
        covariance.set_loading(0.01);
        covariance.accumulate(&borrowed(&lanes));
        let mut r = Vec::new();
        covariance.matrix(&mut r);
        let mut solver = crate::linalg::HermitianEigen::new(3).expect("order");
        let mut eigen = crate::linalg::Eigen::default();
        solver.solve(&r, &mut eigen);
        assert!(eigen.values[0] > 0.0, "{:?}", eigen.values);
        assert!(
            eigen.values[2] > 100.0 * eigen.values[0],
            "{:?}",
            eigen.values
        );
    }

    #[test]
    fn decay_fades_what_came_before() {
        let lanes = snapshots([0.0, 0.0, 0.0], 100);
        let mut covariance = Covariance::new(3);
        covariance.accumulate(&borrowed(&lanes));
        assert!((covariance.samples() - 100.0).abs() < 1e-6);
        covariance.decay(0.5);
        assert!((covariance.samples() - 50.0).abs() < 1e-6);
        covariance.reset();
        assert_eq!(covariance.samples(), 0.0);
    }

    #[test]
    fn an_empty_average_reports_the_identity_rather_than_zeros() {
        let covariance = Covariance::new(4);
        let mut r = Vec::new();
        covariance.matrix(&mut r);
        for row in 0..4 {
            for col in 0..4 {
                let want = if row == col { 1.0 } else { 0.0 };
                assert!((r[row * 4 + col].re - want).abs() < 1e-6);
            }
        }
    }
}
