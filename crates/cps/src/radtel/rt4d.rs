use std::sync::OnceLock;

use sdrmm_wire::cps::{
    Admit, Bandwidth, Channel, ChannelKind, ChannelMode, Codeplug, CodeplugMeta, Contact,
    ContactKind, ConversionReport, DmrChannel, FmChannel, FrequencyRange, GroupList, Power,
    RadioFeatures, RadioId, RadioLimits, RadioModelDescriptor, TimeSlot, Tone, Zone,
};

use super::protocol::Rt4DSession;
use crate::{
    CpsError, Image, RadioModel, RadioSession, Region, SerialLink,
    bits::{
        get_bcd8_le, get_bit, get_bits, get_u8, get_u16_le, get_u32_le, is_blank, is_erased,
        read_ascii, set_bcd8_le, set_bit, set_bits, set_u8, set_u16_le, set_u32_le, write_ascii,
    },
    catalog::{Catalog, UniqueNames},
    convert::fit,
    tones::{dcs_from_binary, dcs_to_binary},
};

pub const MODEL_ID: &str = "radtel-rt4d";

const FIRST_SETTINGS: u32 = 0x0000_2000;
const FIRST_SETTINGS_LEN: u32 = 0x0000_0400;
const RADIO_DMR_ID_AT: usize = 0x0180;

const CHANNELS: u32 = 0x0000_4000;
const CHANNEL_SIZE: u32 = 0x0030;
const NUM_CHANNELS: u32 = 1024;

const ZONES: u32 = 0x0001_e000;
const ZONE_SIZE: u32 = 0x0208;
const NUM_ZONES: u32 = 250;
const CHANNELS_PER_ZONE: u32 = 200;
const ZONE_NAME_AT: usize = 0x0004;
const ZONE_CHANNELS_AT: usize = 0x0014;

const CONTACTS: u32 = 0x0005_e000;
const CONTACT_SIZE: u32 = 0x15;
const NUM_CONTACTS: u32 = 10_000;

const GROUP_LISTS: u32 = 0x000c_6000;
const GROUP_LIST_SIZE: u32 = 0x50;
const NUM_GROUP_LISTS: u32 = 250;
const GROUP_LIST_MEMBERS: u32 = 32;
const GROUP_NAME_LEN: usize = 14;
const GROUP_MEMBERS_AT: usize = 0x0010;

const NAME_LEN: usize = 16;
const EOS: u8 = 0xff;
const NO_INDEX16: u16 = 0xffff;
const ALL_CALL_RAW: u32 = 0xaaaa_aaaa;

const PROMISCUOUS_BIT: u8 = 0;
const TIME_SLOT_BIT: u8 = 1;
const DMR_ID_SOURCE_BIT: u8 = 3;
const TRX_MODE_BIT: u8 = 4;
const CHANNEL_TYPE_BIT: u8 = 6;
const COLOR_CODE_BIT: u8 = 4;
const POWER_BIT: u8 = 6;
const FM_ADMIT_BIT: u8 = 3;
const DMR_ADMIT_BIT: u8 = 5;
const BANDWIDTH_BIT: u8 = 6;

const RX_FREQUENCY_AT: usize = 0x0005;
const TX_FREQUENCY_AT: usize = 0x0009;
const RX_SUBTONE_AT: usize = 0x000d;
const TX_SUBTONE_AT: usize = 0x000f;
const CONTACT_INDEX_AT: usize = 0x0011;
const GROUP_LIST_INDEX_AT: usize = 0x0014;
const CHANNEL_DMR_ID_AT: usize = 0x0016;
const CHANNEL_NAME_AT: usize = 0x0020;

pub struct Rt4D;

fn regions() -> &'static [Region] {
    static ALL: OnceLock<Vec<Region>> = OnceLock::new();
    ALL.get_or_init(|| {
        vec![
            Region::fixed("settings", FIRST_SETTINGS, FIRST_SETTINGS_LEN),
            Region::fixed("channels", CHANNELS, NUM_CHANNELS * CHANNEL_SIZE),
            Region::fixed("zones", ZONES, NUM_ZONES * ZONE_SIZE),
            Region::fixed("contacts", CONTACTS, NUM_CONTACTS * CONTACT_SIZE),
            Region::fixed(
                "group lists",
                GROUP_LISTS,
                NUM_GROUP_LISTS * GROUP_LIST_SIZE,
            ),
        ]
    })
}

#[must_use]
pub fn limits() -> RadioLimits {
    RadioLimits {
        channels: NUM_CHANNELS,
        contacts: NUM_CONTACTS,
        group_lists: NUM_GROUP_LISTS,
        group_list_members: GROUP_LIST_MEMBERS,
        zones: NUM_ZONES,
        zone_channels: CHANNELS_PER_ZONE,
        scan_lists: 0,
        scan_list_members: 0,
        radio_ids: 1,
        channel_name_len: NAME_LEN as u32,
        contact_name_len: NAME_LEN as u32,
        group_list_name_len: GROUP_NAME_LEN as u32,
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
            dual_zone_lists: false,
            per_channel_radio_id: true,
            scan_lists: false,
            group_lists: true,
            dcs_tones: true,
            talkaround: false,
            named_radio_ids: false,
        },
    }
}

impl RadioModel for Rt4D {
    fn descriptor(&self) -> RadioModelDescriptor {
        RadioModelDescriptor {
            id: MODEL_ID.to_owned(),
            manufacturer: "Radtel".to_owned(),
            model: "RT-4D".to_owned(),
            family: "radtel-rt4d".to_owned(),
            usb: Vec::new(),
            needs_explicit_selection: true,
            transfer_bytes: self.transfer_bytes(),
            limits: limits(),
        }
    }

    fn regions(&self) -> &'static [Region] {
        regions()
    }

    fn erased_byte(&self) -> u8 {
        0xff
    }

    fn open(&self, link: Box<dyn SerialLink>) -> Result<Box<dyn RadioSession>, CpsError> {
        Ok(Box::new(Rt4DSession::open(link, MODEL_ID)?))
    }

    fn decode(&self, image: &Image) -> Result<Codeplug, CpsError> {
        let mut codeplug = Codeplug::empty();
        codeplug.meta = CodeplugMeta {
            source_model: Some(MODEL_ID.to_owned()),
            ..CodeplugMeta::default()
        };
        decode_radio_id(image, &mut codeplug);
        decode_contacts(image, &mut codeplug);
        decode_group_lists(image, &Catalog::of(&codeplug), &mut codeplug);
        decode_channels(image, &Catalog::of(&codeplug), &mut codeplug);
        decode_zones(image, &Catalog::of(&codeplug), &mut codeplug);
        Ok(codeplug)
    }

    fn encode(&self, codeplug: &Codeplug, image: &mut Image) -> Result<ConversionReport, CpsError> {
        let (fitted, report) = fit(codeplug, MODEL_ID, &limits());
        let catalog = Catalog::of(&fitted);
        encode_radio_id(image, &fitted)?;
        encode_contacts(image, &fitted)?;
        encode_group_lists(image, &catalog, &fitted)?;
        encode_channels(image, &catalog, &fitted)?;
        encode_zones(image, &catalog, &fitted)?;
        Ok(report)
    }
}

fn slot(image: &Image, addr: u32, len: u32) -> Option<&[u8]> {
    image.get(addr, len as usize)
}

fn prepare(data: &mut [u8]) {
    if is_erased(data) {
        data.fill(0);
    }
}

fn clear(data: &mut [u8]) {
    if !is_blank(data) {
        data.fill(EOS);
    }
}

fn slot_mut(image: &mut Image, addr: u32, len: u32) -> Result<&mut [u8], CpsError> {
    image.allocate(addr, len, EOS);
    image
        .get_mut(addr, len as usize)
        .ok_or(CpsError::MissingRegion {
            addr,
            len: len as usize,
        })
}

fn decode_radio_id(image: &Image, codeplug: &mut Codeplug) {
    let Some(data) = slot(image, FIRST_SETTINGS, FIRST_SETTINGS_LEN) else {
        return;
    };
    let number = get_bcd8_le(data, RADIO_DMR_ID_AT);
    if number == 0 || number > 0x00ff_ffff {
        return;
    }
    codeplug.radio_ids.push(RadioId {
        name: "Radio ID".to_owned(),
        number,
    });
    codeplug.settings.default_radio_id = Some("Radio ID".to_owned());
}

fn encode_radio_id(image: &mut Image, codeplug: &Codeplug) -> Result<(), CpsError> {
    let number = codeplug
        .settings
        .default_radio_id
        .as_deref()
        .and_then(|name| codeplug.radio_ids.iter().find(|id| id.name == name))
        .or_else(|| codeplug.radio_ids.first())
        .map(|id| id.number);
    let Some(number) = number else {
        return Ok(());
    };
    let data = slot_mut(image, FIRST_SETTINGS, FIRST_SETTINGS_LEN)?;
    set_bcd8_le(data, RADIO_DMR_ID_AT, number);
    Ok(())
}

fn read_subtone(data: &[u8], at: usize) -> Option<Tone> {
    let raw = get_u16_le(data, at);
    let code = raw & 0x0fff;
    match raw >> 12 {
        1 => Some(Tone::Ctcss { decihertz: code }),
        2 => Some(Tone::Dcs {
            code: dcs_from_binary(code),
            inverted: false,
        }),
        3 => Some(Tone::Dcs {
            code: dcs_from_binary(code),
            inverted: true,
        }),
        _ => None,
    }
}

fn write_subtone(data: &mut [u8], at: usize, tone: Option<Tone>) {
    let raw = match tone {
        None => 0x0fff,
        Some(Tone::Ctcss { decihertz }) => (1 << 12) | (decihertz & 0x0fff),
        Some(Tone::Dcs { code, inverted }) => {
            let kind: u16 = if inverted { 3 } else { 2 };
            (kind << 12) | (dcs_to_binary(code) & 0x0fff)
        }
    };
    set_u16_le(data, at, raw);
}

fn fm_admit_from(raw: u8) -> Admit {
    match raw {
        1 => Admit::ChannelFree,
        2 => Admit::ToneFree,
        _ => Admit::Always,
    }
}

fn fm_admit_to(admit: Admit) -> u8 {
    match admit {
        Admit::Always => 0,
        Admit::ToneFree => 2,
        _ => 1,
    }
}

fn dmr_admit_from(raw: u8) -> Admit {
    match raw {
        1 => Admit::ChannelFree,
        2 => Admit::ColorCodeFree,
        _ => Admit::Always,
    }
}

fn dmr_admit_to(admit: Admit) -> u8 {
    match admit {
        Admit::Always => 0,
        Admit::ColorCodeFree | Admit::DifferentColorCode => 2,
        _ => 1,
    }
}

fn decode_channels(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_CHANNELS {
        let Some(data) = slot(image, CHANNELS + index * CHANNEL_SIZE, CHANNEL_SIZE) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = read_ascii(data, CHANNEL_NAME_AT, NAME_LEN, EOS);
        if name.is_empty() {
            continue;
        }
        let analogue = get_bit(data, 0, CHANNEL_TYPE_BIT);
        let mode = if analogue {
            ChannelMode::Fm(FmChannel {
                bandwidth: if get_bit(data, 4, BANDWIDTH_BIT) {
                    Bandwidth::Narrow
                } else {
                    Bandwidth::Wide
                },
                rx_tone: read_subtone(data, RX_SUBTONE_AT),
                tx_tone: read_subtone(data, TX_SUBTONE_AT),
                squelch: None,
                admit: fm_admit_from(get_bits(data, 3, FM_ADMIT_BIT, 2)),
            })
        } else {
            let group_index = get_u8(data, GROUP_LIST_INDEX_AT);
            ChannelMode::Dmr(DmrChannel {
                color_code: get_bits(data, 1, COLOR_CODE_BIT, 4),
                time_slot: if get_bit(data, 0, TIME_SLOT_BIT) {
                    TimeSlot::Two
                } else {
                    TimeSlot::One
                },
                contact: catalog
                    .contacts
                    .name_at(u32::from(get_u16_le(data, CONTACT_INDEX_AT)))
                    .map(str::to_owned),
                group_list: (group_index != 0)
                    .then(|| catalog.group_lists.name_at(u32::from(group_index) - 1))
                    .flatten()
                    .map(str::to_owned),
                radio_id: get_bit(data, 0, DMR_ID_SOURCE_BIT)
                    .then(|| get_bcd8_le(data, CHANNEL_DMR_ID_AT))
                    .filter(|number| *number != 0)
                    .and_then(|number| {
                        codeplug
                            .radio_ids
                            .iter()
                            .find(|id| id.number == number)
                            .map(|id| id.name.clone())
                    }),
                admit: dmr_admit_from(get_bits(data, 3, DMR_ADMIT_BIT, 2)),
            })
        };
        codeplug.channels.push(Channel {
            name: names.claim(&name, "Channel"),
            rx_hz: u64::from(get_u32_le(data, RX_FREQUENCY_AT)) * 10,
            tx_hz: u64::from(get_u32_le(data, TX_FREQUENCY_AT)) * 10,
            power: if get_bit(data, 2, POWER_BIT) {
                Power::High
            } else {
                Power::Low
            },
            rx_only: get_bits(data, 0, TRX_MODE_BIT, 2) == 1,
            timeout_s: None,
            scan_list: None,
            mode,
        });
    }
}

fn encode_channels(
    image: &mut Image,
    catalog: &Catalog,
    codeplug: &Codeplug,
) -> Result<(), CpsError> {
    for index in 0..NUM_CHANNELS {
        let item = codeplug.channels.get(index as usize).cloned();
        let data = slot_mut(image, CHANNELS + index * CHANNEL_SIZE, CHANNEL_SIZE)?;
        let Some(item) = item else {
            clear(data);
            continue;
        };
        prepare(data);
        set_bit(data, 0, PROMISCUOUS_BIT, false);
        set_bits(data, 0, TRX_MODE_BIT, 2, u8::from(item.rx_only));
        set_bit(data, 2, POWER_BIT, item.power.rank() >= Power::Mid.rank());
        set_u32_le(data, RX_FREQUENCY_AT, (item.rx_hz / 10) as u32);
        set_u32_le(data, TX_FREQUENCY_AT, (item.tx_hz / 10) as u32);
        set_u8(data, GROUP_LIST_INDEX_AT, 0);
        write_subtone(data, RX_SUBTONE_AT, None);
        write_subtone(data, TX_SUBTONE_AT, None);
        match &item.mode {
            ChannelMode::Fm(fm) => {
                set_bit(data, 0, CHANNEL_TYPE_BIT, true);
                set_bit(
                    data,
                    4,
                    BANDWIDTH_BIT,
                    matches!(fm.bandwidth, Bandwidth::Narrow),
                );
                write_subtone(data, RX_SUBTONE_AT, fm.rx_tone);
                write_subtone(data, TX_SUBTONE_AT, fm.tx_tone);
                set_bits(data, 3, FM_ADMIT_BIT, 2, fm_admit_to(fm.admit));
            }
            ChannelMode::Dmr(dmr) => {
                set_bit(data, 0, CHANNEL_TYPE_BIT, false);
                set_bit(data, 4, BANDWIDTH_BIT, true);
                set_bits(data, 1, COLOR_CODE_BIT, 4, dmr.color_code.min(15));
                set_bit(
                    data,
                    0,
                    TIME_SLOT_BIT,
                    matches!(dmr.time_slot, TimeSlot::Two),
                );
                set_bits(data, 3, DMR_ADMIT_BIT, 2, dmr_admit_to(dmr.admit));
                set_u16_le(
                    data,
                    CONTACT_INDEX_AT,
                    dmr.contact
                        .as_deref()
                        .and_then(|name| catalog.contacts.index_of(name))
                        .and_then(|at| u16::try_from(at).ok())
                        .unwrap_or(0),
                );
                if let Some(at) = dmr
                    .group_list
                    .as_deref()
                    .and_then(|name| catalog.group_lists.index_of(name))
                    .and_then(|at| u8::try_from(at + 1).ok())
                {
                    set_u8(data, GROUP_LIST_INDEX_AT, at);
                }
                let channel_id = dmr
                    .radio_id
                    .as_deref()
                    .and_then(|name| codeplug.radio_ids.iter().find(|id| id.name == name))
                    .map(|id| id.number)
                    .filter(|number| {
                        codeplug.settings.default_radio_id.as_deref() != dmr.radio_id.as_deref()
                            && *number != 0
                    });
                set_bit(data, 0, DMR_ID_SOURCE_BIT, channel_id.is_some());
                set_bcd8_le(data, CHANNEL_DMR_ID_AT, channel_id.unwrap_or(0));
            }
        }
        write_ascii(data, CHANNEL_NAME_AT, &item.name, NAME_LEN, EOS);
    }
    Ok(())
}

fn decode_contacts(image: &Image, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_CONTACTS {
        let Some(data) = slot(image, CONTACTS + index * CONTACT_SIZE, CONTACT_SIZE) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = read_ascii(data, 0x0005, NAME_LEN, EOS);
        if name.is_empty() {
            continue;
        }
        let all_call = get_u32_le(data, 0x0001) == ALL_CALL_RAW;
        let kind = match get_u8(data, 0) {
            1 => ContactKind::Group,
            2 => ContactKind::All,
            _ => ContactKind::Private,
        };
        codeplug.contacts.push(Contact {
            name: names.claim(&name, "Contact"),
            kind,
            number: if all_call {
                sdrmm_wire::cps::ALL_CALL_NUMBER
            } else {
                get_bcd8_le(data, 0x0001)
            },
            ring: false,
        });
    }
}

fn encode_contacts(image: &mut Image, codeplug: &Codeplug) -> Result<(), CpsError> {
    for index in 0..NUM_CONTACTS {
        let contact = codeplug.contacts.get(index as usize).cloned();
        let data = slot_mut(image, CONTACTS + index * CONTACT_SIZE, CONTACT_SIZE)?;
        let Some(contact) = contact else {
            clear(data);
            continue;
        };
        prepare(data);
        set_u8(
            data,
            0,
            match contact.kind {
                ContactKind::Private => 0,
                ContactKind::Group => 1,
                ContactKind::All => 2,
            },
        );
        if matches!(contact.kind, ContactKind::All) {
            set_u32_le(data, 0x0001, ALL_CALL_RAW);
        } else {
            set_bcd8_le(data, 0x0001, contact.number);
        }
        write_ascii(data, 0x0005, &contact.name, NAME_LEN, EOS);
    }
    Ok(())
}

fn decode_group_lists(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_GROUP_LISTS {
        let Some(data) = slot(
            image,
            GROUP_LISTS + index * GROUP_LIST_SIZE,
            GROUP_LIST_SIZE,
        ) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = read_ascii(data, 0, GROUP_NAME_LEN, EOS);
        if name.is_empty() {
            continue;
        }
        let contacts = (0..GROUP_LIST_MEMBERS)
            .map(|member| get_u16_le(data, GROUP_MEMBERS_AT + member as usize * 2))
            .filter(|member| *member != NO_INDEX16)
            .filter_map(|member| catalog.contacts.name_at(u32::from(member)))
            .map(str::to_owned)
            .collect();
        codeplug.group_lists.push(GroupList {
            name: names.claim(&name, "Group list"),
            contacts,
        });
    }
}

fn encode_group_lists(
    image: &mut Image,
    catalog: &Catalog,
    codeplug: &Codeplug,
) -> Result<(), CpsError> {
    for index in 0..NUM_GROUP_LISTS {
        let list = codeplug.group_lists.get(index as usize).cloned();
        let data = slot_mut(
            image,
            GROUP_LISTS + index * GROUP_LIST_SIZE,
            GROUP_LIST_SIZE,
        )?;
        let Some(list) = list else {
            clear(data);
            continue;
        };
        prepare(data);
        data[GROUP_MEMBERS_AT..].fill(0xff);
        write_ascii(data, 0, &list.name, GROUP_NAME_LEN, EOS);
        for (position, contact) in list
            .contacts
            .iter()
            .take(GROUP_LIST_MEMBERS as usize)
            .enumerate()
        {
            if let Some(at) = catalog
                .contacts
                .index_of(contact)
                .and_then(|at| u16::try_from(at).ok())
            {
                set_u16_le(data, GROUP_MEMBERS_AT + position * 2, at);
            }
        }
    }
    Ok(())
}

fn decode_zones(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_ZONES {
        let Some(data) = slot(image, ZONES + index * ZONE_SIZE, ZONE_SIZE) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = read_ascii(data, ZONE_NAME_AT, NAME_LEN, EOS);
        if name.is_empty() {
            continue;
        }
        let channels = (0..CHANNELS_PER_ZONE)
            .map(|member| get_u16_le(data, ZONE_CHANNELS_AT + member as usize * 2))
            .filter(|member| *member != NO_INDEX16)
            .filter_map(|member| catalog.channels.name_at(u32::from(member)))
            .map(str::to_owned)
            .collect();
        codeplug.zones.push(Zone {
            name: names.claim(&name, "Zone"),
            channels_a: channels,
            channels_b: Vec::new(),
        });
    }
}

fn encode_zones(image: &mut Image, catalog: &Catalog, codeplug: &Codeplug) -> Result<(), CpsError> {
    for index in 0..NUM_ZONES {
        let zone = codeplug.zones.get(index as usize).cloned();
        let data = slot_mut(image, ZONES + index * ZONE_SIZE, ZONE_SIZE)?;
        let Some(zone) = zone else {
            clear(data);
            continue;
        };
        prepare(data);
        data[ZONE_CHANNELS_AT..].fill(0xff);
        set_u16_le(data, 0, 0);
        set_u16_le(data, 0x0002, 0);
        write_ascii(data, ZONE_NAME_AT, &zone.name, NAME_LEN, EOS);
        for (position, name) in zone
            .channels_a
            .iter()
            .take(CHANNELS_PER_ZONE as usize)
            .enumerate()
        {
            if let Some(at) = catalog
                .channels
                .index_of(name)
                .and_then(|at| u16::try_from(at).ok())
            {
                set_u16_le(data, ZONE_CHANNELS_AT + position * 2, at);
            }
        }
    }
    Ok(())
}
