//! The two pieces of linear algebra all four multicarrier waveforms are built from: a **unitary**
//! transform of arbitrary size, and a dense complex solve.
//!
//! **Unitary in both directions, always.** Each direction carries `1/√N`, so Parseval holds
//! sample for sample: a unit-energy symbol grid produces a unit-energy waveform, and the
//! per-subcarrier Eb/N0 a curve is plotted against is the same quantity as the time-domain one
//! the sweep runner sets. Every entry in this module inherits that, and it is the reason their
//! curves can be compared against the linear engine's closed forms at all. (`ofdm/` makes the
//! same choice for the same reason; the wrapper is repeated here rather than shared because that
//! one is welded to a subcarrier map and this one has to run at `K·M`, `2N` and `M×N` sizes no
//! power of two describes.)
//!
//! **The solve exists for exactly one entry.** GFDM is *not* orthogonal — its subcarriers overlap
//! by construction, which is the point — so a zero-forcing receiver is a genuine matrix inverse
//! and not a per-bin division. Computed once at construction from the modulation matrix the
//! transmitter is defined by, so the receive path stays a matrix–vector product.

use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// A unitary DFT of one fixed size, planned once.
#[derive(Clone)]
pub struct Dft {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    scale: f32,
    n: usize,
}

impl Dft {
    /// # Panics
    /// If `n` is zero — a transform of nothing has no scaling.
    #[must_use]
    pub fn new(n: usize) -> Self {
        assert!(n > 0, "a transform needs at least one point");
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(n);
        let inverse = planner.plan_fft_inverse(n);
        let scratch = vec![
            Complex::new(0.0, 0.0);
            forward
                .get_inplace_scratch_len()
                .max(inverse.get_inplace_scratch_len())
        ];
        Self {
            forward,
            inverse,
            scratch,
            scale: (n as f32).sqrt().recip(),
            n,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// In-place forward transform, scaled by `1/√N`.
    pub fn forward(&mut self, buf: &mut [Complex<f32>]) {
        self.forward.process_with_scratch(buf, &mut self.scratch);
        for v in buf.iter_mut() {
            *v *= self.scale;
        }
    }

    /// In-place inverse transform, scaled by `1/√N`.
    pub fn inverse(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse.process_with_scratch(buf, &mut self.scratch);
        for v in buf.iter_mut() {
            *v *= self.scale;
        }
    }
}

/// Inverts a dense `n × n` complex matrix in row-major order, in place, by Gauss–Jordan with
/// partial pivoting. `None` when the matrix is singular to working precision — which for the one
/// caller means a prototype pulse that cannot be zero-forced, a real design outcome rather than
/// an error to hide.
///
/// `f64` throughout: the matrix is built once at construction and its condition number is the
/// whole quality of the receiver it becomes, so this is the one place in the crate where the
/// signal path's `f32` would be the wrong precision.
#[must_use]
pub fn invert(a: &mut [Complex<f64>], n: usize) -> Option<()> {
    assert_eq!(a.len(), n * n, "matrix is not {n}×{n}");
    let mut inv = vec![Complex::new(0.0, 0.0); n * n];
    for i in 0..n {
        inv[i * n + i] = Complex::new(1.0, 0.0);
    }
    for col in 0..n {
        let (pivot, magnitude) = (col..n).fold((col, 0.0), |(best, mag), row| {
            let candidate = a[row * n + col].norm();
            if candidate > mag {
                (row, candidate)
            } else {
                (best, mag)
            }
        });
        if magnitude < 1e-12 {
            return None;
        }
        if pivot != col {
            for k in 0..n {
                a.swap(pivot * n + k, col * n + k);
                inv.swap(pivot * n + k, col * n + k);
            }
        }
        let scale = a[col * n + col].inv();
        for k in 0..n {
            a[col * n + k] *= scale;
            inv[col * n + k] *= scale;
        }
        for row in 0..n {
            if row == col {
                continue;
            }
            let factor = a[row * n + col];
            if factor == Complex::new(0.0, 0.0) {
                continue;
            }
            for k in 0..n {
                let (a_col, inv_col) = (a[col * n + k], inv[col * n + k]);
                a[row * n + k] -= factor * a_col;
                inv[row * n + k] -= factor * inv_col;
            }
        }
    }
    a.copy_from_slice(&inv);
    Some(())
}

/// `y = A·x` for a row-major `rows × cols` matrix — the one operation a dense receiver performs
/// per block, written once so no entry rolls its own loop order.
pub fn matvec(
    a: &[Complex<f32>],
    rows: usize,
    cols: usize,
    x: &[Complex<f32>],
    y: &mut [Complex<f32>],
) {
    debug_assert_eq!(a.len(), rows * cols);
    debug_assert_eq!(x.len(), cols);
    debug_assert_eq!(y.len(), rows);
    for (slot, chunk) in y.iter_mut().zip(a.chunks_exact(cols)) {
        let mut acc = Complex::new(0.0f32, 0.0);
        for (&coeff, &v) in chunk.iter().zip(x) {
            acc += coeff * v;
        }
        *slot = acc;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unitarity, both halves: a round trip is the identity, and the energy is unchanged in the
    /// middle — which is what makes a per-subcarrier Eb/N0 the same quantity as a time-domain
    /// one.
    #[test]
    fn the_transform_is_unitary_at_any_size() {
        for n in [8usize, 48, 80, 100] {
            let mut dft = Dft::new(n);
            let original: Vec<Complex<f32>> = (0..n)
                .map(|k| Complex::new((k as f32).sin(), (0.7 * k as f32).cos()))
                .collect();
            let energy =
                |x: &[Complex<f32>]| x.iter().map(|v| f64::from(v.norm_sqr())).sum::<f64>();
            let mut buf = original.clone();
            dft.forward(&mut buf);
            assert!(
                (energy(&buf) / energy(&original) - 1.0).abs() < 1e-4,
                "n = {n}: energy moved"
            );
            dft.inverse(&mut buf);
            for (k, (a, b)) in buf.iter().zip(&original).enumerate() {
                assert!((a - b).norm() < 1e-4, "n = {n}, sample {k}");
            }
        }
    }

    /// The solve against a matrix whose inverse is known by construction, and a singular one it
    /// must refuse rather than return noise for.
    #[test]
    fn the_inverse_is_an_inverse_and_a_singular_matrix_is_refused() {
        let n = 6;
        // A well-conditioned complex matrix: diagonally dominant, with structure.
        let mut a: Vec<Complex<f64>> = (0..n * n)
            .map(|i| {
                let (r, c) = (i / n, i % n);
                if r == c {
                    Complex::new(4.0, 0.5)
                } else {
                    Complex::new(0.3 / (1.0 + (r as f64 - c as f64).abs()), -0.2)
                }
            })
            .collect();
        let original = a.clone();
        invert(&mut a, n).expect("well-conditioned");
        for r in 0..n {
            for c in 0..n {
                let entry: Complex<f64> = (0..n).map(|k| original[r * n + k] * a[k * n + c]).sum();
                let want = f64::from(u8::from(r == c));
                assert!((entry - Complex::new(want, 0.0)).norm() < 1e-9, "({r},{c})");
            }
        }
        let mut singular = vec![Complex::new(0.0, 0.0); n * n];
        singular[0] = Complex::new(1.0, 0.0);
        assert!(invert(&mut singular, n).is_none());
    }

    #[test]
    fn matvec_computes_the_product_it_says_it_does() {
        let a = [
            Complex::new(1.0f32, 0.0),
            Complex::new(0.0, 1.0),
            Complex::new(2.0, 0.0),
            Complex::new(0.0, -1.0),
        ];
        let x = [Complex::new(1.0f32, 0.0), Complex::new(0.0, 1.0)];
        let mut y = [Complex::new(0.0f32, 0.0); 2];
        matvec(&a, 2, 2, &x, &mut y);
        // Row 0: 1·1 + j·j = 1 − 1 = 0. Row 1: 2·1 + (−j)(j) = 2 + 1 = 3.
        assert!(y[0].norm() < 1e-6);
        assert!((y[1] - Complex::new(3.0, 0.0)).norm() < 1e-6);
    }
}
