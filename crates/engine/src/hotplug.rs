use std::time::{Duration, Instant};

pub(crate) const FULL_PROBE_INTERVAL: Duration = Duration::from_secs(60);

/// Decides when a hotplug tick is allowed to enumerate radios.
///
/// Enumeration runs vendor SoapySDR modules that open USB devices, so it is kept for the moments
/// that can reveal something new: a change in the attached USB devices, or the periodic sweep that
/// picks up network radios the USB view cannot see.
#[derive(Debug, Default)]
pub(crate) struct ProbeGate {
    usb: Option<u64>,
    last: Option<Instant>,
}

impl ProbeGate {
    pub(crate) fn should_probe(&mut self, now: Instant, usb: Option<u64>) -> bool {
        let due = match self.last {
            None => true,
            Some(last) => usb != self.usb || now.duration_since(last) >= FULL_PROBE_INTERVAL,
        };
        if due {
            self.usb = usb;
            self.last = Some(now);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tick_always_probes() {
        let mut gate = ProbeGate::default();
        assert!(gate.should_probe(Instant::now(), Some(1)));
    }

    #[test]
    fn an_unchanged_bus_is_left_alone_between_sweeps() {
        let start = Instant::now();
        let mut gate = ProbeGate::default();
        assert!(gate.should_probe(start, Some(1)));
        assert!(!gate.should_probe(start + Duration::from_secs(5), Some(1)));
        assert!(!gate.should_probe(
            start + FULL_PROBE_INTERVAL - Duration::from_secs(1),
            Some(1)
        ));
    }

    #[test]
    fn a_plugged_or_unplugged_radio_probes_at_once() {
        let start = Instant::now();
        let mut gate = ProbeGate::default();
        assert!(gate.should_probe(start, Some(1)));
        assert!(gate.should_probe(start + Duration::from_secs(5), Some(2)));
        assert!(gate.should_probe(start + Duration::from_secs(10), Some(1)));
    }

    #[test]
    fn network_radios_are_still_found_by_the_periodic_sweep() {
        let start = Instant::now();
        let mut gate = ProbeGate::default();
        assert!(gate.should_probe(start, Some(1)));
        assert!(gate.should_probe(start + FULL_PROBE_INTERVAL, Some(1)));
        assert!(!gate.should_probe(start + FULL_PROBE_INTERVAL, Some(1)));
    }

    #[test]
    fn a_platform_without_a_usb_view_probes_every_sweep() {
        let start = Instant::now();
        let mut gate = ProbeGate::default();
        assert!(gate.should_probe(start, None));
        assert!(!gate.should_probe(start + Duration::from_secs(5), None));
        assert!(gate.should_probe(start + FULL_PROBE_INTERVAL, None));
    }
}
