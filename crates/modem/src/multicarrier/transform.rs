use num_complex::Complex;

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
