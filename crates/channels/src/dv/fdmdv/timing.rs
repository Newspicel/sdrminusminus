use super::{
    CARRIERS, CODEC2_PI, FrameLength, OVERSAMPLING, SYMBOL_SAMPLES, Sample, downconvert::Filtered,
    magnitude,
};

const WINDOW_SYMBOLS: usize = 5;
const WINDOW: usize = WINDOW_SYMBOLS * OVERSAMPLING;
const CENTRE: f32 = (WINDOW_SYMBOLS / 2 * OVERSAMPLING) as f32;
const EMPIRICAL_OFFSET: f32 = (OVERSAMPLING / 4) as f32;

pub(super) struct TimingEstimator {
    history: [[Sample; WINDOW]; CARRIERS],
    rotation: Sample,
}

impl TimingEstimator {
    pub(super) fn new() -> Self {
        let radians = (2.0 * CODEC2_PI / OVERSAMPLING as f64) as f32;
        Self {
            history: [[Sample::ZERO; WINDOW]; CARRIERS],
            rotation: Sample::new(radians.cos(), radians.sin()),
        }
    }

    pub(super) fn estimate(
        &mut self,
        filtered: &Filtered,
        length: FrameLength,
        symbols: &mut [Sample; CARRIERS],
    ) -> f32 {
        let fresh = length.oversampled();
        for (history, filtered) in self.history.iter_mut().zip(filtered) {
            history.copy_within(fresh.., 0);
            history[WINDOW - fresh..].copy_from_slice(&filtered[..fresh]);
        }
        let cycles = self.symbol_phase() / (2.0 * CODEC2_PI);
        let cycles = cycles as f32;
        self.resample(interpolation_point(cycles), symbols);
        cycles * SYMBOL_SAMPLES as f32
    }

    fn symbol_phase(&self) -> f64 {
        let mut phase = Sample::ONE;
        let mut tone = Sample::ZERO;
        for index in 0..WINDOW {
            let envelope = self
                .history
                .iter()
                .fold(0.0, |sum, carrier| sum + magnitude(carrier[index]));
            tone += phase.scale(envelope);
            phase *= self.rotation;
        }
        f64::from(tone.im.atan2(tone.re))
    }

    fn resample(&self, point: f32, symbols: &mut [Sample; CARRIERS]) {
        let low = (point.floor() as usize).clamp(1, WINDOW);
        let high = (point.ceil() as usize).clamp(1, WINDOW);
        let fraction = point - low as f32;
        let complement = (1.0 - f64::from(fraction)) as f32;
        for (symbol, history) in symbols.iter_mut().zip(&self.history) {
            *symbol = history[low - 1].scale(complement) + history[high - 1].scale(fraction);
        }
    }
}

fn interpolation_point(cycles: f32) -> f32 {
    let steps = OVERSAMPLING as f32;
    let mut point = cycles * steps + EMPIRICAL_OFFSET;
    if point > steps {
        point -= steps;
    }
    if point < -steps {
        point += steps;
    }
    point + CENTRE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interpolation_point_stays_inside_the_window() {
        for cycles in [-0.5, -0.25, 0.0, 0.25, 0.4999] {
            let point = interpolation_point(cycles);
            assert!((1.0..=WINDOW as f32).contains(&point), "{cycles}: {point}");
        }
        assert_eq!(interpolation_point(0.0), CENTRE + 1.0);
    }

    #[test]
    fn an_envelope_peak_on_each_symbol_is_found() {
        let mut estimator = TimingEstimator::new();
        let mut filtered: Filtered = [[Sample::ZERO; OVERSAMPLING + 1]; CARRIERS];
        for carrier in &mut filtered {
            carrier[..4].copy_from_slice(&[0.2, 1.0, 0.2, 0.0].map(Sample::from));
        }
        let mut symbols = [Sample::ZERO; CARRIERS];
        let mut timing = 0.0;
        for _ in 0..WINDOW_SYMBOLS {
            timing = estimator.estimate(&filtered, FrameLength::Nominal, &mut symbols);
        }
        assert!((timing - 40.0).abs() < 1.0, "{timing}");
        assert!(symbols.iter().all(|symbol| (symbol.re - 1.0).abs() < 1e-5));
    }

    #[test]
    fn nan_timing_does_not_index_out_of_the_window() {
        let estimator = TimingEstimator::new();
        let mut symbols = [Sample::ONE; CARRIERS];
        estimator.resample(f32::NAN, &mut symbols);
        estimator.resample(1e9, &mut symbols);
    }
}
