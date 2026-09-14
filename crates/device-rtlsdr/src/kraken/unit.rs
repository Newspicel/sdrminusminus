use std::collections::BTreeMap;

use crate::driver::DeviceDescriptor;

/// The serial KrakenRF gives the first receive chain of a unit; the rest count up from it.
const FIRST_SERIAL: u32 = 1000;

const KRAKEN_LANES: usize = 5;
const KERBEROS_LANES: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unit {
    pub(crate) key: String,
    pub(crate) model: &'static str,
    /// Where each lane sits in the enumerated dongle list, in lane order.
    pub(crate) members: Vec<usize>,
}

impl Unit {
    pub(crate) fn label(&self) -> String {
        format!("{} ({})", self.model, self.key)
    }

    pub(crate) fn lanes(&self) -> u32 {
        self.members.len() as u32
    }
}

fn lane_of(descriptor: &DeviceDescriptor) -> Option<usize> {
    let serial: u32 = descriptor.serial.as_deref()?.parse().ok()?;
    let lane = serial.checked_sub(FIRST_SERIAL)? as usize;
    (lane < KRAKEN_LANES).then_some(lane)
}

/// What the dongle hangs off, which for these units is the hub built into the case. Two units on
/// one machine carry the same serials, so where they are plugged in is the only thing that tells
/// them apart.
fn hub(descriptor: &DeviceDescriptor) -> String {
    let ports = descriptor
        .port_chain
        .split_last()
        .map_or(&[][..], |(_, above)| above);
    if ports.is_empty() {
        return descriptor.bus.clone();
    }
    let path: Vec<String> = ports.iter().map(u8::to_string).collect();
    format!("{}/{}", descriptor.bus, path.join("."))
}

/// Finds the coherent units among the attached dongles.
///
/// A unit is a full run of KrakenRF serials behind one hub. A partial run is left alone: four of
/// five chains is a broken radio, not a smaller one, and grouping it would hide the fault behind
/// an array that quietly measures the wrong thing.
pub(crate) fn units(descriptors: &[DeviceDescriptor]) -> Vec<Unit> {
    let mut behind: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        if let Some(lane) = lane_of(descriptor) {
            behind
                .entry(hub(descriptor))
                .or_default()
                .push((lane, index));
        }
    }
    behind
        .into_iter()
        .filter_map(|(key, mut found)| {
            found.sort_unstable();
            let model = match found.len() {
                KRAKEN_LANES => "KrakenSDR",
                KERBEROS_LANES => "KerberosSDR",
                _ => return None,
            };
            let complete = found.iter().map(|(lane, _)| *lane).eq(0..found.len());
            complete.then(|| Unit {
                key,
                model,
                members: found.into_iter().map(|(_, index)| index).collect(),
            })
        })
        .collect()
}

/// The dongles that belong to a unit, which the single-dongle driver stops offering on its own.
pub(crate) fn claimed(descriptors: &[DeviceDescriptor]) -> Vec<usize> {
    units(descriptors)
        .into_iter()
        .flat_map(|unit| unit.members)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::BoardVariant;

    fn dongle(bus: &str, chain: &[u8], serial: Option<&str>) -> DeviceDescriptor {
        DeviceDescriptor {
            index: 0,
            bus: bus.to_owned(),
            address: chain.last().copied().unwrap_or(1),
            manufacturer: None,
            product: None,
            serial: serial.map(str::to_owned),
            port_chain: chain.to_vec(),
            board_variant: BoardVariant::Generic,
        }
    }

    fn unit_behind(bus: &str, hub: u8, count: u32) -> Vec<DeviceDescriptor> {
        (0..count)
            .map(|lane| {
                dongle(
                    bus,
                    &[hub, lane as u8 + 1],
                    Some(&(FIRST_SERIAL + lane).to_string()),
                )
            })
            .collect()
    }

    #[test]
    fn five_chains_behind_one_hub_are_one_radio() {
        let found = units(&unit_behind("0", 3, 5));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, "KrakenSDR");
        assert_eq!(found[0].members, vec![0, 1, 2, 3, 4]);
        assert_eq!(found[0].key, "0/3");
        assert_eq!(found[0].label(), "KrakenSDR (0/3)");
    }

    #[test]
    fn four_chains_are_the_older_unit() {
        let found = units(&unit_behind("0", 3, 4));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, "KerberosSDR");
        assert_eq!(found[0].lanes(), 4);
    }

    #[test]
    fn lanes_are_numbered_by_serial_rather_than_by_enumeration_order() {
        let mut descriptors = unit_behind("0", 3, 5);
        descriptors.reverse();
        let found = units(&descriptors);
        assert_eq!(found[0].members, vec![4, 3, 2, 1, 0]);
    }

    #[test]
    fn a_missing_chain_is_a_fault_rather_than_a_smaller_radio() {
        let mut descriptors = unit_behind("0", 3, 5);
        descriptors.remove(2);
        assert!(units(&descriptors).is_empty());
    }

    #[test]
    fn ordinary_dongles_are_never_grouped() {
        let descriptors = vec![
            dongle("0", &[1], Some("00000001")),
            dongle("0", &[2], Some("77771111")),
            dongle("0", &[3], None),
        ];
        assert!(units(&descriptors).is_empty());
        assert!(claimed(&descriptors).is_empty());
    }

    #[test]
    fn two_units_on_one_machine_are_told_apart_by_where_they_hang() {
        let mut descriptors = unit_behind("0", 3, 5);
        descriptors.extend(unit_behind("0", 7, 5));
        let found = units(&descriptors);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].key, "0/3");
        assert_eq!(found[1].key, "0/7");
        assert_eq!(claimed(&descriptors).len(), 10);
    }

    #[test]
    fn a_bus_that_reports_no_topology_still_groups_one_unit() {
        let descriptors: Vec<DeviceDescriptor> = (0..5)
            .map(|lane| dongle("0", &[], Some(&(FIRST_SERIAL + lane).to_string())))
            .collect();
        let found = units(&descriptors);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "0");
    }
}
