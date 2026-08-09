//! USB enumeration and exact serial-number selection.

use nusb::MaybeFuture;

use super::error::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UsbDeviceId {
    vid: u16,
    pid: u16,
    description: &'static str,
}

/// Every VID/PID libhackrf claims in normal (non-DFU) mode.
const USB_DEVICE_IDS: &[UsbDeviceId] = &[
    UsbDeviceId {
        vid: 0x1d50,
        pid: 0x604b,
        description: "HackRF Jawbreaker",
    },
    UsbDeviceId {
        vid: 0x1d50,
        pid: 0x6089,
        description: "HackRF One / HackRF Pro",
    },
    UsbDeviceId {
        vid: 0x1d50,
        pid: 0xcc15,
        description: "rad1o",
    },
];

/// What USB enumeration says about one attached radio, without claiming it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceDescriptor {
    /// USB vendor ID.
    pub(crate) vid: u16,
    /// USB product ID.
    pub(crate) pid: u16,
    /// Static description from the known HackRF USB ID table.
    pub(crate) description: &'static str,
    /// The full 128-bit serial, if the descriptor carries a parseable one.
    pub(crate) serial: Option<u128>,
    /// USB product string, when available.
    pub(crate) product_string: Option<String>,
    /// HackRF USB API version from `bcdDevice`.
    pub(crate) usb_api_version: u16,
}

impl DeviceDescriptor {
    fn from_nusb(info: &nusb::DeviceInfo) -> Option<Self> {
        let id = find_usb_device_id(info.vendor_id(), info.product_id())?;
        Some(Self {
            vid: id.vid,
            pid: id.pid,
            description: id.description,
            serial: info.serial_number().and_then(parse_serial),
            product_string: info.product_string().map(str::to_owned),
            usb_api_version: info.device_version(),
        })
    }
}

fn find_usb_device_id(vid: u16, pid: u16) -> Option<UsbDeviceId> {
    USB_DEVICE_IDS
        .iter()
        .copied()
        .find(|candidate| candidate.vid == vid && candidate.pid == pid)
}

/// Every HackRF currently attached.
pub(crate) fn list_devices() -> Result<Vec<DeviceDescriptor>> {
    Ok(nusb::list_devices()
        .wait()
        .map_err(|e| Error::usb("listing USB devices", e))?
        .filter_map(|info| DeviceDescriptor::from_nusb(&info))
        .collect())
}

/// The USB device for `serial`, or the first HackRF on the bus when it is `None`.
pub(crate) fn select_device(serial: Option<u128>) -> Result<nusb::DeviceInfo> {
    nusb::list_devices()
        .wait()
        .map_err(|e| Error::usb("listing USB devices", e))?
        .find(|info| matches_device(info, serial))
        .ok_or(Error::DeviceNotFound)
}

fn matches_device(info: &nusb::DeviceInfo, serial: Option<u128>) -> bool {
    find_usb_device_id(info.vendor_id(), info.product_id()).is_some()
        && serial.is_none_or(|wanted| info.serial_number().and_then(parse_serial) == Some(wanted))
}

/// Parse the 32 hexadecimal characters HackRF firmware puts in its serial descriptor. Anything
/// else is not a HackRF serial, and must not be silently coerced into one.
pub(crate) fn parse_serial(value: &str) -> Option<u128> {
    let value = value.trim();
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_round_trip_preserves_leading_zeroes() {
        let serial = 0x0011_2233_4455_6677_8899_aabb_ccdd_eeff;
        let text = format!("{serial:032x}");
        assert_eq!(text, "00112233445566778899aabbccddeeff");
        assert_eq!(parse_serial(&text), Some(serial));
        assert_eq!(parse_serial(&text.to_uppercase()), Some(serial));
    }

    #[test]
    fn serial_parser_requires_the_exact_firmware_shape() {
        assert_eq!(parse_serial("1234"), None);
        assert_eq!(parse_serial("00112233445566778899aabbccddeefg"), None);
        assert_eq!(
            parse_serial(" 00112233445566778899aabbccddeeff "),
            Some(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
        );
    }

    #[test]
    fn all_libhackrf_ids_are_known() {
        for pid in [0x604b, 0x6089, 0xcc15] {
            assert!(find_usb_device_id(0x1d50, pid).is_some());
        }
        assert!(find_usb_device_id(0x0bda, 0x2838).is_none());
    }
}
