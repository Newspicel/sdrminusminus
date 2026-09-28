use num_complex::Complex;

pub const POWER_ITERATIONS: usize = 16;

const STACK_ORDER: usize = 32;

pub fn dominant(
    matrix: &[Complex<f32>],
    order: usize,
    iterations: usize,
    vector: &mut [Complex<f32>],
) -> f32 {
    if order == 0 || vector.len() < order || matrix.len() < order * order {
        vector.fill(Complex::default());
        return 0.0;
    }
    let mut stack = [Complex::default(); STACK_ORDER];
    let mut heap = Vec::new();
    let scratch: &mut [Complex<f32>] = if order <= STACK_ORDER {
        &mut stack[..order]
    } else {
        heap.resize(order, Complex::default());
        &mut heap
    };
    let vector = &mut vector[..order];
    vector.fill(Complex::default());
    vector[strongest_column(matrix, order)] = Complex::new(1.0, 0.0);
    for _ in 0..=iterations {
        multiply(matrix, order, vector, scratch);
        if !normalise(scratch) {
            vector.fill(Complex::default());
            return 0.0;
        }
        vector.copy_from_slice(scratch);
    }
    rayleigh(matrix, order, vector)
}

fn strongest_column(matrix: &[Complex<f32>], order: usize) -> usize {
    let column_power = |column: usize| -> f32 {
        (0..order)
            .map(|row| matrix[row * order + column].norm_sqr())
            .sum()
    };
    if column_power(0) > f32::MIN_POSITIVE {
        return 0;
    }
    (0..order)
        .max_by(|a, b| column_power(*a).total_cmp(&column_power(*b)))
        .unwrap_or(0)
}

fn multiply(
    matrix: &[Complex<f32>],
    order: usize,
    vector: &[Complex<f32>],
    out: &mut [Complex<f32>],
) {
    for (row, value) in out.iter_mut().enumerate() {
        *value = matrix[row * order..(row + 1) * order]
            .iter()
            .zip(vector)
            .map(|(m, v)| m * v)
            .sum();
    }
}

fn normalise(vector: &mut [Complex<f32>]) -> bool {
    let norm = vector
        .iter()
        .map(|value| f64::from(value.norm_sqr()))
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::from(f32::MIN_POSITIVE) {
        return false;
    }
    let scale = (1.0 / norm) as f32;
    for value in vector.iter_mut() {
        *value *= scale;
    }
    true
}

fn rayleigh(matrix: &[Complex<f32>], order: usize, vector: &[Complex<f32>]) -> f32 {
    let mut total = Complex::<f64>::default();
    for (row, left) in vector.iter().enumerate() {
        let product: Complex<f32> = matrix[row * order..(row + 1) * order]
            .iter()
            .zip(vector)
            .map(|(m, v)| m * v)
            .sum();
        let term = left.conj() * product;
        total += Complex::new(f64::from(term.re), f64::from(term.im));
    }
    total.re as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank_one(response: &[Complex<f32>], power: f32, noise: f32) -> Vec<Complex<f32>> {
        let order = response.len();
        let mut matrix = vec![Complex::default(); order * order];
        for row in 0..order {
            for column in 0..order {
                matrix[row * order + column] = response[row] * response[column].conj() * power;
            }
            matrix[row * order + row] += noise;
        }
        matrix
    }

    #[test]
    fn dominant_recovers_a_rank_one_response() {
        let response = [
            Complex::new(1.0, 0.0),
            Complex::from_polar(0.8, 1.0),
            Complex::from_polar(1.3, -2.2),
            Complex::from_polar(0.5, 0.3),
        ];
        let matrix = rank_one(&response, 2.0, 0.01);
        let mut vector = [Complex::default(); 4];
        let lambda = dominant(&matrix, 4, POWER_ITERATIONS, &mut vector);
        let energy: f32 = response.iter().map(Complex::norm_sqr).sum();
        assert!(
            (lambda - (2.0 * energy + 0.01)).abs() < 1e-4 * lambda,
            "{lambda}"
        );
        for (lane, expected) in response.iter().enumerate() {
            let measured = vector[lane] / vector[0];
            assert!(
                (measured - expected).norm() < 1e-4,
                "lane {lane}: {measured} vs {expected}"
            );
        }
        let norm: f32 = vector.iter().map(Complex::norm_sqr).sum();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a_dead_first_lane_still_finds_the_strongest_direction() {
        let response = [
            Complex::new(0.0, 0.0),
            Complex::new(1.0, 0.0),
            Complex::from_polar(0.7, 0.4),
        ];
        let matrix = rank_one(&response, 1.0, 0.0);
        let mut vector = [Complex::default(); 3];
        let lambda = dominant(&matrix, 3, POWER_ITERATIONS, &mut vector);
        assert!((lambda - 1.49).abs() < 1e-4, "{lambda}");
        assert!(vector[0].norm() < 1e-6);
        assert!((vector[2] / vector[1] - response[2]).norm() < 1e-5);
    }

    #[test]
    fn an_empty_matrix_has_no_dominant_direction() {
        let matrix = [Complex::default(); 4];
        let mut vector = [Complex::new(1.0, 0.0); 2];
        assert!(dominant(&matrix, 2, POWER_ITERATIONS, &mut vector).abs() < f32::EPSILON);
        assert!(vector.iter().all(|value| value.norm() == 0.0));
        let identity = [
            Complex::new(1.0, 0.0),
            Complex::default(),
            Complex::default(),
            Complex::new(1.0, 0.0),
        ];
        let mut short = [Complex::new(1.0, 0.0); 1];
        assert!(dominant(&identity, 2, POWER_ITERATIONS, &mut short).abs() < f32::EPSILON);
        assert!(short[0].norm() < f32::EPSILON);
        assert!(dominant(&identity[..3], 2, POWER_ITERATIONS, &mut vector).abs() < f32::EPSILON);
    }
}
