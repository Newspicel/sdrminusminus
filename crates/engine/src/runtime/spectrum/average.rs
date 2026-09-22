use sdrmm_dsp::PowerAverage;

use crate::spectrum::SpectrumFrame;

pub(in crate::runtime) struct FrameAverage {
    power: PowerAverage,
    tuning: Option<(f64, f32)>,
}

impl FrameAverage {
    pub(in crate::runtime) fn new(size: usize) -> Self {
        Self {
            power: PowerAverage::new(size),
            tuning: None,
        }
    }

    pub(in crate::runtime) fn reset(&mut self) {
        self.power.reset();
        self.tuning = None;
    }

    pub(in crate::runtime) fn push(
        &mut self,
        frame: SpectrumFrame,
        db: &[f32],
        wanted: u32,
        out: &mut [f32],
    ) -> Option<SpectrumFrame> {
        let tuning = (frame.center_hz, frame.span_hz);
        if self.tuning != Some(tuning) {
            self.power.reset();
            self.tuning = Some(tuning);
        }
        self.power.add(db);
        if self.power.count() < wanted.max(1) {
            return None;
        }
        self.power.take_db(out);
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_test_support::assert_no_alloc;

    use super::*;

    fn frame(center_hz: f64) -> SpectrumFrame {
        SpectrumFrame {
            timestamp: 0,
            center_hz,
            span_hz: 2_048_000.0,
        }
    }

    #[test]
    fn publishes_once_every_wanted_transforms() {
        let mut average = FrameAverage::new(2);
        let mut out = [0.0; 2];
        assert!(
            average
                .push(frame(1e8), &[-10.0, -20.0], 2, &mut out)
                .is_none()
        );
        assert!(
            average
                .push(frame(1e8), &[-10.0, -20.0], 2, &mut out)
                .is_some()
        );
        assert!((out[0] - -10.0).abs() < 1e-3 && (out[1] - -20.0).abs() < 1e-3);
    }

    #[test]
    fn a_retune_starts_the_average_over() {
        let mut average = FrameAverage::new(1);
        let mut out = [0.0; 1];
        assert!(average.push(frame(1e8), &[0.0], 2, &mut out).is_none());
        assert!(average.push(frame(2e8), &[-50.0], 2, &mut out).is_none());
        assert!(average.push(frame(2e8), &[-50.0], 2, &mut out).is_some());
        assert!((out[0] - -50.0).abs() < 1e-3, "{}", out[0]);
    }

    #[test]
    fn averaging_does_not_allocate() {
        let mut average = FrameAverage::new(4_096);
        let db = vec![-80.0; 4_096];
        let mut out = vec![0.0; 4_096];
        assert_no_alloc("frame average", || {
            for _ in 0..4 {
                average.push(frame(1e8), &db, 2, &mut out);
            }
        });
    }
}
