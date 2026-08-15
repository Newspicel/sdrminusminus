use sdrmm_wire::{NanoVnaDevice, NanoVnaMatch};

const KNOWN_IDS: &[(u16, u16)] = &[
    (0x0483, 0x5740),
    (0x16c0, 0x0483),
    (0x04b4, 0x0008),
    (0x0403, 0x6001),
];

const NAME_MARKERS: &[&str] = &["nanovna", "litevna", "deepvna", "sysjoint", "vna"];

pub fn partition(ports: Vec<serialport::SerialPortInfo>) -> (Vec<NanoVnaDevice>, Vec<String>) {
    let mut devices = Vec::new();
    let mut ignored = Vec::new();
    for info in callout_nodes(ports) {
        match identify(&info) {
            Some(device) => devices.push(device),
            None => ignored.push(info.port_name),
        }
    }
    devices.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| left.port.cmp(&right.port))
    });
    ignored.sort();
    (devices, ignored)
}

fn callout_nodes(ports: Vec<serialport::SerialPortInfo>) -> Vec<serialport::SerialPortInfo> {
    let callouts: Vec<String> = ports
        .iter()
        .filter_map(|info| info.port_name.strip_prefix("/dev/cu.").map(str::to_owned))
        .collect();
    ports
        .into_iter()
        .filter(|info| {
            info.port_name
                .strip_prefix("/dev/tty.")
                .is_none_or(|name| !callouts.iter().any(|callout| callout == name))
        })
        .collect()
}

fn identify(info: &serialport::SerialPortInfo) -> Option<NanoVnaDevice> {
    let serialport::SerialPortType::UsbPort(usb) = &info.port_type else {
        return None;
    };
    let named = names_a_vna(usb.product.as_deref()) || names_a_vna(usb.manufacturer.as_deref());
    let match_kind = if named {
        NanoVnaMatch::Confirmed
    } else if KNOWN_IDS.contains(&(usb.vid, usb.pid)) {
        NanoVnaMatch::Probable
    } else {
        return None;
    };
    let model = usb.product.as_deref().map(tidy);
    let port = info.port_name.clone();
    Some(NanoVnaDevice {
        label: match &model {
            Some(model) => format!("{model} · {port}"),
            None => format!("Unnamed VNA · {port}"),
        },
        port,
        match_kind,
        model,
        manufacturer: usb.manufacturer.clone(),
        product: usb.product.clone(),
        serial_number: usb.serial_number.clone(),
        usb_vid: Some(usb.vid),
        usb_pid: Some(usb.pid),
    })
}

fn names_a_vna(field: Option<&str>) -> bool {
    let Some(field) = field else {
        return false;
    };
    let field = field.to_ascii_lowercase();
    NAME_MARKERS.iter().any(|marker| field.contains(marker))
}

fn tidy(product: &str) -> String {
    product.replace('_', "-").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(
        vid: u16,
        pid: u16,
        product: Option<&str>,
        manufacturer: Option<&str>,
    ) -> serialport::SerialPortInfo {
        serialport::SerialPortInfo {
            port_name: "/dev/cu.usbmodem4001".to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: Some("400".to_owned()),
                manufacturer: manufacturer.map(str::to_owned),
                product: product.map(str::to_owned),
            }),
        }
    }

    #[test]
    fn the_attached_h4_is_confirmed_by_its_own_name() {
        let device = identify(&usb(
            0x0483,
            0x5740,
            Some("NanoVNA_H4"),
            Some("nanovna.com"),
        ))
        .expect("a NanoVNA-H4 is an instrument");
        assert_eq!(device.match_kind, NanoVnaMatch::Confirmed);
        assert_eq!(device.model.as_deref(), Some("NanoVNA-H4"));
        assert_eq!(device.label, "NanoVNA-H4 · /dev/cu.usbmodem4001");
    }

    #[test]
    fn a_bare_stm32_cdc_is_offered_but_not_claimed() {
        let device = identify(&usb(0x0483, 0x5740, Some("STM32 Virtual ComPort"), None))
            .expect("a known id is worth offering");
        assert_eq!(device.match_kind, NanoVnaMatch::Probable);
    }

    #[test]
    fn unrelated_serial_hardware_is_left_out_of_the_list() {
        assert!(identify(&usb(0x1546, 0x01a8, Some("u-blox GNSS receiver"), None)).is_none());
        assert!(
            identify(&serialport::SerialPortInfo {
                port_name: "/dev/cu.Bluetooth-Incoming-Port".to_owned(),
                port_type: serialport::SerialPortType::BluetoothPort,
            })
            .is_none()
        );
    }

    #[test]
    fn the_dial_in_twin_of_a_call_out_node_is_dropped() {
        let (devices, ignored) = partition(vec![
            serialport::SerialPortInfo {
                port_name: "/dev/tty.usbmodem4001".to_owned(),
                ..usb(0x0483, 0x5740, Some("NanoVNA_H4"), Some("nanovna.com"))
            },
            usb(0x0483, 0x5740, Some("NanoVNA_H4"), Some("nanovna.com")),
            serialport::SerialPortInfo {
                port_name: "/dev/tty.usbserial-lonely".to_owned(),
                ..usb(0x1546, 0x01a8, Some("u-blox GNSS receiver"), None)
            },
        ]);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].port, "/dev/cu.usbmodem4001");
        assert_eq!(ignored, vec!["/dev/tty.usbserial-lonely".to_owned()]);
    }

    #[test]
    fn discovery_lists_instruments_first_and_names_what_it_skipped() {
        let (devices, ignored) = partition(vec![
            usb(0x1546, 0x01a8, Some("u-blox GNSS receiver"), None),
            usb(0x0483, 0x5740, Some("STM32 Virtual ComPort"), None),
            serialport::SerialPortInfo {
                port_name: "/dev/cu.usbmodem4001".to_owned(),
                ..usb(0x0483, 0x5740, Some("NanoVNA_H4"), Some("nanovna.com"))
            },
        ]);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].match_kind, NanoVnaMatch::Confirmed);
        assert_eq!(ignored, vec!["/dev/cu.usbmodem4001".to_owned()]);
    }
}
