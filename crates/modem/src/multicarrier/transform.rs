use std::sync::Arc;

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

#[derive(Clone)]
pub struct Dft {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex<f32>>,
    scale: f32,
    n: usize,
}

impl Dft {
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

    pub fn forward(&mut self, buf: &mut [Complex<f32>]) {
        self.forward.process_with_scratch(buf, &mut self.scratch);
        for v in buf.iter_mut() {
            *v *= self.scale;
        }
    }

    pub fn inverse(&mut self, buf: &mut [Complex<f32>]) {
        self.inverse.process_with_scratch(buf, &mut self.scratch);
        for v in buf.iter_mut() {
            *v *= self.scale;
        }
    }
}

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

    #[test]
    fn the_inverse_is_an_inverse_and_a_singular_matrix_is_refused() {
        let n = 6;
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
        assert!(y[0].norm() < 1e-6);
        assert!((y[1] - Complex::new(3.0, 0.0)).norm() < 1e-6);
    }
}
