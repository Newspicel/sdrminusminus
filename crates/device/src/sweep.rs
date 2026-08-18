use crate::{DeviceError, Sample};

/// A stretch of spectrum a firmware sweep walks across.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepBand {
    pub start_hz: f64,
    pub stop_hz: f64,
}

/// What the caller wants swept, in the terms every radio can be asked in. A driver translates it
/// into whatever its firmware takes and reports back what it could not honour.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepPlan {
    pub bands: Vec<SweepBand>,
    pub sample_rate_hz: f64,
}

impl SweepPlan {
    #[must_use]
    pub fn new(bands: Vec<SweepBand>, sample_rate_hz: f64) -> Self {
        Self {
            bands,
            sample_rate_hz,
        }
    }

    pub fn check(&self) -> Result<(), DeviceError> {
        if self.bands.is_empty() {
            return Err(DeviceError::Unsupported(
                "a sweep needs at least one band".to_string(),
            ));
        }
        if !self.sample_rate_hz.is_finite() || self.sample_rate_hz <= 0.0 {
            return Err(DeviceError::Unsupported(format!(
                "a sweep needs a positive sample rate, got {}",
                self.sample_rate_hz
            )));
        }
        for band in &self.bands {
            if !band.start_hz.is_finite() || !band.stop_hz.is_finite() {
                return Err(DeviceError::Unsupported(
                    "sweep band bounds must be finite".to_string(),
                ));
            }
            if band.stop_hz <= band.start_hz {
                return Err(DeviceError::Unsupported(format!(
                    "sweep band {} Hz–{} Hz ends at or before it starts",
                    band.start_hz, band.stop_hz
                )));
            }
        }
        Ok(())
    }
}

type SweepPushFn = Box<dyn FnMut(f64, &[Sample]) + Send>;
type SweepFatalFn = Box<dyn FnOnce(DeviceError) + Send>;

/// Where a firmware sweep delivers its blocks. Unlike a receive sink each block carries the
/// frequency it was tuned to, because a sweep never stays at one.
pub struct SweepSink {
    push_fn: SweepPushFn,
    fatal_fn: Option<SweepFatalFn>,
}

impl SweepSink {
    #[must_use]
    pub fn new(push_fn: impl FnMut(f64, &[Sample]) + Send + 'static) -> Self {
        Self {
            push_fn: Box::new(push_fn),
            fatal_fn: None,
        }
    }

    #[must_use]
    pub fn with_fatal_handler(
        push_fn: impl FnMut(f64, &[Sample]) + Send + 'static,
        fatal_fn: impl FnOnce(DeviceError) + Send + 'static,
    ) -> Self {
        Self {
            push_fn: Box::new(push_fn),
            fatal_fn: Some(Box::new(fatal_fn)),
        }
    }

    pub fn push(&mut self, center_hz: f64, samples: &[Sample]) {
        (self.push_fn)(center_hz, samples);
    }

    pub fn fail(&mut self, err: DeviceError) {
        if let Some(fatal_fn) = self.fatal_fn.take() {
            fatal_fn(err);
        }
    }
}

impl std::fmt::Debug for SweepSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SweepSink")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(start_hz: f64, stop_hz: f64) -> SweepBand {
        SweepBand { start_hz, stop_hz }
    }

    #[test]
    fn a_plan_no_radio_could_run_is_refused() {
        for bad in [
            SweepPlan::new(Vec::new(), 20e6),
            SweepPlan::new(vec![band(88e6, 108e6)], 0.0),
            SweepPlan::new(vec![band(88e6, 108e6)], f64::NAN),
            SweepPlan::new(vec![band(108e6, 88e6)], 20e6),
            SweepPlan::new(vec![band(88e6, 88e6)], 20e6),
            SweepPlan::new(vec![band(f64::NAN, 108e6)], 20e6),
        ] {
            assert!(bad.check().is_err(), "accepted {bad:?}");
        }
        assert!(
            SweepPlan::new(vec![band(88e6, 108e6)], 20e6)
                .check()
                .is_ok()
        );
    }

    #[test]
    fn a_sink_carries_the_frequency_of_every_block() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let mut sink = SweepSink::new(move |center_hz, samples: &[Sample]| {
            record
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((center_hz, samples.len()));
        });
        sink.push(88e6, &[Sample::new(0.0, 0.0); 4]);
        sink.push(108e6, &[Sample::new(0.0, 0.0); 2]);
        let seen = seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(*seen, vec![(88e6, 4), (108e6, 2)]);
    }
}
