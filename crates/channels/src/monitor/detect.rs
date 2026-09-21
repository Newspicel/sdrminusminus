use num_complex::Complex;

use crate::ident::detect::{Band, Detector as SignalDetector, Search};

pub(super) struct Detector {
    detector: SignalDetector,
    rate: f64,
}

impl Detector {
    pub(super) fn new(rate: f64) -> Self {
        let size = ((rate / 250.0) as usize)
            .next_power_of_two()
            .clamp(4096, 262_144);
        Self {
            detector: SignalDetector::with_size(size, 33),
            rate,
        }
    }

    pub(super) fn measure(&mut self, iq: &[Complex<f32>]) -> Vec<Band> {
        self.detector
            .measure(
                iq,
                self.rate,
                &Search {
                    half_span_hz: self.rate / 2.0,
                    threshold_db: 12.0,
                    gap_hz: 3000.0,
                    dominated: false,
                    artifact_hz: None,
                },
            )
            .bands
    }
}
