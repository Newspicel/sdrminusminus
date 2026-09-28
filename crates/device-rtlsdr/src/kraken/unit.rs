use std::collections::BTreeMap;

use crate::driver::DeviceDescriptor;

const FIRST_SERIAL: u32 = 1000;
const KRAKEN_LANES: u32 = 5;
const KERBEROS_LANES: u32 = 4;

pub(crate) const KRAKEN_HUB: (u16, u16) = (0x0424, 0x2517);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Model {
    Kraken,
    Kerberos,
}

impl Model {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Kraken => "KrakenSDR",
            Self::Kerberos => "KerberosSDR",
        }
    }

    pub(crate) const fn lanes(self) -> u32 {
        match self {
            Self::Kraken => KRAKEN_LANES,
            Self::Kerberos => KERBEROS_LANES,
        }
    }

    fn identify(hub: Option<(u16, u16)>, present: &[u32]) -> Option<Self> {
        let past_kerberos =
            present.len() >= KERBEROS_LANES as usize && present.contains(&(KRAKEN_LANES - 1));
        if hub == Some(KRAKEN_HUB) || past_kerberos {
            return Some(Self::Kraken);
        }
        present
            .iter()
            .copied()
            .eq(0..KERBEROS_LANES)
            .then_some(Self::Kerberos)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unit {
    pub(crate) key: String,
    pub(crate) model: Model,
    pub(crate) members: Vec<usize>,
    pub(crate) missing: Vec<u32>,
}

impl Unit {
    pub(crate) fn complete(&self) -> bool {
        self.missing.is_empty()
    }

    pub(crate) const fn expected_lanes(&self) -> u32 {
        self.model.lanes()
    }

    pub(crate) fn label(&self) -> String {
        if self.complete() {
            return format!("{} ({})", self.model.name(), self.key);
        }
        let missing: Vec<String> = self.missing.iter().map(u32::to_string).collect();
        format!("{} ({} missing)", self.model.name(), missing.join(", "))
    }
}

fn lane_of(descriptor: &DeviceDescriptor) -> Option<u32> {
    let serial: u32 = descriptor.serial.as_deref()?.parse().ok()?;
    let lane = serial.checked_sub(FIRST_SERIAL)?;
    (lane < KRAKEN_LANES).then_some(lane)
}

fn hub_key(descriptor: &DeviceDescriptor) -> String {
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

#[derive(Default)]
struct Behind {
    hub: Option<(u16, u16)>,
    lanes: Vec<(u32, usize)>,
}

fn unit(key: String, behind: Behind) -> Option<Unit> {
    let mut found = behind.lanes;
    found.sort_unstable();
    let present: Vec<u32> = found.iter().map(|(lane, _)| *lane).collect();
    if present.windows(2).any(|pair| pair[0] == pair[1]) {
        return None;
    }
    let model = Model::identify(behind.hub, &present)?;
    let missing = (0..model.lanes())
        .filter(|lane| !present.contains(lane))
        .map(|lane| FIRST_SERIAL + lane)
        .collect();
    Some(Unit {
        key,
        model,
        members: found.into_iter().map(|(_, index)| index).collect(),
        missing,
    })
}

pub(crate) fn units(descriptors: &[DeviceDescriptor]) -> Vec<Unit> {
    let mut behind: BTreeMap<String, Behind> = BTreeMap::new();
    for (index, descriptor) in descriptors.iter().enumerate() {
        if let Some(lane) = lane_of(descriptor) {
            let group = behind.entry(hub_key(descriptor)).or_default();
            group.hub = group.hub.or(descriptor.hub);
            group.lanes.push((lane, index));
        }
    }
    behind
        .into_iter()
        .filter_map(|(key, behind)| unit(key, behind))
        .collect()
}

pub(crate) fn claimed(descriptors: &[DeviceDescriptor]) -> Vec<usize> {
    units(descriptors)
        .into_iter()
        .flat_map(|unit| unit.members)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::driver::BoardVariant;

    const OTHER_HUB: (u16, u16) = (0x05e3, 0x0610);

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
            hub: None,
        }
    }

    pub(crate) fn lanes_behind(
        bus: &str,
        hub: u8,
        id: Option<(u16, u16)>,
        lanes: &[u32],
    ) -> Vec<DeviceDescriptor> {
        lanes
            .iter()
            .map(|lane| DeviceDescriptor {
                hub: id,
                ..dongle(
                    bus,
                    &[hub, *lane as u8 + 1],
                    Some(&(FIRST_SERIAL + lane).to_string()),
                )
            })
            .collect()
    }

    fn unit_behind(bus: &str, hub: u8, count: u32) -> Vec<DeviceDescriptor> {
        lanes_behind(bus, hub, Some(KRAKEN_HUB), &(0..count).collect::<Vec<_>>())
    }

    #[test]
    fn five_chains_behind_one_hub_are_one_radio() {
        let found = units(&unit_behind("0", 3, 5));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, Model::Kraken);
        assert_eq!(found[0].members, vec![0, 1, 2, 3, 4]);
        assert_eq!(found[0].key, "0/3");
        assert!(found[0].complete());
        assert_eq!(found[0].label(), "KrakenSDR (0/3)");
    }

    #[test]
    fn five_serials_behind_an_unknown_hub_are_a_kraken() {
        let found = units(&lanes_behind("0", 3, None, &[0, 1, 2, 3, 4]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, Model::Kraken);
        assert!(found[0].complete());
    }

    #[test]
    fn kerberos_needs_a_non_kraken_hub() {
        let kerberos = units(&lanes_behind("0", 3, Some(OTHER_HUB), &[0, 1, 2, 3]));
        assert_eq!(kerberos.len(), 1);
        assert_eq!(kerberos[0].model, Model::Kerberos);
        assert_eq!(kerberos[0].expected_lanes(), 4);
        assert!(kerberos[0].complete());
        assert_eq!(kerberos[0].label(), "KerberosSDR (0/3)");
        let unknown = units(&lanes_behind("0", 3, None, &[0, 1, 2, 3]));
        assert_eq!(unknown[0].model, Model::Kerberos);
        let kraken = units(&lanes_behind("0", 3, Some(KRAKEN_HUB), &[0, 1, 2, 3]));
        assert_eq!(kraken[0].model, Model::Kraken);
    }

    #[test]
    fn an_incomplete_kraken_is_not_a_kerberos() {
        let found = units(&unit_behind("0", 3, 4));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, Model::Kraken);
        assert_eq!(found[0].missing, vec![1004]);
        assert!(!found[0].complete());
        assert_eq!(found[0].expected_lanes(), 5);
        assert_eq!(found[0].label(), "KrakenSDR (1004 missing)");
        assert_eq!(claimed(&unit_behind("0", 3, 4)).len(), 4);
    }

    #[test]
    fn lanes_are_numbered_by_serial_rather_than_by_enumeration_order() {
        let mut descriptors = unit_behind("0", 3, 5);
        descriptors.reverse();
        let found = units(&descriptors);
        assert_eq!(found[0].members, vec![4, 3, 2, 1, 0]);
    }

    #[test]
    fn a_missing_chain_is_claimed_and_named_rather_than_offered_loose() {
        let mut descriptors = lanes_behind("0", 3, None, &[0, 1, 2, 3, 4]);
        descriptors.remove(2);
        let found = units(&descriptors);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].model, Model::Kraken);
        assert_eq!(found[0].missing, vec![1002]);
        assert_eq!(found[0].label(), "KrakenSDR (1002 missing)");
        assert_eq!(claimed(&descriptors).len(), 4);
    }

    #[test]
    fn a_few_kraken_serials_behind_another_hub_stay_loose() {
        assert!(units(&lanes_behind("0", 3, Some(OTHER_HUB), &[0, 1])).is_empty());
        assert!(units(&lanes_behind("0", 3, Some(OTHER_HUB), &[4])).is_empty());
        assert!(units(&lanes_behind("0", 3, None, &[2, 3, 4])).is_empty());
    }

    #[test]
    fn a_few_lanes_behind_the_kraken_hub_are_an_incomplete_kraken() {
        let found = units(&unit_behind("0", 3, 2));
        assert_eq!(found[0].model, Model::Kraken);
        assert_eq!(found[0].missing, vec![1002, 1003, 1004]);
        assert_eq!(found[0].label(), "KrakenSDR (1002, 1003, 1004 missing)");
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
    fn two_units_that_cannot_be_told_apart_are_not_grouped() {
        let descriptors: Vec<DeviceDescriptor> = (0..10)
            .map(|lane| dongle("0", &[], Some(&(FIRST_SERIAL + lane % 5).to_string())))
            .collect();
        assert!(units(&descriptors).is_empty());
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
