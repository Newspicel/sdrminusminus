use std::time::Duration;

use nusb::MaybeFuture;
use sdrmm_device::{DeviceError, net::Endpoint};

use crate::iio::{DEFAULT_PORT, INTERFACE_NAME, iio_interface};

/// The USB identity every AD936x board running the reference firmware carries, an AntSDR and
/// a PlutoSDR alike.
/// It is only a fallback: a radio is recognised by its IIO interface wherever the operating
/// system reports interface names, so a board with its own identity is still found.
const VENDOR_ID: u16 = 0x0456;
const PRODUCT_ID: u16 = 0xb673;

pub(crate) const USB_PREFIX: &str = "usb-";

/// Addresses these radios ship on. A search tries them so that a board straight out of the box
/// appears without the operator having to type anything; the names resolve through whatever
/// multicast DNS the host already runs.
const WELL_KNOWN: [&str; 4] = ["ant.local", "192.168.1.10", "pluto.local", "192.168.2.1"];

/// How long a search waits on an address nobody said was there.
const REACH_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsbRadio {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) serial: Option<String>,
}

/// Whether this USB device serves iiod. The interface name is the reliable answer and the
/// identity is the fallback for the platforms that do not report interface names.
fn serves_iio(info: &nusb::DeviceInfo) -> bool {
    let mut interfaces = info.interfaces().peekable();
    if interfaces.peek().is_some()
        && info
            .interfaces()
            .any(|interface| interface.interface_string() == Some(INTERFACE_NAME))
    {
        return true;
    }
    info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID
}

pub(crate) fn usb_key(info: &nusb::DeviceInfo) -> String {
    match info.serial_number() {
        Some(serial) if !serial.trim().is_empty() => format!("{USB_PREFIX}{}", serial.trim()),
        // A radio with no serial is named by where it is plugged in, which is stable while it
        // stays in that port; an enumeration index moves as soon as anything else is plugged in.
        _ => format!("{USB_PREFIX}{}-{}", info.bus_id(), info.device_address()),
    }
}

fn label(info: &nusb::DeviceInfo) -> String {
    let name = info
        .product_string()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("AD936x");
    match info
        .serial_number()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(serial) => format!("{name} {}", short(serial)),
        None => name.to_string(),
    }
}

/// Serials on these boards are long; the tail is what is printed on the label and what tells two
/// of them apart.
fn short(serial: &str) -> String {
    let start = serial.len().saturating_sub(8);
    serial[start..].to_string()
}

pub(crate) fn usb_radios() -> Vec<UsbRadio> {
    let devices = match nusb::list_devices().wait() {
        Ok(devices) => devices,
        Err(e) => {
            tracing::debug!("ad936x usb enumeration unavailable: {e}");
            return Vec::new();
        }
    };
    devices
        .filter(serves_iio)
        .map(|info| UsbRadio {
            key: usb_key(&info),
            label: label(&info),
            serial: info
                .serial_number()
                .map(str::trim)
                .filter(|serial| !serial.is_empty())
                .map(str::to_string),
        })
        .collect()
}

pub(crate) fn find_usb(key: &str) -> Result<nusb::DeviceInfo, DeviceError> {
    let devices = nusb::list_devices()
        .wait()
        .map_err(|e| DeviceError::Io(format!("listing usb devices: {e}")))?;
    devices
        .filter(serves_iio)
        .find(|info| usb_key(info) == key)
        .ok_or_else(|| DeviceError::NotFound(format!("no AD936x radio at {key}")))
}

/// Confirms that this USB device really does serve iiod before it is offered, so a board that
/// only matched on identity is not listed as something it cannot be.
pub(crate) fn has_iio_interface(info: &nusb::DeviceInfo) -> bool {
    iio_interface(info).is_ok()
}

pub(crate) fn well_known() -> Vec<Endpoint> {
    WELL_KNOWN
        .iter()
        .filter_map(|host| Endpoint::parse(host, DEFAULT_PORT).ok())
        .collect()
}

/// The addresses that answer, asked all at once so a search costs one wait rather than four.
pub(crate) fn reachable(candidates: Vec<Endpoint>) -> Vec<Endpoint> {
    let probes: Vec<_> = candidates
        .into_iter()
        .filter_map(|endpoint| {
            std::thread::Builder::new()
                .name("sdrmm-ad936x-probe".to_string())
                .spawn(move || {
                    endpoint.connect_within(REACH_TIMEOUT).ok().map(|socket| {
                        let _ = socket.shutdown(std::net::Shutdown::Both);
                        endpoint
                    })
                })
                .inspect_err(|e| tracing::debug!("ad936x search: {e}"))
                .ok()
        })
        .collect();
    probes
        .into_iter()
        .filter_map(|probe| probe.join().ok().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_well_known_address_is_a_usable_endpoint_on_the_iiod_port() {
        let found = well_known();
        assert_eq!(found.len(), WELL_KNOWN.len());
        for endpoint in &found {
            assert!(
                endpoint.to_string().ends_with(":30431"),
                "{endpoint} is not on the iiod port"
            );
        }
        assert_eq!(found[0].to_string(), "ant.local:30431");
    }

    #[test]
    fn a_long_serial_is_shortened_to_the_tail_that_is_printed_on_the_board() {
        let serial = "1044734c960500111e002e0041984fc267";
        let tail = short(serial);
        assert_eq!(tail.len(), 8);
        assert!(
            serial.ends_with(&tail),
            "{tail} is not the tail of {serial}"
        );
        assert_eq!(short("abc"), "abc", "a short serial is left whole");
        assert_eq!(short(""), "");
    }

    #[test]
    fn an_address_nothing_listens_on_is_not_reported_as_reachable() {
        let endpoint = Endpoint::parse("127.0.0.1:1", DEFAULT_PORT).expect("endpoint");
        assert!(reachable(vec![endpoint]).is_empty());
    }

    #[test]
    fn a_reachable_address_comes_back_from_a_search() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let endpoint =
            Endpoint::parse(&format!("127.0.0.1:{port}"), DEFAULT_PORT).expect("endpoint");
        assert_eq!(reachable(vec![endpoint.clone()]), vec![endpoint]);
    }

    #[test]
    fn searching_nowhere_costs_nothing() {
        assert!(reachable(Vec::new()).is_empty());
    }
}
