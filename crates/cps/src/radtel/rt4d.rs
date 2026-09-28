use sdrmm_wire::cps::{
    ChannelKind, ChannelMode, Codeplug, CodeplugMeta, ConversionReport, FrequencyRange, Power,
    RadioFeatures, RadioId, RadioLimits, RadioModelDescriptor,
};

use super::{
    channel::{self, Decoded, Links, Targets},
    layout::{CHANNELS, CONTACTS, GROUP_LISTS, REGIONS, SETTINGS, SlotNames, ZONES},
    protocol::Rt4DSession,
    records::{self, GROUP_LIST_MEMBERS, GROUP_LIST_NAME_LEN, Listing, NAME_LEN, ZONE_MEMBERS},
};
use crate::{
    CpsError, Image, RadioModel, RadioSession, Region, SerialLink,
    catalog::{Catalog, NameList, UniqueNames},
    convert::fit,
};

pub const MODEL_ID: &str = "radtel-rt4d";
const RADIO_ID_NAME: &str = "Radio ID";

pub struct Rt4D;

#[must_use]
pub fn limits() -> RadioLimits {
    RadioLimits {
        channels: CHANNELS.count,
        contacts: CONTACTS.count,
        group_lists: GROUP_LISTS.count,
        group_list_members: GROUP_LIST_MEMBERS as u32,
        zones: ZONES.count,
        zone_channels: ZONE_MEMBERS as u32,
        scan_lists: 0,
        scan_list_members: 0,
        radio_ids: 1,
        channel_name_len: channel::NAME_LEN as u32,
        contact_name_len: NAME_LEN as u32,
        group_list_name_len: GROUP_LIST_NAME_LEN as u32,
        zone_name_len: NAME_LEN as u32,
        scan_list_name_len: 0,
        radio_id_name_len: NAME_LEN as u32,
        rx_ranges: vec![FrequencyRange::new(18_000_000, 1_000_000_000)],
        tx_ranges: vec![
            FrequencyRange::new(136_000_000, 174_000_000),
            FrequencyRange::new(400_000_000, 520_000_000),
        ],
        powers: vec![Power::Low, Power::High],
        modes: vec![ChannelKind::Fm, ChannelKind::Dmr],
        frequency_step_hz: 10,
        features: RadioFeatures {
            per_channel_radio_id: true,
            group_lists: true,
            dcs_tones: true,
            ..RadioFeatures::default()
        },
    }
}

impl RadioModel for Rt4D {
    fn descriptor(&self) -> RadioModelDescriptor {
        RadioModelDescriptor {
            id: MODEL_ID.to_owned(),
            manufacturer: "Radtel".to_owned(),
            model: "RT-4D".to_owned(),
            family: MODEL_ID.to_owned(),
            usb: Vec::new(),
            needs_explicit_selection: true,
            transfer_bytes: self.transfer_bytes(),
            limits: limits(),
        }
    }

    fn regions(&self) -> &'static [Region] {
        &REGIONS
    }

    fn erased_byte(&self) -> u8 {
        super::layout::ERASED
    }

    fn open(&self, link: Box<dyn SerialLink>) -> Result<Box<dyn RadioSession>, CpsError> {
        Ok(Box::new(Rt4DSession::open(link)?))
    }

    fn decode(&self, image: &Image) -> Result<Codeplug, CpsError> {
        let mut codeplug = Codeplug::empty();
        codeplug.meta = CodeplugMeta {
            source_model: Some(MODEL_ID.to_owned()),
            ..CodeplugMeta::default()
        };
        read_radio_id(image, &mut codeplug);
        let contacts = read_contacts(image, &mut codeplug);
        let group_lists = read_group_lists(image, &contacts, &mut codeplug);
        let links = Links {
            contacts: &contacts,
            group_lists: &group_lists,
        };
        let channels = read_channels(image, &links, &mut codeplug);
        read_zones(image, &channels, &mut codeplug);
        Ok(codeplug)
    }

    fn encode(&self, codeplug: &Codeplug, image: &mut Image) -> Result<ConversionReport, CpsError> {
        let (fitted, report) = fit(codeplug, MODEL_ID, &limits());
        let catalog = Catalog::of(&fitted);
        let default_id = default_radio_number(&fitted);
        write_radio_id(image, default_id)?;
        CONTACTS.fill(image, &fitted.contacts, records::put_contact)?;
        let group_lists = listings(&fitted.group_lists, &catalog.contacts, |list| {
            (&list.name, &list.contacts)
        });
        GROUP_LISTS.fill(image, &group_lists, records::put_group_list)?;
        CHANNELS.fill(image, &fitted.channels, |record, item| {
            let targets = targets(item, &catalog, &fitted, default_id);
            channel::encode(record, item, &targets);
        })?;
        let zones = listings(&fitted.zones, &catalog.channels, |zone| {
            (&zone.name, &zone.channels_a)
        });
        ZONES.fill(image, &zones, records::put_zone)?;
        Ok(report)
    }
}

fn read_radio_id(image: &Image, codeplug: &mut Codeplug) {
    let Some(number) = SETTINGS.record(image, 0).and_then(records::radio_id) else {
        return;
    };
    codeplug.radio_ids.push(RadioId {
        name: RADIO_ID_NAME.to_owned(),
        number,
    });
    codeplug.settings.default_radio_id = Some(RADIO_ID_NAME.to_owned());
}

fn read_contacts(image: &Image, codeplug: &mut Codeplug) -> SlotNames {
    let mut names = UniqueNames::default();
    let mut slots = SlotNames::default();
    for (slot, record) in CONTACTS.records(image) {
        let Some(mut contact) = records::contact(record) else {
            continue;
        };
        contact.name = names.claim(&contact.name, "Contact");
        slots.remember(slot, &contact.name);
        codeplug.contacts.push(contact);
    }
    slots
}

fn read_group_lists(image: &Image, contacts: &SlotNames, codeplug: &mut Codeplug) -> SlotNames {
    let mut names = UniqueNames::default();
    let mut slots = SlotNames::default();
    for (slot, record) in GROUP_LISTS.records(image) {
        let Some(listing) = records::group_list(record) else {
            continue;
        };
        let name = names.claim(&listing.name, "Group list");
        slots.remember(slot, &name);
        codeplug.group_lists.push(sdrmm_wire::cps::GroupList {
            name,
            contacts: resolve(&listing.members, contacts),
        });
    }
    slots
}

fn read_channels(image: &Image, links: &Links, codeplug: &mut Codeplug) -> SlotNames {
    let mut names = UniqueNames::default();
    let mut slots = SlotNames::default();
    for (slot, record) in CHANNELS.records(image) {
        let Some(Decoded {
            mut channel,
            own_id,
        }) = channel::decode(record, links)
        else {
            continue;
        };
        if let ChannelMode::Dmr(dmr) = &mut channel.mode {
            dmr.radio_id = own_id.and_then(|number| own_radio_id(codeplug, number));
        }
        channel.name = names.claim(&channel.name, "Channel");
        slots.remember(slot, &channel.name);
        codeplug.channels.push(channel);
    }
    slots
}

fn own_radio_id(codeplug: &mut Codeplug, number: u32) -> Option<String> {
    if number == 0 || default_radio_number(codeplug) == Some(number) {
        return None;
    }
    if let Some(known) = codeplug.radio_ids.iter().find(|id| id.number == number) {
        return Some(known.name.clone());
    }
    let name = format!("ID {number}");
    codeplug.radio_ids.push(RadioId {
        name: name.clone(),
        number,
    });
    Some(name)
}

fn read_zones(image: &Image, channels: &SlotNames, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for (_, record) in ZONES.records(image) {
        let Some(listing) = records::zone(record) else {
            continue;
        };
        codeplug.zones.push(sdrmm_wire::cps::Zone {
            name: names.claim(&listing.name, "Zone"),
            channels_a: resolve(&listing.members, channels),
            channels_b: Vec::new(),
        });
    }
}

fn resolve(members: &[u16], slots: &SlotNames) -> Vec<String> {
    members
        .iter()
        .filter_map(|member| slots.name(usize::from(*member)))
        .collect()
}

fn default_radio_number(codeplug: &Codeplug) -> Option<u32> {
    codeplug
        .settings
        .default_radio_id
        .as_deref()
        .and_then(|name| codeplug.radio_ids.iter().find(|id| id.name == name))
        .or_else(|| codeplug.radio_ids.first())
        .map(|id| id.number)
}

fn write_radio_id(image: &mut Image, number: Option<u32>) -> Result<(), CpsError> {
    records::put_radio_id(SETTINGS.whole_mut(image)?, number);
    Ok(())
}

fn slot_of(names: &NameList, name: &str) -> Option<u16> {
    names
        .index_of(name)
        .and_then(|index| u16::try_from(index).ok())
}

fn listings<T>(
    items: &[T],
    members: &NameList,
    parts: impl Fn(&T) -> (&String, &Vec<String>),
) -> Vec<Listing> {
    items
        .iter()
        .map(|item| {
            let (name, wanted) = parts(item);
            Listing {
                name: name.clone(),
                members: wanted
                    .iter()
                    .filter_map(|member| slot_of(members, member))
                    .collect(),
            }
        })
        .collect()
}

fn targets(
    item: &sdrmm_wire::cps::Channel,
    catalog: &Catalog,
    codeplug: &Codeplug,
    default_id: Option<u32>,
) -> Targets {
    let ChannelMode::Dmr(dmr) = &item.mode else {
        return Targets {
            contact: None,
            group_list: None,
            own_id: None,
        };
    };
    let own_id = dmr
        .radio_id
        .as_deref()
        .and_then(|name| codeplug.radio_ids.iter().find(|id| id.name == name))
        .map(|id| id.number)
        .filter(|number| *number != 0 && Some(*number) != default_id);
    Targets {
        contact: dmr
            .contact
            .as_deref()
            .and_then(|name| slot_of(&catalog.contacts, name)),
        group_list: dmr
            .group_list
            .as_deref()
            .and_then(|name| slot_of(&catalog.group_lists, name))
            .and_then(|slot| u8::try_from(slot).ok()),
        own_id,
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::{
        Channel, Contact, ContactKind, DmrChannel, FmChannel, GroupList, TimeSlot, Zone,
    };

    use super::*;

    fn contact(name: &str, number: u32) -> Contact {
        Contact {
            name: name.to_owned(),
            kind: ContactKind::Group,
            number,
            ring: false,
        }
    }

    fn dmr(name: &str, contact: &str, radio_id: Option<&str>) -> Channel {
        Channel {
            name: name.to_owned(),
            rx_hz: 439_000_000,
            tx_hz: 431_400_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Dmr(DmrChannel {
                color_code: 1,
                time_slot: TimeSlot::One,
                contact: Some(contact.to_owned()),
                group_list: Some("Local".to_owned()),
                radio_id: radio_id.map(str::to_owned),
                admit: sdrmm_wire::cps::Admit::Always,
            }),
        }
    }

    fn sample() -> Codeplug {
        let mut codeplug = Codeplug::empty();
        codeplug.radio_ids = vec![RadioId {
            name: "Me".to_owned(),
            number: 2_628_001,
        }];
        codeplug.contacts = vec![contact("TG1", 1), contact("TG2", 2)];
        codeplug.group_lists = vec![GroupList {
            name: "Local".to_owned(),
            contacts: vec!["TG2".to_owned(), "TG1".to_owned()],
        }];
        codeplug.channels = vec![
            dmr("One", "TG2", Some("Me")),
            Channel {
                name: "Two".to_owned(),
                rx_hz: 145_500_000,
                tx_hz: 145_500_000,
                mode: ChannelMode::Fm(FmChannel::default()),
                ..Channel::default()
            },
        ];
        codeplug.zones = vec![Zone {
            name: "Home".to_owned(),
            channels_a: vec!["Two".to_owned(), "One".to_owned()],
            channels_b: Vec::new(),
        }];
        codeplug
    }

    #[test]
    fn a_codeplug_survives_the_image() {
        let model = Rt4D;
        let mut image = model.blank_image();
        model.encode(&sample(), &mut image).expect("encode");
        let decoded = model.decode(&image).expect("decode");
        let source = sample();
        assert_eq!(decoded.radio_ids[0].number, 2_628_001);
        assert_eq!(
            decoded.settings.default_radio_id.as_deref(),
            Some("Radio ID")
        );
        assert_eq!(decoded.contacts, source.contacts);
        assert_eq!(decoded.group_lists, source.group_lists);
        assert_eq!(decoded.zones, source.zones);
        let ChannelMode::Dmr(first) = &decoded.channels[0].mode else {
            panic!("the first channel is DMR");
        };
        assert_eq!(first.contact.as_deref(), Some("TG2"));
        assert_eq!(first.group_list.as_deref(), Some("Local"));
        assert_eq!(first.radio_id, None);
        let record = CHANNELS.record(&image, 0).expect("slot");
        assert_eq!(
            record[0] & 0x08,
            0,
            "the default ID is not written per channel"
        );
    }

    #[test]
    fn a_foreign_per_channel_id_decodes_as_its_own_radio_id() {
        let model = Rt4D;
        let mut image = model.blank_image();
        let mut source = sample();
        source.radio_ids.push(RadioId {
            name: "Club".to_owned(),
            number: 2_628_999,
        });
        model.encode(&source, &mut image).expect("encode");
        let record = CHANNELS.whole_mut(&mut image).expect("channels");
        record[0] |= 0x08;
        crate::bits::set_bcd8_le(record, 0x16, 2_628_999);
        let decoded = model.decode(&image).expect("decode");
        let ChannelMode::Dmr(first) = &decoded.channels[0].mode else {
            panic!("the first channel is DMR");
        };
        assert_eq!(first.radio_id.as_deref(), Some("ID 2628999"));
        assert_eq!(decoded.radio_ids.len(), 2);
    }

    #[test]
    fn references_follow_slots_across_gaps() {
        let model = Rt4D;
        let mut image = model.blank_image();
        model.encode(&sample(), &mut image).expect("encode");
        let contacts = CONTACTS.whole_mut(&mut image).expect("contacts");
        contacts[..0x15].fill(0xff);
        let decoded = model.decode(&image).expect("decode");
        assert_eq!(decoded.contacts, vec![contact("TG2", 2)]);
        let ChannelMode::Dmr(first) = &decoded.channels[0].mode else {
            panic!("the first channel is DMR");
        };
        assert_eq!(first.contact.as_deref(), Some("TG2"));
    }

    #[test]
    fn slots_no_longer_used_are_erased() {
        let model = Rt4D;
        let mut image = model.blank_image();
        model.encode(&sample(), &mut image).expect("encode");
        let mut smaller = sample();
        smaller.channels.truncate(1);
        smaller.zones.clear();
        model.encode(&smaller, &mut image).expect("encode");
        assert!(super::super::layout::is_vacant(
            CHANNELS.record(&image, 1).expect("slot")
        ));
        assert!(super::super::layout::is_vacant(
            ZONES.record(&image, 0).expect("slot")
        ));
    }

    #[test]
    fn the_descriptor_names_the_radio() {
        let descriptor = Rt4D.descriptor();
        assert_eq!(descriptor.id, MODEL_ID);
        assert_eq!(descriptor.model, "RT-4D");
        assert!(descriptor.usb.is_empty());
        assert_eq!(Rt4D.erased_byte(), 0xff);
        assert_eq!(
            descriptor.transfer_bytes,
            0x400 + 1024 * 0x30 + 250 * 0x208 + 10_000 * 0x15 + 250 * 0x50_u64
        );
    }
}
