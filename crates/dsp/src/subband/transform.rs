use num_complex::Complex;

pub(super) struct Inverse25 {
    twiddles: [[Complex<f32>; 4]; 4],
}

impl Inverse25 {
    pub(super) fn new() -> Self {
        Self {
            twiddles: std::array::from_fn(|row| {
                std::array::from_fn(|column| {
                    let angle = std::f64::consts::TAU * ((row + 1) * (column + 1)) as f64 / 25.0;
                    let (sin, cos) = angle.sin_cos();
                    Complex::new(cos as f32, sin as f32)
                })
            }),
        }
    }

    pub(super) fn process(&self, input: &mut [Complex<f32>; 25]) {
        let mut columns = [[Complex::new(0.0, 0.0); 5]; 5];
        for (row, column) in columns.iter_mut().enumerate() {
            *column = inverse5(std::array::from_fn(|index| input[5 * index + row]));
            if row > 0 {
                for (value, twiddle) in column[1..].iter_mut().zip(self.twiddles[row - 1]) {
                    *value *= twiddle;
                }
            }
        }
        for column in 0..5 {
            let result = inverse5(std::array::from_fn(|row| columns[row][column]));
            for (row, value) in result.into_iter().enumerate() {
                input[5 * row + column] = value;
            }
        }
    }
}

fn inverse5(input: [Complex<f32>; 5]) -> [Complex<f32>; 5] {
    let pair1 = input[1] + input[4];
    let pair2 = input[2] + input[3];
    let difference1 = input[1] - input[4];
    let difference2 = input[2] - input[3];
    let even1 = input[0] + pair1 * 0.309_017 - pair2 * 0.809_017;
    let even2 = input[0] - pair1 * 0.809_017 + pair2 * 0.309_017;
    let odd1 = difference1 * 0.951_056_54 + difference2 * 0.587_785_24;
    let odd2 = difference1 * 0.587_785_24 - difference2 * 0.951_056_54;
    let odd1 = Complex::new(-odd1.im, odd1.re);
    let odd2 = Complex::new(-odd2.im, odd2.re);
    [
        input[0] + pair1 + pair2,
        even1 + odd1,
        even2 + odd2,
        even2 - odd2,
        even1 - odd1,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_matches_double_precision_dft_for_impulses_and_noise() {
        let transform = Inverse25::new();
        for case in 0..125 {
            let input = std::array::from_fn(|index| {
                if case < 25 {
                    Complex::new(f32::from(index == case), 0.0)
                } else {
                    Complex::new(
                        ((index * 37 + case * 71) % 251) as f32 / 125.0 - 1.0,
                        ((index * 73 + case * 43) % 257) as f32 / 128.0 - 1.0,
                    )
                }
            });
            let mut actual = input;
            transform.process(&mut actual);
            for (bin, value) in actual.into_iter().enumerate() {
                let expected: Complex<f64> = input
                    .iter()
                    .enumerate()
                    .map(|(index, sample)| {
                        let angle = std::f64::consts::TAU * (index * bin) as f64 / 25.0;
                        Complex::new(f64::from(sample.re), f64::from(sample.im))
                            * Complex::from_polar(1.0, angle)
                    })
                    .sum();
                let error =
                    (Complex::new(f64::from(value.re), f64::from(value.im)) - expected).norm();
                assert!(error < 3e-6, "case={case} bin={bin} error={error}");
            }
        }
    }
}
