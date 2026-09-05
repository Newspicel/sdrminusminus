use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SpectrumSnapshot {
    pub seq: u32,
    pub timestamp: u64,
    pub center_hz: f64,
    pub span_hz: f32,
    pub lo_hz: f64,
    pub db: Arc<[f32]>,
}

const LO_GUARD_BINS: usize = 2;

impl SpectrumSnapshot {
    #[must_use]
    pub fn lo_guard(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let n = self.db.len();
        if n == 0 || !self.span_hz.is_finite() || self.span_hz <= 0.0 || !self.lo_hz.is_finite() {
            return None;
        }
        let offset = (self.lo_hz - self.center_hz) / f64::from(self.span_hz);
        let bin = (offset + 0.5) * n as f64;
        if !bin.is_finite() {
            return None;
        }
        let lo = (bin - LO_GUARD_BINS as f64).ceil();
        let hi = (bin + LO_GUARD_BINS as f64).floor();
        if hi < 0.0 || lo > (n - 1) as f64 {
            return None;
        }
        Some((lo.max(0.0) as usize)..=(hi.max(0.0) as usize).min(n - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_at(center_hz: f64, lo_hz: f64, bins: usize) -> SpectrumSnapshot {
        SpectrumSnapshot {
            seq: 0,
            timestamp: 0,
            center_hz,
            span_hz: 1_024_000.0,
            lo_hz,
            db: Arc::from(vec![-100.0f32; bins].as_slice()),
        }
    }

    #[test]
    fn the_lo_guard_covers_the_artifact_wherever_the_lo_was_parked() {
        let centred = snapshot_at(100e6, 100e6, 1_024);
        assert_eq!(centred.lo_guard(), Some(510..=514));

        let displaced = snapshot_at(100e6, 100e6 - 256_000.0, 1_024);
        assert_eq!(displaced.lo_guard(), Some(254..=258));
    }

    #[test]
    fn an_lo_outside_the_span_guards_nothing() {
        assert_eq!(snapshot_at(100e6, 90e6, 1_024).lo_guard(), None);
        assert_eq!(snapshot_at(100e6, f64::NAN, 1_024).lo_guard(), None);
        assert_eq!(snapshot_at(100e6, 100e6, 0).lo_guard(), None);
    }
}
