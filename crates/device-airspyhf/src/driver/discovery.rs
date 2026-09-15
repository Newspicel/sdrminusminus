use nusb::MaybeFuture;

use super::error::{Error, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct UsbDeviceId {
    vid: u16,
    pid: u16,
    description: &'static str,
}

const USB_DEVICE_IDS: &[UsbDeviceId] = &[UsbDeviceId {
    vid: 0x03eb,
    pid: 0x800c,
    description: "Airspy HF+",
}];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeviceDescriptor {
    pub(crate) description: &'static str,
    pub(crate) serial: Option<u64>,
    pub(crate) product_string: Option<String>,
    pub(crate) bus: String,
    pub(crate) address: u8,
}

impl DeviceDescriptor {
    fn from_nusb(info: &nusb::DeviceInfo) -> Option<Self> {
        let id = find_usb_device_id(info.vendor_id(), info.product_id())?;
        Some(Self {
            description: id.description,
            serial: info.serial_number().and_then(parse_serial),
            product_string: info.product_string().map(str::to_owned),
            bus: info.bus_id().to_string(),
            address: info.device_address(),
        })
    }
}

fn find_usb_device_id(vid: u16, pid: u16) -> Option<UsbDeviceId> {
    USB_DEVICE_IDS
        .iter()
        .copied()
        .find(|candidate| candidate.vid == vid && candidate.pid == pid)
}

pub(crate) fn list_devices() -> Result<Vec<DeviceDescriptor>> {
    Ok(nusb::list_devices()
        .wait()
        .map_err(|e| Error::usb("listing USB devices", e))?
        .filter_map(|info| DeviceDescriptor::from_nusb(&info))
        .collect())
}

pub(crate) enum Select {
    Serial(u64),
    Location { bus: String, address: u8 },
}

pub(crate) fn select_device(select: &Select) -> Result<nusb::DeviceInfo> {
    nusb::list_devices()
        .wait()
        .map_err(|e| Error::usb("listing USB devices", e))?
        .find(|info| matches_device(info, select))
        .ok_or(Error::DeviceNotFound)
}

fn matches_device(info: &nusb::DeviceInfo, select: &Select) -> bool {
    if find_usb_device_id(info.vendor_id(), info.product_id()).is_none() {
        return false;
    }
    match select {
        Select::Serial(wanted) => info.serial_number().and_then(parse_serial) == Some(*wanted),
        Select::Location { bus, address } => {
            info.bus_id() == bus && info.device_address() == *address
        }
    }
}

/// The descriptor carries the same 16 hex digits the part-id reply spells out.
pub(crate) fn parse_serial(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.len() != 16 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(value, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serial_round_trip_preserves_leading_zeroes() {
        let serial = 0x0044_0000_2e19_a5b3;
        let text = format!("{serial:016x}");
        assert_eq!(text, "004400002e19a5b3");
        assert_eq!(parse_serial(&text), Some(serial));
        assert_eq!(parse_serial(&text.to_uppercase()), Some(serial));
    }

    #[test]
    fn serial_parser_requires_the_exact_descriptor_shape() {
        assert_eq!(parse_serial("1234"), None);
        assert_eq!(parse_serial("004400002e19a5bg"), None);
        assert_eq!(
            parse_serial(" 004400002e19a5b3 "),
            Some(0x0044_0000_2e19_a5b3)
        );
    }

    #[test]
    fn only_the_hf_plus_usb_id_is_claimed() {
        assert!(find_usb_device_id(0x03eb, 0x800c).is_some());
        assert!(
            find_usb_device_id(0x1d50, 0x60a1).is_none(),
            "that is an Airspy R2 or Mini, which has its own driver"
        );
        assert!(
            find_usb_device_id(0x1d50, 0x6089).is_none(),
            "that is a HackRF"
        );
        assert!(find_usb_device_id(0x0bda, 0x2838).is_none());
    }
}
