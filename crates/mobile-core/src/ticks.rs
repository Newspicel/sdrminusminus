const STEPS: [f64; 3] = [1.0, 2.0, 5.0];

#[cfg_attr(not(test), expect(dead_code))]
pub(crate) fn nice(min: f64, max: f64, count: u32) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() || max <= min || count == 0 {
        return Vec::new();
    }
    let step = Step::fit((max - min) / f64::from(count));
    let size = step.at(1);
    if !size.is_finite() || size <= 0.0 {
        return Vec::new();
    }
    let first = (min / size).ceil() as i64;
    let last = (max / size).floor() as i64;
    (first..=last).map(|index| step.at(index)).collect()
}

#[derive(Clone, Copy)]
struct Step {
    factor: f64,
    exponent: i32,
}

impl Step {
    fn fit(raw: f64) -> Self {
        let exponent = raw.log10().floor() as i32;
        let decade = 10f64.powi(exponent);
        let factor = STEPS
            .into_iter()
            .find(|factor| factor * decade >= raw)
            .unwrap_or(10.0);
        Self { factor, exponent }
    }

    fn at(self, index: i64) -> f64 {
        let units = index as f64 * self.factor;
        let scale = 10f64.powi(self.exponent.saturating_abs());
        let value = if self.exponent < 0 {
            units / scale
        } else {
            units * scale
        };
        value + 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round_numbers() {
        assert_eq!(nice(0.0, 76.7, 5), [0.0, 20.0, 40.0, 60.0]);
        assert_eq!(nice(0.0, 1.0, 4), [0.0, 0.5, 1.0]);
        assert_eq!(nice(3.0, 9.0, 3), [4.0, 6.0, 8.0]);
        assert_eq!(nice(0.0, 1.0, 5), [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
        assert_eq!(nice(-0.35, 0.35, 5), [-0.2, 0.0, 0.2]);
        assert_eq!(nice(0.0, 2500.0, 3), [0.0, 1000.0, 2000.0]);
    }

    #[test]
    fn negative_ranges_straddle_zero() {
        assert_eq!(nice(-101.0, 101.0, 5), [-100.0, -50.0, 0.0, 50.0, 100.0]);
    }

    #[test]
    fn degenerate_input_gives_no_ticks() {
        assert!(nice(1.0, 1.0, 5).is_empty());
        assert!(nice(2.0, 1.0, 5).is_empty());
        assert!(nice(0.0, f64::NAN, 5).is_empty());
        assert!(nice(0.0, f64::INFINITY, 5).is_empty());
        assert!(nice(0.0, 1.0, 0).is_empty());
        assert!(nice(-f64::MAX, f64::MAX, 5).is_empty());
    }
}
