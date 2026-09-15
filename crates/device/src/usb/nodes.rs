use std::{
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
};

use nusb::MaybeFuture;

use super::RadioNode;

struct UsbIdentity {
    vendor: u16,
    product: Option<u16>,
    name: &'static str,
}

/// The radios sdr-- can drive over USB, by the identity they announce on the bus.
///
/// A product of `None` claims the whole vendor, which is only correct for a vendor that makes
/// nothing else. The entries match the identities the vendors' own udev rules match.
const RADIOS: &[UsbIdentity] = &[
    UsbIdentity {
        vendor: 0x0bda,
        product: Some(0x2832),
        name: "RTL-SDR",
    },
    UsbIdentity {
        vendor: 0x0bda,
        product: Some(0x2838),
        name: "RTL-SDR",
    },
    UsbIdentity {
        vendor: 0x1d50,
        product: Some(0x604b),
        name: "HackRF Jawbreaker",
    },
    UsbIdentity {
        vendor: 0x1d50,
        product: Some(0x6089),
        name: "HackRF One",
    },
    UsbIdentity {
        vendor: 0x1d50,
        product: Some(0xcc15),
        name: "rad1o",
    },
    UsbIdentity {
        vendor: 0x1d50,
        product: Some(0x60a1),
        name: "Airspy",
    },
    UsbIdentity {
        vendor: 0x1d50,
        product: Some(0x6108),
        name: "LimeSDR Mini",
    },
    UsbIdentity {
        vendor: 0x0403,
        product: Some(0x601f),
        name: "LimeSDR-USB",
    },
    UsbIdentity {
        vendor: 0x03eb,
        product: Some(0x800c),
        name: "Airspy HF+",
    },
    UsbIdentity {
        vendor: 0x2cf0,
        product: Some(0x5246),
        name: "bladeRF x40/x115",
    },
    UsbIdentity {
        vendor: 0x2cf0,
        product: Some(0x5250),
        name: "bladeRF 2.0 micro",
    },
    UsbIdentity {
        vendor: 0x0456,
        product: Some(0xb673),
        name: "AD936x board",
    },
    UsbIdentity {
        vendor: 0x1df7,
        product: None,
        name: "SDRplay RSP",
    },
];

fn radio_name(vendor: u16, product: u16) -> Option<&'static str> {
    RADIOS
        .iter()
        .find(|radio| {
            radio.vendor == vendor && radio.product.is_none_or(|wanted| wanted == product)
        })
        .map(|radio| radio.name)
}

/// The device nodes of the attached USB radios, each with what this process may do with it.
///
/// Listing the bus opens no device, and the kernel answers the access question itself, so an ACL
/// from a `uaccess` udev rule counts as much as the file mode does.
#[must_use]
pub fn radio_nodes() -> Vec<RadioNode> {
    let devices = match nusb::list_devices().wait() {
        Ok(devices) => devices,
        Err(error) => {
            tracing::debug!("usb enumeration unavailable: {error}");
            return Vec::new();
        }
    };
    let mut nodes: Vec<RadioNode> = devices
        .filter_map(|device| {
            let name = radio_name(device.vendor_id(), device.product_id())?;
            let path = PathBuf::from(format!(
                "/dev/bus/usb/{:03}/{:03}",
                device.busnum(),
                device.device_address()
            ));
            let node = std::fs::metadata(&path).ok();
            Some(RadioNode {
                name,
                present: node.is_some(),
                uid: node.as_ref().map_or(0, MetadataExt::uid),
                gid: node.as_ref().map_or(0, MetadataExt::gid),
                mode: node.as_ref().map_or(0, |node| node.mode() & 0o777),
                openable: node.is_some() && openable(&path),
                path,
            })
        })
        .collect();
    nodes.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    nodes
}

fn openable(path: &Path) -> bool {
    let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: the string is NUL-terminated and outlives the call, which only reads it.
    unsafe {
        libc::faccessat(
            libc::AT_FDCWD,
            path.as_ptr(),
            libc::R_OK | libc::W_OK,
            libc::AT_EACCESS,
        ) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_radio_is_named_by_its_usb_identity() {
        assert_eq!(radio_name(0x2cf0, 0x5250), Some("bladeRF 2.0 micro"));
        assert_eq!(radio_name(0x0bda, 0x2838), Some("RTL-SDR"));
        assert_eq!(radio_name(0x1d50, 0x6089), Some("HackRF One"));
    }

    #[test]
    fn a_vendor_that_makes_only_radios_claims_every_product() {
        assert_eq!(radio_name(0x1df7, 0x3020), Some("SDRplay RSP"));
        assert_eq!(radio_name(0x1df7, 0x2500), Some("SDRplay RSP"));
    }

    #[test]
    fn anything_else_on_the_bus_is_not_a_radio() {
        assert_eq!(radio_name(0x0bda, 0x2839), None);
        assert_eq!(radio_name(0x1d50, 0x0001), None);
        assert_eq!(radio_name(0x1234, 0x5678), None);
    }

    #[test]
    fn a_node_nobody_has_cannot_be_opened() {
        assert!(!openable(Path::new("/dev/bus/usb/255/255")));
    }

    #[test]
    fn listing_the_bus_reports_something_or_nothing_without_panicking() {
        for node in radio_nodes() {
            assert!(node.path.starts_with("/dev/bus/usb"));
        }
    }
}
