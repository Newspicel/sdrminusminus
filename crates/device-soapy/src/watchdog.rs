use std::time::{Duration, Instant};

use sdrmm_device::SILENT_STREAM_TIMEOUT;

const TIMEOUT_READS_BEFORE_PROBE: u32 = 10;
const PROBE_MIN_INTERVAL: Duration = Duration::from_secs(1);
const PROBE_FAILURES_BEFORE_GIVING_UP: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Watch {
    Wait,
    Probe,
    Silent,
}

#[derive(Debug)]
pub(crate) struct Watchdog {
    silence: Duration,
    timeouts: u32,
    probe_failures: u32,
    last_block: Instant,
    last_probe: Option<Instant>,
}

impl Watchdog {
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            silence: SILENT_STREAM_TIMEOUT,
            timeouts: 0,
            probe_failures: 0,
            last_block: now,
            last_probe: None,
        }
    }

    pub(crate) const fn silence(&self) -> Duration {
        self.silence
    }

    pub(crate) fn delivered(&mut self, now: Instant) {
        self.timeouts = 0;
        self.probe_failures = 0;
        self.last_block = now;
    }

    pub(crate) fn timed_out(&mut self, now: Instant) -> Watch {
        self.timeouts += 1;
        if now.duration_since(self.last_block) >= self.silence {
            return Watch::Silent;
        }
        if self.timeouts < TIMEOUT_READS_BEFORE_PROBE {
            return Watch::Wait;
        }
        if self
            .last_probe
            .is_some_and(|at| now.duration_since(at) < PROBE_MIN_INTERVAL)
        {
            return Watch::Wait;
        }
        self.last_probe = Some(now);
        Watch::Probe
    }

    pub(crate) fn present(&mut self) {
        self.timeouts = 0;
        self.probe_failures = 0;
    }

    pub(crate) fn probe_failed(&mut self) -> bool {
        self.probe_failures += 1;
        self.probe_failures >= PROBE_FAILURES_BEFORE_GIVING_UP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ: Duration = Duration::from_millis(100);

    fn timeouts(watchdog: &mut Watchdog, at: Instant, count: u32) -> Vec<Watch> {
        (1..=count)
            .map(|n| watchdog.timed_out(at + READ * n))
            .collect()
    }

    #[test]
    fn a_short_gap_only_waits() {
        let start = Instant::now();
        let mut watchdog = Watchdog::new(start);
        assert!(
            timeouts(&mut watchdog, start, TIMEOUT_READS_BEFORE_PROBE - 1)
                .iter()
                .all(|watch| *watch == Watch::Wait)
        );
    }

    #[test]
    fn a_longer_gap_probes_at_most_once_a_second() {
        let start = Instant::now();
        let mut watchdog = Watchdog::new(start);
        let watches = timeouts(&mut watchdog, start, 20);
        assert_eq!(
            watches
                .iter()
                .filter(|watch| **watch == Watch::Probe)
                .count(),
            2,
            "two seconds of timeouts is two probes: {watches:?}"
        );
    }

    #[test]
    fn a_stream_that_never_delivers_goes_silent_however_often_it_probes() {
        let start = Instant::now();
        let mut watchdog = Watchdog::new(start);
        let mut now = start;
        let mut probes = 0;
        loop {
            now += READ;
            match watchdog.timed_out(now) {
                Watch::Wait => {}
                Watch::Probe => {
                    probes += 1;
                    watchdog.present();
                }
                Watch::Silent => break,
            }
            assert!(
                now.duration_since(start) <= SILENT_STREAM_TIMEOUT,
                "an enumerated but silent stream must not wait forever"
            );
        }
        assert!(probes > 0, "the device was probed while it stayed quiet");
        assert_eq!(now.duration_since(start), SILENT_STREAM_TIMEOUT);
    }

    #[test]
    fn a_stream_that_keeps_delivering_never_goes_silent() {
        let start = Instant::now();
        let mut watchdog = Watchdog::new(start);
        let mut now = start;
        for _ in 0..100 {
            now += SILENT_STREAM_TIMEOUT - READ;
            assert_ne!(watchdog.timed_out(now), Watch::Silent);
            now += READ;
            watchdog.delivered(now);
        }
    }

    #[test]
    fn probe_failures_give_up_only_after_a_second_one() {
        let mut watchdog = Watchdog::new(Instant::now());
        assert!(!watchdog.probe_failed());
        assert!(watchdog.probe_failed());
    }

    #[test]
    fn a_probe_that_finds_the_device_clears_earlier_failures() {
        let mut watchdog = Watchdog::new(Instant::now());
        assert!(!watchdog.probe_failed());
        watchdog.present();
        assert!(
            !watchdog.probe_failed(),
            "the failure count was not cleared"
        );
    }
}
