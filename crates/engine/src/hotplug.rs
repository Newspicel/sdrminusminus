use std::time::Duration;

use sdrmm_device::usb::BusWatch;

/// Decides when a hotplug tick is allowed to enumerate radios.
///
/// Enumeration runs vendor modules that open USB devices, so it is kept for the only moment that
/// can reveal something new without anyone asking: a change in the attached USB devices. Radios
/// that answer over the network are found when the device list is actually requested.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Probe {
    /// Nothing is known yet, so this one only establishes what is attached.
    First,
    /// A radio was plugged or unplugged: whoever is looking at the device list wants to know,
    /// whether or not the cheap search can already name what changed.
    BusChanged,
}

#[derive(Debug, Default)]
pub(crate) struct ProbeGate {
    usb: Option<u64>,
    probed: bool,
}

impl ProbeGate {
    pub(crate) fn should_probe(&mut self, usb: Option<u64>, woken: bool) -> Option<Probe> {
        let reason = match self.probed {
            false => Some(Probe::First),
            true if woken || usb != self.usb => Some(Probe::BusChanged),
            true => None,
        };
        if reason.is_some() {
            self.usb = usb;
            self.probed = true;
        }
        reason
    }
}

/// Paces the hotplug thread: it wakes on the interval for the housekeeping every tick does, and
/// at once when a radio is plugged or unplugged.
pub(crate) enum Pace {
    Watched(BusWatch),
    Polled,
}

impl Pace {
    pub(crate) fn start() -> Self {
        BusWatch::start().map_or(Self::Polled, Self::Watched)
    }

    /// Waits for the next tick and reports whether the bus is why it came early.
    pub(crate) fn wait(&self, interval: Duration) -> bool {
        match self {
            Self::Watched(watch) => watch.wait(interval).is_some(),
            Self::Polled => {
                std::thread::sleep(interval);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_tick_always_probes() {
        let mut gate = ProbeGate::default();
        assert_eq!(gate.should_probe(Some(1), false), Some(Probe::First));
    }

    #[test]
    fn an_unchanged_bus_is_left_alone() {
        let mut gate = ProbeGate::default();
        assert_eq!(gate.should_probe(Some(1), false), Some(Probe::First));
        assert_eq!(gate.should_probe(Some(1), false), None);
        assert_eq!(gate.should_probe(Some(1), false), None);
    }

    #[test]
    fn a_plugged_or_unplugged_radio_probes_at_once() {
        let mut gate = ProbeGate::default();
        assert_eq!(gate.should_probe(Some(1), false), Some(Probe::First));
        assert_eq!(gate.should_probe(Some(2), false), Some(Probe::BusChanged));
        assert_eq!(gate.should_probe(Some(1), false), Some(Probe::BusChanged));
    }

    #[test]
    fn a_bus_event_probes_even_where_the_fingerprint_cannot_tell() {
        let mut gate = ProbeGate::default();
        assert_eq!(gate.should_probe(None, false), Some(Probe::First));
        assert_eq!(gate.should_probe(None, false), None);
        assert_eq!(gate.should_probe(None, true), Some(Probe::BusChanged));
    }

    #[test]
    fn a_polled_pace_sleeps_out_its_interval() {
        let start = std::time::Instant::now();
        assert!(!Pace::Polled.wait(Duration::from_millis(50)));
        assert!(start.elapsed() >= Duration::from_millis(50));
    }
}
