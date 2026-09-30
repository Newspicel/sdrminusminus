use num_complex::Complex;
use sdrmm_dsp::linalg::{
    CMat, Cholesky, Eigen, GeneralEigen, HermitianEigen, LinalgError, MAX_ORDER, MAX_POLY_DEGREE,
    Qr, Roots,
};
use sdrmm_dsp::special::{bessel_j, brent_max, erf, wrap_deg};
use sdrmm_test_support::{CountingAlloc, assert_no_alloc};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc::new();

fn element(row: usize, col: usize) -> Complex<f32> {
    let phase = 0.37 * (row * 7 + col * 3) as f32;
    Complex::from_polar(1.0 / (1.0 + (row + col) as f32), phase)
}

fn covariance(order: usize) -> Result<CMat, LinalgError> {
    let mut matrix = CMat::zeros(order)?;
    for snapshot in 0..order + 4 {
        for row in 0..order {
            for col in 0..order {
                let product = element(snapshot, row) * element(snapshot, col).conj();
                matrix.add(row, col, product);
            }
        }
    }
    for i in 0..order {
        matrix.add(i, i, Complex::new(0.1, 0.0));
    }
    Ok(matrix)
}

#[test]
fn cholesky_and_jacobi_do_not_allocate() {
    let matrix = covariance(MAX_ORDER).unwrap();
    let mut rank_one = CMat::zeros(MAX_ORDER).unwrap();
    for row in 0..MAX_ORDER {
        for col in 0..MAX_ORDER {
            rank_one.set(row, col, element(0, row) * element(0, col).conj());
        }
    }
    let mut eigen = HermitianEigen::new(MAX_ORDER).unwrap();
    let mut values = Eigen::new();
    let mut chol = Cholesky::new(MAX_ORDER).unwrap();
    let mut inverse = CMat::zeros(1).unwrap();
    let mut block = CMat::zeros(1).unwrap();
    let steer = [Complex::new(0.5f32, -0.25); MAX_ORDER];
    let mut scratch = [Complex::new(0.0f32, 0.0); MAX_ORDER];
    let mut solved = [Complex::new(0.0f32, 0.0); MAX_ORDER];
    let mut sink = 0.0f32;
    assert_no_alloc("jacobi and cholesky", || {
        for _ in 0..4 {
            eigen.solve(&matrix, &mut values).unwrap();
            sink += values.values()[MAX_ORDER - 1];
            let loading = chol.factor_loaded(&matrix, 1e-3).unwrap();
            sink += loading + chol.quad_inverse(&steer, &mut scratch);
            chol.solve_into(&steer, &mut solved);
            chol.inverse_into(&mut inverse, &mut scratch).unwrap();
            sink += chol.log_det() + chol.pivot_ratio() + matrix.quad(&steer);
            matrix.copy_block(2, 8, &mut block).unwrap();
            chol.factor_cmat(&block).unwrap();
            chol.factor(block.as_slice()).unwrap();
            sink += chol.factor_loaded(&rank_one, 0.0).unwrap();
        }
    });
    assert!(sink.is_finite());
    assert!(values.values()[0] >= -1e-3);
}

fn expand(roots: &[Complex<f64>]) -> Vec<Complex<f64>> {
    let mut coeffs = vec![Complex::new(1.0f64, 0.0)];
    for &root in roots {
        let mut next = vec![Complex::new(0.0f64, 0.0); coeffs.len() + 1];
        for (k, &value) in coeffs.iter().enumerate() {
            next[k + 1] += value;
            next[k] -= value * root;
        }
        coeffs = next;
    }
    coeffs
}

#[test]
fn qr_schur_and_roots_do_not_allocate() {
    let columns: Vec<_> = (0..MAX_ORDER * 3).map(|i| element(i / 3, i % 3)).collect();
    let mut qr = Qr::new(MAX_ORDER, 3).unwrap();
    let mut basis = vec![Complex::new(0.0f32, 0.0); MAX_ORDER * (MAX_ORDER - 3)];
    let roots_in: Vec<_> = (0..MAX_POLY_DEGREE)
        .map(|k| Complex::from_polar(0.8 + 0.02 * k as f64, 0.9 * k as f64))
        .collect();
    let coeffs = expand(&roots_in);
    let double: Vec<_> = (0..MAX_POLY_DEGREE / 2)
        .flat_map(|k| [Complex::from_polar(1.0, 0.4 * k as f64); 2])
        .collect();
    let double_coeffs = expand(&double);
    let mut fell_back = [true, false];
    let mut roots = Roots::new(MAX_POLY_DEGREE).unwrap();
    let mut found = vec![Complex::new(0.0f64, 0.0); MAX_POLY_DEGREE];
    let companion: Vec<_> = (0..16)
        .map(|i| Complex::new((i % 5) as f64 - 2.0, (i % 3) as f64))
        .collect();
    let mut general = GeneralEigen::new(MAX_POLY_DEGREE).unwrap();
    let mut eigenvalues = [Complex::new(0.0f64, 0.0); 4];
    let mut sink = 0.0f64;
    assert_no_alloc("qr, schur and roots", || {
        qr.factor(&columns).unwrap();
        sink += qr.null_basis(&mut basis).unwrap() as f64;
        general.eigenvalues(&companion, &mut eigenvalues).unwrap();
        sink += roots.solve(&coeffs, &mut found).unwrap() as f64;
        fell_back[0] = roots.fell_back();
        sink += roots.solve(&double_coeffs, &mut found).unwrap() as f64;
        fell_back[1] = roots.fell_back();
        sink += bessel_j(3, 4.5).unwrap() + erf(0.3) + wrap_deg(-190.0);
        sink += brent_max(|x| -(x - 0.2) * (x - 0.2), -1.0, 1.0, 1e-9, 50).0;
    });
    assert!(sink.is_finite());
    assert!(found.iter().all(|root| root.is_finite()));
    assert_eq!(fell_back, [false, true]);
}
