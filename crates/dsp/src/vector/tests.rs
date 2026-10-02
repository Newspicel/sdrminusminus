use super::*;

fn values(len: usize, seed: u32) -> Vec<f32> {
    (0..len)
        .map(|i| {
            ((i as u32).wrapping_mul(2_654_435_761).wrapping_add(seed) % 1_000) as f32 / 500.0 - 1.0
        })
        .collect()
}

fn kernels() -> Vec<Vector> {
    let mut all = vec![Vector::portable()];
    if Vector::detect() != Vector::portable() {
        all.push(Vector::detect());
    }
    all
}

#[test]
fn dot_matches_a_plain_sum_at_every_length() {
    for vector in kernels() {
        for len in [0, 1, 15, 16, 17, 64, 100] {
            let (a, b) = (values(len, 1), values(len, 2));
            let want: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
            assert!((vector.dot(&a, &b) - want).abs() < 1e-4, "len {len}");
        }
    }
}

#[test]
fn axpy_adds_a_scaled_vector() {
    for vector in kernels() {
        let mut y = values(37, 3);
        let x = values(37, 4);
        let want: Vec<f32> = y.iter().zip(&x).map(|(y, x)| y + 0.5 * x).collect();
        vector.axpy(&mut y, 0.5, &x);
        for (got, want) in y.iter().zip(want) {
            assert!((got - want).abs() < 1e-6);
        }
    }
}

#[test]
fn gemm_matches_the_textbook_product_for_ragged_shapes() {
    for vector in kernels() {
        for (rows, depth, cols) in [
            (1, 1, 1),
            (4, 3, 16),
            (5, 7, 33),
            (9, 64, 48),
            (2, 5, 3),
            (1, 64, 192),
            (3, 9, 100),
        ] {
            let (lhs, rhs) = (values(rows * depth, 5), values(depth * cols, 6));
            let mut out = vec![f32::NAN; rows * cols];
            vector.gemm(GemmShape { rows, depth, cols }, &lhs, &rhs, &mut out);
            for r in 0..rows {
                for c in 0..cols {
                    let want: f32 = (0..depth)
                        .map(|k| lhs[r * depth + k] * rhs[k * cols + c])
                        .sum();
                    assert!(
                        (out[r * cols + c] - want).abs() < 1e-4,
                        "{rows}x{depth}x{cols} at {r},{c}"
                    );
                }
            }
        }
    }
}

#[test]
fn gemm_leaves_short_buffers_alone() {
    let mut out = vec![7.0; 4];
    Vector::detect().gemm(
        GemmShape {
            rows: 2,
            depth: 2,
            cols: 2,
        },
        &[1.0; 3],
        &[1.0; 4],
        &mut out,
    );
    assert_eq!(out, [7.0; 4]);
}

#[test]
fn tanh_and_sigmoid_stay_within_float_rounding() {
    for vector in kernels() {
        let inputs: Vec<f32> = (-2_000..=2_000).map(|i| i as f32 / 100.0).collect();
        let mut tanh = inputs.clone();
        let mut sigmoid = inputs.clone();
        vector.tanh(&mut tanh);
        vector.sigmoid(&mut sigmoid);
        for ((&x, t), s) in inputs.iter().zip(tanh).zip(sigmoid) {
            assert!((t - x.tanh()).abs() < 2e-6, "tanh({x}) = {t}");
            assert!(
                (s - 1.0 / (1.0 + (-x).exp())).abs() < 2e-6,
                "sigmoid({x}) = {s}"
            );
        }
    }
}
