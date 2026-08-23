use sdrmm_wire::cps::{CpsPort, PortMatch};

use crate::{RadioModel, registry::ModelRegistry};

const RADIO_MARKERS: &[&str] = &["anytone", "at-d", "radtel", "retevis", "rt4", "rd-4"];

#[must_use]
pub fn partition(
    ports: Vec<serialport::SerialPortInfo>,
    registry: &ModelRegistry,
) -> (Vec<CpsPort>, Vec<String>) {
    let mut found = Vec::new();
    let mut ignored = Vec::new();
    for info in callout_nodes(ports) {
        match identify(&info, registry) {
            Some(port) => found.push(port),
            None => ignored.push(info.port_name),
        }
    }
    found.sort_by(|left, right| {
        left.match_kind
            .cmp(&right.match_kind)
            .then_with(|| left.port.cmp(&right.port))
    });
    ignored.sort();
    (found, ignored)
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

fn names_a_radio(field: Option<&str>) -> bool {
    field.is_some_and(|field| {
        let field = field.to_ascii_lowercase();
        RADIO_MARKERS.iter().any(|marker| field.contains(marker))
    })
}

fn matching_models(vid: u16, pid: u16, registry: &ModelRegistry) -> Vec<String> {
    registry
        .iter()
        .filter(|model: &&dyn RadioModel| {
            model
                .descriptor()
                .usb
                .iter()
                .any(|entry| entry.vid == vid && entry.pid == pid)
        })
        .map(|model| model.descriptor().id)
        .collect()
}

fn identify(info: &serialport::SerialPortInfo, registry: &ModelRegistry) -> Option<CpsPort> {
    let serialport::SerialPortType::UsbPort(usb) = &info.port_type else {
        return None;
    };
    let candidate_models = matching_models(usb.vid, usb.pid, registry);
    let named = names_a_radio(usb.product.as_deref()) || names_a_radio(usb.manufacturer.as_deref());
    let match_kind = if named {
        PortMatch::Confirmed
    } else if candidate_models.is_empty() {
        PortMatch::Possible
    } else {
        PortMatch::Probable
    };
    let port = info.port_name.clone();
    let label = match usb.product.as_deref() {
        Some(product) => format!("{} · {port}", product.replace('_', "-").trim()),
        None => format!("Serial port · {port}"),
    };
    Some(CpsPort {
        port,
        label,
        match_kind,
        manufacturer: usb.manufacturer.clone(),
        product: usb.product.clone(),
        serial_number: usb.serial_number.clone(),
        usb_vid: Some(usb.vid),
        usb_pid: Some(usb.pid),
        candidate_models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::models;

    fn usb(vid: u16, pid: u16, product: Option<&str>) -> serialport::SerialPortInfo {
        serialport::SerialPortInfo {
            port_name: "/dev/cu.usbmodem1".to_owned(),
            port_type: serialport::SerialPortType::UsbPort(serialport::UsbPortInfo {
                vid,
                pid,
                serial_number: Some("1".to_owned()),
                manufacturer: None,
                product: product.map(str::to_owned),
            }),
        }
    }

    #[test]
    fn a_generic_stm32_port_is_offered_with_the_models_that_use_that_id() {
        let port = identify(
            &usb(0x0483, 0x5740, Some("STM32 Virtual ComPort")),
            models(),
        )
        .expect("a serial port is worth offering");
        assert_eq!(port.match_kind, PortMatch::Probable);
        assert_eq!(port.candidate_models, vec!["anytone-d890uv".to_owned()]);
    }

    #[test]
    fn an_unknown_serial_port_is_offered_last_and_claims_no_model() {
        let port = identify(&usb(0x1a86, 0x7523, Some("USB Serial")), models())
            .expect("still a serial port");
        assert_eq!(port.match_kind, PortMatch::Possible);
        assert!(port.candidate_models.is_empty());
    }

    #[test]
    fn a_port_that_names_the_radio_is_confirmed() {
        let port = identify(&usb(0x0483, 0x5740, Some("AnyTone AT-D890UV")), models())
            .expect("named radio");
        assert_eq!(port.match_kind, PortMatch::Confirmed);
    }

    #[test]
    fn bluetooth_and_dial_in_twins_are_left_out() {
        let (ports, ignored) = partition(
            vec![
                serialport::SerialPortInfo {
                    port_name: "/dev/tty.usbmodem1".to_owned(),
                    ..usb(0x0483, 0x5740, Some("STM32 Virtual ComPort"))
                },
                usb(0x0483, 0x5740, Some("STM32 Virtual ComPort")),
                serialport::SerialPortInfo {
                    port_name: "/dev/cu.Bluetooth-Incoming-Port".to_owned(),
                    port_type: serialport::SerialPortType::BluetoothPort,
                },
            ],
            models(),
        );
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, "/dev/cu.usbmodem1");
        assert_eq!(ignored, vec!["/dev/cu.Bluetooth-Incoming-Port".to_owned()]);
    }
}
