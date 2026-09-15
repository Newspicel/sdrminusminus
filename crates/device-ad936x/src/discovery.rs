use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use nusb::MaybeFuture;
use sdrmm_device::{DeviceError, net::Endpoint};

use crate::{
    iio::{Client, DEFAULT_PORT, NetTransport, named_interface},
    layout::Layout,
};

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
pub(crate) const WELL_KNOWN: [&str; 4] =
    ["ant.local", "192.168.1.10", "pluto.local", "192.168.2.1"];

/// How long a search waits on an address nobody said was there.
const REACH_TIMEOUT: Duration = Duration::from_millis(400);

/// How long a host that answered the door is given to describe itself.
const GREET_TIMEOUT: Duration = Duration::from_secs(1);

/// The whole search, name resolution included. An address still unanswered by then is treated
/// as empty rather than holding up whoever asked.
const SWEEP_TIMEOUT: Duration = Duration::from_millis(1_800);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsbRadio {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) serial: Option<String>,
}

/// A radio a search found at one of the addresses it tried.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Found {
    pub(crate) endpoint: Endpoint,
    pub(crate) serial: Option<String>,
}

/// Whether this USB device serves iiod. The interface name is the reliable answer and the
/// identity is the fallback for the platforms that do not report interface names.
fn serves_iio(info: &nusb::DeviceInfo) -> bool {
    named_interface(info).is_some()
        || (info.vendor_id() == VENDOR_ID && info.product_id() == PRODUCT_ID)
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
    let start = serial.char_indices().rev().nth(7).map_or(0, |(at, _)| at);
    serial[start..].to_string()
}

fn usb_radio(info: &nusb::DeviceInfo) -> UsbRadio {
    UsbRadio {
        key: usb_key(info),
        label: label(info),
        serial: info
            .serial_number()
            .map(str::trim)
            .filter(|serial| !serial.is_empty())
            .map(str::to_string),
    }
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
        .map(|info| usb_radio(&info))
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

pub(crate) fn attached(key: &str) -> Option<UsbRadio> {
    find_usb(key).ok().map(|info| usb_radio(&info))
}

/// The radios at these addresses, asked all at once so a search costs one wait rather than one
/// per address. Only a host that describes itself as an AD936x counts: a port that merely
/// accepts a connection is anybody's.
pub(crate) fn sweep(candidates: Vec<Endpoint>) -> Vec<Found> {
    let (report, reports) = mpsc::channel();
    let mut asked = 0;
    for endpoint in candidates {
        let report = report.clone();
        let spawned = std::thread::Builder::new()
            .name("sdrmm-ad936x-probe".to_string())
            .spawn(move || {
                let _ = report.send(greet(endpoint));
            });
        match spawned {
            Ok(_) => asked += 1,
            Err(e) => tracing::debug!("ad936x search: {e}"),
        }
    }
    drop(report);
    let deadline = Instant::now() + SWEEP_TIMEOUT;
    let mut found = Vec::new();
    for _ in 0..asked {
        let patience = deadline.saturating_duration_since(Instant::now());
        match reports.recv_timeout(patience) {
            Ok(Some(radio)) => found.push(radio),
            Ok(None) => {}
            Err(_) => break,
        }
    }
    found
}

fn greet(endpoint: Endpoint) -> Option<Found> {
    let transport = NetTransport::connect_within(&endpoint, REACH_TIMEOUT).ok()?;
    let client = Client::new(Arc::new(transport));
    let context = client
        .context_within(GREET_TIMEOUT)
        .inspect_err(|e| tracing::debug!(%endpoint, "not an iiod host: {e}"))
        .ok()?;
    client.close();
    Layout::read(&context)
        .inspect_err(|e| tracing::debug!(%endpoint, "an iiod host but not an AD936x: {e}"))
        .ok()?;
    Some(Found {
        endpoint,
        serial: context.attribute("hw_serial").map(str::to_string),
    })
}

pub(crate) fn endpoints(hosts: impl IntoIterator<Item = String>) -> Vec<Endpoint> {
    hosts
        .into_iter()
        .filter_map(|host| {
            Endpoint::parse(&host, DEFAULT_PORT)
                .inspect_err(|e| tracing::warn!("ad936x search address: {e}"))
                .ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::{Read as _, Write as _};

    use sdrmm_device::net::testing::FakeServer;

    use super::*;

    #[test]
    fn every_well_known_address_is_a_usable_endpoint_on_the_iiod_port() {
        let found = endpoints(WELL_KNOWN.iter().map(|host| host.to_string()));
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
        assert_eq!(short("12345678"), "12345678");
    }

    #[test]
    fn a_serial_that_is_not_ascii_is_shortened_by_character_rather_than_by_byte() {
        assert_eq!(short("ab序列号CDEFGH"), "列号CDEFGH");
        assert_eq!(short("序列号"), "序列号");
    }

    #[test]
    fn an_address_nothing_listens_on_is_not_reported_as_reachable() {
        let endpoint = Endpoint::parse("127.0.0.1:1", DEFAULT_PORT).expect("endpoint");
        assert!(sweep(vec![endpoint]).is_empty());
    }

    #[test]
    fn a_host_that_answers_but_is_not_iiod_is_not_adopted() {
        let server = FakeServer::spawn(|mut stream, _| {
            let mut command = [0u8; 8];
            let _ = stream.read(&mut command);
            let _ = stream.write_all(b"220 smtp ready\r\n");
        });
        let endpoint = Endpoint::parse(&server.endpoint(), DEFAULT_PORT).expect("endpoint");
        assert!(sweep(vec![endpoint]).is_empty());
    }

    #[test]
    fn searching_nowhere_costs_nothing() {
        assert!(sweep(Vec::new()).is_empty());
    }
}
