use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

use futures::StreamExt;
use nusb::{MaybeFuture, hotplug::HotplugEvent};

/// How long the bus is given to settle once a device announces itself, so that a radio whose
/// interfaces appear one after another is enumerated once and after it can be opened.
const SETTLE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusChange {
    Arrived,
    Departed,
}

/// Reports plugged and unplugged USB devices as the OS sees them.
///
/// The alternative is listing the bus on a timer, which trades latency for the poll interval and
/// reports a plug only as far as the next tick. Nothing here opens a device.
pub struct BusWatch {
    changes: Receiver<BusChange>,
}

impl BusWatch {
    /// Starts watching, or `None` on a platform that cannot: the caller then falls back to
    /// [`fingerprint`].
    #[must_use]
    pub fn start() -> Option<Self> {
        let mut watch = match nusb::watch_devices() {
            Ok(watch) => watch,
            Err(error) => {
                tracing::debug!("usb hotplug notifications unavailable: {error}");
                return None;
            }
        };
        let (tx, changes) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("sdrmm-usb-watch".to_string())
            .spawn(move || {
                while let Some(event) = futures::executor::block_on(watch.next()) {
                    let change = match event {
                        HotplugEvent::Connected(_) => BusChange::Arrived,
                        HotplugEvent::Disconnected(_) => BusChange::Departed,
                    };
                    if tx.send(change).is_err() {
                        return;
                    }
                }
            });
        match spawned {
            Ok(_) => Some(Self { changes }),
            Err(error) => {
                tracing::warn!("cannot watch the usb bus: {error}");
                None
            }
        }
    }

    /// Waits up to `timeout` for the bus to change, then lets it settle and swallows the rest of
    /// the burst so that one plugged radio wakes its caller once.
    pub fn wait(&self, timeout: Duration) -> Option<BusChange> {
        let first = match self.changes.recv_timeout(timeout) {
            Ok(change) => change,
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(timeout);
                return None;
            }
        };
        std::thread::sleep(SETTLE);
        while self.changes.try_recv().is_ok() {}
        Some(first)
    }
}

/// Fingerprints the attached USB devices, or `None` when the platform cannot report them.
///
/// Reading the bus this way opens no device, so it is safe to call while a radio is streaming.
#[must_use]
pub fn fingerprint() -> Option<u64> {
    let devices = match nusb::list_devices().wait() {
        Ok(devices) => devices,
        Err(error) => {
            tracing::debug!("usb enumeration unavailable: {error}");
            return None;
        }
    };
    let mut seen: Vec<(String, u8, u16, u16)> = devices
        .map(|device| {
            (
                device.bus_id().to_string(),
                device.device_address(),
                device.vendor_id(),
                device.product_id(),
            )
        })
        .collect();
    seen.sort_unstable();
    Some(hash(&seen))
}

fn hash(seen: &[(String, u8, u16, u16)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    seen.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(bus: &str, address: u8, vendor: u16, product: u16) -> (String, u8, u16, u16) {
        (bus.to_string(), address, vendor, product)
    }

    #[test]
    fn the_same_bus_hashes_the_same_way() {
        let bus = vec![
            device("0", 4, 0x0bda, 0x2838),
            device("0", 5, 0x1d50, 0x6089),
        ];
        assert_eq!(hash(&bus), hash(&bus.clone()));
    }

    #[test]
    fn a_plugged_radio_changes_the_fingerprint() {
        let before = vec![device("0", 4, 0x0bda, 0x2838)];
        let mut after = before.clone();
        after.push(device("0", 5, 0x1d50, 0x6089));
        assert_ne!(hash(&before), hash(&after));
    }

    #[test]
    fn a_radio_moved_to_another_port_changes_the_fingerprint() {
        let before = vec![device("0", 4, 0x0bda, 0x2838)];
        let after = vec![device("0", 7, 0x0bda, 0x2838)];
        assert_ne!(hash(&before), hash(&after));
    }

    #[test]
    fn reading_the_bus_reports_something_or_nothing_without_panicking() {
        let first = fingerprint();
        assert_eq!(first, fingerprint());
    }

    #[test]
    fn a_quiet_bus_wakes_nobody_and_still_returns_on_time() {
        let Some(watch) = BusWatch::start() else {
            return;
        };
        let start = std::time::Instant::now();
        assert_eq!(watch.wait(Duration::from_millis(50)), None);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "waiting outlived its timeout by {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_watch_whose_thread_is_gone_still_paces_its_caller() {
        let (tx, changes) = mpsc::channel();
        drop(tx);
        let watch = BusWatch { changes };
        let start = std::time::Instant::now();
        assert_eq!(watch.wait(Duration::from_millis(50)), None);
        assert!(
            start.elapsed() >= Duration::from_millis(50),
            "a dead watch must not spin its caller"
        );
    }
}
