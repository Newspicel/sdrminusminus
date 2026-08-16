use std::hash::{DefaultHasher, Hash, Hasher};

use nusb::MaybeFuture;

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
}
