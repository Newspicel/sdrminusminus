use std::net::Ipv6Addr;

use sdrmm_wire::position::{
    DEFAULT_NMEA_BAUD, DEFAULT_NMEA_UPDATE_INTERVAL_MS, MAX_NMEA_BAUD, MIN_NMEA_BAUD,
    NmeaDeviceInfo, PositionSource,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpsTab {
    Receiver,
    Network,
    Fixed,
    Device,
}

#[must_use]
pub fn gps_tabs(has_geolocation: bool) -> Vec<(GpsTab, String)> {
    let mut tabs = vec![
        (GpsTab::Receiver, "Receiver".to_owned()),
        (GpsTab::Network, "Network".to_owned()),
        (GpsTab::Fixed, "Fixed".to_owned()),
    ];
    if has_geolocation {
        tabs.push((GpsTab::Device, "This device".to_owned()));
    }
    tabs
}

fn valid_host(host: &str) -> bool {
    if host.starts_with('[') || host.ends_with(']') {
        return host
            .strip_prefix('[')
            .and_then(|inner| inner.strip_suffix(']'))
            .is_some_and(|inner| inner.parse::<Ipv6Addr>().is_ok());
    }
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[must_use]
pub fn valid_gpsd_address(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    let port_ok = port
        .parse::<u32>()
        .is_ok_and(|port| (1..=65_535).contains(&port));
    !host.is_empty() && port_ok && valid_host(host)
}

#[must_use]
pub fn valid_baud(text: &str) -> Option<u32> {
    text.trim()
        .parse::<u32>()
        .ok()
        .filter(|baud| (MIN_NMEA_BAUD..=MAX_NMEA_BAUD).contains(baud))
}

#[must_use]
pub fn nmea_detail(device: &NmeaDeviceInfo) -> String {
    [
        device.product.as_ref().or(device.manufacturer.as_ref()),
        device.serial.as_ref(),
    ]
    .into_iter()
    .flatten()
    .filter(|part| !part.is_empty())
    .cloned()
    .collect::<Vec<_>>()
    .join(" · ")
}

#[must_use]
pub fn nmea_suggestion(device: &NmeaDeviceInfo) -> (String, Option<String>) {
    let Some(description) = device.product.as_ref().or(device.manufacturer.as_ref()) else {
        return (device.path.clone(), None);
    };
    let serial = device
        .serial
        .as_ref()
        .map_or_else(String::new, |serial| format!(" · {serial}"));
    (device.path.clone(), Some(format!("{description}{serial}")))
}

#[must_use]
pub fn filter_nmea_devices(devices: &[NmeaDeviceInfo], query: &str) -> Vec<NmeaDeviceInfo> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return devices.to_vec();
    }
    devices
        .iter()
        .filter(|device| {
            [
                Some(&device.path),
                device.product.as_ref(),
                device.manufacturer.as_ref(),
                device.serial.as_ref(),
            ]
            .into_iter()
            .flatten()
            .any(|part| part.to_lowercase().contains(&needle))
        })
        .cloned()
        .collect()
}

#[must_use]
pub fn nmea_source(path: &str) -> PositionSource {
    PositionSource::Nmea {
        device: path.to_owned(),
        baud: DEFAULT_NMEA_BAUD,
        update_interval_ms: DEFAULT_NMEA_UPDATE_INTERVAL_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(
        path: &str,
        product: Option<&str>,
        manufacturer: Option<&str>,
        serial: Option<&str>,
    ) -> NmeaDeviceInfo {
        NmeaDeviceInfo {
            path: path.to_owned(),
            product: product.map(str::to_owned),
            manufacturer: manufacturer.map(str::to_owned),
            serial: serial.map(str::to_owned),
            usb_vid: None,
            usb_pid: None,
        }
    }

    #[test]
    fn this_devices_location_is_offered_only_where_one_can_be_reported() {
        let values =
            |tabs: Vec<(GpsTab, String)>| tabs.into_iter().map(|(tab, _)| tab).collect::<Vec<_>>();
        assert_eq!(
            values(gps_tabs(true)),
            vec![
                GpsTab::Receiver,
                GpsTab::Network,
                GpsTab::Fixed,
                GpsTab::Device
            ]
        );
        assert_eq!(
            values(gps_tabs(false)),
            vec![GpsTab::Receiver, GpsTab::Network, GpsTab::Fixed]
        );
    }

    #[test]
    fn host_and_bracketed_ipv6_endpoints_are_accepted() {
        assert!(valid_gpsd_address("127.0.0.1:2947"));
        assert!(valid_gpsd_address("gps.local:2947"));
        assert!(valid_gpsd_address("[::1]:2947"));
    }

    #[test]
    fn missing_hosts_ports_and_malformed_endpoints_are_refused() {
        for address in [
            "",
            "localhost",
            "localhost:0",
            "bad host:2947",
            "[::1:2947",
            "[::::]:2947",
            "[1:2:3:4:5:6:7:8:9]:2947",
        ] {
            assert!(!valid_gpsd_address(address), "{address}");
        }
    }

    #[test]
    fn a_suggestion_names_the_receiver_behind_the_path() {
        let ublox = device(
            "/dev/cu.usbmodem11401",
            Some("GNSS receiver"),
            Some("u-blox"),
            Some("GPS-1"),
        );
        assert_eq!(
            nmea_suggestion(&ublox),
            (
                "/dev/cu.usbmodem11401".to_owned(),
                Some("GNSS receiver · GPS-1".to_owned())
            )
        );
        assert_eq!(
            nmea_suggestion(&device("/dev/ttyS0", None, None, None)),
            ("/dev/ttyS0".to_owned(), None)
        );
    }

    #[test]
    fn the_receiver_list_names_filters_and_reads_at_the_shipped_rate() {
        let devices = [
            device(
                "/dev/cu.usbmodem11401",
                Some("GNSS receiver"),
                Some("u-blox"),
                None,
            ),
            device("/dev/ttyS0", None, None, None),
        ];
        assert_eq!(nmea_detail(&devices[0]), "GNSS receiver");
        assert_eq!(
            nmea_detail(&NmeaDeviceInfo {
                serial: Some("GPS-1".to_owned()),
                ..devices[0].clone()
            }),
            "GNSS receiver · GPS-1"
        );
        assert_eq!(nmea_detail(&devices[1]), "");
        let paths =
            |found: Vec<NmeaDeviceInfo>| found.into_iter().map(|d| d.path).collect::<Vec<_>>();
        assert_eq!(
            paths(filter_nmea_devices(&devices, " USBMODEM ")),
            vec!["/dev/cu.usbmodem11401"]
        );
        assert_eq!(
            paths(filter_nmea_devices(&devices, "u-blox")),
            vec!["/dev/cu.usbmodem11401"]
        );
        assert_eq!(filter_nmea_devices(&devices, " ").len(), 2);
        assert!(filter_nmea_devices(&devices, "garmin").is_empty());
        assert_eq!(
            nmea_source("/dev/ttyS0"),
            PositionSource::Nmea {
                device: "/dev/ttyS0".to_owned(),
                baud: 9_600,
                update_interval_ms: 1_000
            }
        );
    }

    #[test]
    fn a_baud_is_an_integer_the_port_can_run() {
        assert_eq!(valid_baud("9600"), Some(9_600));
        assert_eq!(valid_baud("300"), None);
        assert_eq!(valid_baud("9600.5"), None);
        assert_eq!(valid_baud("fast"), None);
    }
}
