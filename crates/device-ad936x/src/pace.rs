use std::time::Instant;

const SLACK_BUFFERS: f64 = 4.0;
const CLOCK_TOLERANCE: f64 = 100e-6;

#[derive(Debug)]
pub(crate) struct Pace {
    rate: f64,
    slack: f64,
    origin: Option<Instant>,
    received: f64,
    lost: f64,
}

impl Pace {
    pub(crate) fn new(rate: Option<f64>, buffer_frames: usize) -> Self {
        Self {
            rate: rate
                .filter(|rate| rate.is_finite() && *rate > 0.0)
                .unwrap_or(0.0),
            slack: buffer_frames as f64 * SLACK_BUFFERS,
            origin: None,
            received: 0.0,
            lost: 0.0,
        }
    }

    pub(crate) fn arrived(&mut self, frames: usize, now: Instant) -> u64 {
        let Some(origin) = self.origin.filter(|_| self.rate > 0.0) else {
            self.origin = Some(now);
            return 0;
        };
        self.received += frames as f64;
        let expected = now.duration_since(origin).as_secs_f64() * self.rate;
        let missing = expected - self.received - self.lost;
        if missing <= self.slack + expected * CLOCK_TOLERANCE {
            return 0;
        }
        self.lost += missing;
        missing.round() as u64
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const BUFFER: usize = 1_000;

    fn after(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_radio_keeping_pace_loses_nothing() {
        let start = Instant::now();
        let mut pace = Pace::new(Some(100_000.0), BUFFER);
        assert_eq!(pace.arrived(BUFFER, start), 0);
        for step in 1..=1_000 {
            assert_eq!(pace.arrived(BUFFER, after(start, step * 10)), 0);
        }
    }

    #[test]
    fn a_late_buffer_within_the_slack_is_not_a_loss() {
        let start = Instant::now();
        let mut pace = Pace::new(Some(100_000.0), BUFFER);
        pace.arrived(BUFFER, start);
        assert_eq!(pace.arrived(BUFFER, after(start, 40)), 0);
        assert_eq!(pace.arrived(BUFFER * 3, after(start, 45)), 0);
    }

    #[test]
    fn a_radio_delivering_a_quarter_of_its_rate_reports_the_rest_once() {
        let start = Instant::now();
        let mut pace = Pace::new(Some(100_000.0), BUFFER);
        pace.arrived(BUFFER, start);
        let mut lost = 0;
        for step in 1..=100 {
            lost += pace.arrived(BUFFER, after(start, step * 40));
        }
        let expected = 400_000.0 - 100_000.0;
        assert!(
            (lost as f64 - expected).abs() < BUFFER as f64 * SLACK_BUFFERS + 1.0,
            "{lost}"
        );
    }

    #[test]
    fn a_stream_without_a_rate_is_not_judged() {
        let start = Instant::now();
        let mut pace = Pace::new(None, BUFFER);
        pace.arrived(1, start);
        assert_eq!(pace.arrived(1, after(start, 10_000)), 0);
    }
}
