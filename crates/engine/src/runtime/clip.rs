use std::sync::atomic::{AtomicU64, Ordering};

use num_complex::Complex;

const FULL_SCALE: f32 = 0.995;
const CLIPPING_FRACTION: u64 = 10_000;

#[derive(Debug, Default)]
pub(crate) struct ClipMeter {
    clipped: AtomicU64,
    samples: AtomicU64,
}

impl ClipMeter {
    pub(crate) fn measure(&self, samples: &[Complex<f32>]) {
        let clipped = samples
            .iter()
            .filter(|sample| sample.re.abs() >= FULL_SCALE || sample.im.abs() >= FULL_SCALE)
            .count() as u64;
        if clipped > 0 {
            self.clipped.fetch_add(clipped, Ordering::Relaxed);
        }
        self.samples
            .fetch_add(samples.len() as u64, Ordering::Relaxed);
    }

    pub(crate) fn take_clipping(&self) -> bool {
        let clipped = self.clipped.swap(0, Ordering::Relaxed);
        let samples = self.samples.swap(0, Ordering::Relaxed);
        clipped > 0 && clipped.saturating_mul(CLIPPING_FRACTION) >= samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet(len: usize) -> Vec<Complex<f32>> {
        vec![Complex::new(0.1, -0.2); len]
    }

    #[test]
    fn a_quiet_lane_is_not_clipping() {
        let meter = ClipMeter::default();
        meter.measure(&quiet(100_000));
        assert!(!meter.take_clipping());
    }

    #[test]
    fn samples_at_full_scale_on_either_rail_are_clipping() {
        let meter = ClipMeter::default();
        let mut block = quiet(1_000);
        block[10] = Complex::new(0.0, -0.9992);
        meter.measure(&block);
        assert!(meter.take_clipping());
    }

    #[test]
    fn a_single_spike_in_a_long_window_is_not_clipping() {
        let meter = ClipMeter::default();
        let mut block = quiet(100_000);
        block[0] = Complex::new(1.0, 0.0);
        meter.measure(&block);
        assert!(!meter.take_clipping());
    }

    #[test]
    fn each_window_is_judged_on_its_own() {
        let meter = ClipMeter::default();
        meter.measure(&[Complex::new(1.0, 1.0)]);
        assert!(meter.take_clipping());
        meter.measure(&quiet(10));
        assert!(!meter.take_clipping());
    }
}
