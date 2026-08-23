use std::sync::OnceLock;

use sdrmm_wire::cps::{
    Bandwidth, ChannelKind, ChannelMode, Codeplug, CodeplugMeta, Contact, ContactKind,
    ConversionIssue, ConversionReport, FrequencyRange, GroupList, IssueScope, IssueSeverity, Power,
    RadioFeatures, RadioId, RadioLimits, RadioModelDescriptor, ScanList, ScanRevert, ScanTarget,
    UsbMatch, Zone,
};

use super::{channel, protocol::AnytoneSession};
use crate::{
    CpsError, Image, RadioModel, RadioSession, Region, SerialLink,
    bits::{
        get_bcd8_be, get_u8, get_u16_le, is_blank, is_erased, read_utf16, set_bcd8_be, set_u8,
        set_u16_le, write_utf16,
    },
    catalog::{Catalog, UniqueNames},
    convert::fit,
};

pub const MODEL_ID: &str = "anytone-d890uv";
const ACCEPTED: &[&str] = &["D890UV", "890UV"];

const CHANNEL_BANKS: u32 = 0x0100_0000;
const BETWEEN_CHANNEL_BANKS: u32 = 0x0008_0000;
const CHANNELS_PER_BANK: u32 = 128;
const NUM_CHANNELS: u32 = 4096;
const CHANNEL_SIZE: u32 = channel::CHANNEL_SIZE as u32;

const ZONE_CHANNELS: u32 = 0x0200_0000;
const BETWEEN_ZONES: u32 = 0x0000_0200;
const ZONE_NAMES: u32 = 0x0360_0000;
const BETWEEN_ZONE_NAMES: u32 = 0x0000_0040;
const NUM_ZONES: u32 = 250;
const CHANNELS_PER_ZONE: u32 = 250;

const SCAN_LISTS: u32 = 0x0210_0000;
const BETWEEN_SCAN_LISTS: u32 = 0x0000_0200;
const NUM_SCAN_LISTS: u32 = 250;
const SCAN_LIST_MEMBERS: u32 = 100;
const SCAN_MEMBERS_AT: usize = 0x0030;
const SCAN_NAME_AT: usize = 0x000e;
const SCAN_REVERT_AT: usize = 0x00f8;

const RADIO_IDS: u32 = 0x0368_0000;
const RADIO_ID_SIZE: u32 = 0x0000_0040;
const NUM_RADIO_IDS: u32 = 250;
const PRIMARY_RADIO_ID: u32 = 0x0368_4000;
const PRIMARY_NAME_KEY: &str = "primary_radio_id_name";

const GROUP_LISTS: u32 = 0x0378_0000;
const BETWEEN_GROUP_LISTS: u32 = 0x0000_0200;
const NUM_GROUP_LISTS: u32 = 250;
const GROUP_LIST_MEMBERS: u32 = 64;
const GROUP_NAME_AT: usize = 0x0100;

const CONTACTS: u32 = 0x03a0_0000;
const CONTACT_SIZE: u32 = 0x0000_00c8;
const NUM_CONTACTS: u32 = 10_000;

const NAME_UNITS: usize = 16;
const NO_INDEX16: u16 = 0xffff;
const EMPTY_RUN: u32 = 8;

pub struct D890Uv;

fn channel_address(index: u32) -> u32 {
    CHANNEL_BANKS
        + (index / CHANNELS_PER_BANK) * BETWEEN_CHANNEL_BANKS
        + (index % CHANNELS_PER_BANK) * CHANNEL_SIZE
}

fn regions() -> &'static [Region] {
    static ALL: OnceLock<Vec<Region>> = OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = vec![
            Region::sparse(
                "radio IDs",
                RADIO_IDS,
                NUM_RADIO_IDS * RADIO_ID_SIZE,
                RADIO_ID_SIZE,
                EMPTY_RUN,
            ),
            Region::fixed("primary radio ID", PRIMARY_RADIO_ID, RADIO_ID_SIZE),
            Region::sparse(
                "contacts",
                CONTACTS,
                NUM_CONTACTS * CONTACT_SIZE,
                CONTACT_SIZE,
                EMPTY_RUN,
            ),
            Region::sparse(
                "group lists",
                GROUP_LISTS,
                NUM_GROUP_LISTS * BETWEEN_GROUP_LISTS,
                BETWEEN_GROUP_LISTS,
                EMPTY_RUN,
            ),
            Region::sparse(
                "zone names",
                ZONE_NAMES,
                NUM_ZONES * BETWEEN_ZONE_NAMES,
                BETWEEN_ZONE_NAMES,
                EMPTY_RUN,
            ),
            Region::sparse(
                "zone channels",
                ZONE_CHANNELS,
                NUM_ZONES * BETWEEN_ZONES,
                BETWEEN_ZONES,
                EMPTY_RUN,
            ),
            Region::sparse(
                "scan lists",
                SCAN_LISTS,
                NUM_SCAN_LISTS * BETWEEN_SCAN_LISTS,
                BETWEEN_SCAN_LISTS,
                EMPTY_RUN,
            ),
        ];
        all.extend((0..NUM_CHANNELS / CHANNELS_PER_BANK).map(|bank| {
            Region::sparse(
                "channels",
                CHANNEL_BANKS + bank * BETWEEN_CHANNEL_BANKS,
                CHANNELS_PER_BANK * CHANNEL_SIZE,
                CHANNEL_SIZE,
                EMPTY_RUN,
            )
        }));
        all
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
        scan_lists: NUM_SCAN_LISTS,
        scan_list_members: SCAN_LIST_MEMBERS,
        radio_ids: NUM_RADIO_IDS,
        channel_name_len: NAME_UNITS as u32,
        contact_name_len: NAME_UNITS as u32,
        group_list_name_len: NAME_UNITS as u32,
        zone_name_len: NAME_UNITS as u32,
        scan_list_name_len: NAME_UNITS as u32,
        radio_id_name_len: NAME_UNITS as u32,
        rx_ranges: vec![
            FrequencyRange::new(136_000_000, 174_000_000),
            FrequencyRange::new(400_000_000, 480_000_000),
        ],
        tx_ranges: vec![
            FrequencyRange::new(136_000_000, 174_000_000),
            FrequencyRange::new(400_000_000, 480_000_000),
        ],
        powers: vec![Power::Low, Power::Mid, Power::High, Power::Max],
        modes: vec![ChannelKind::Fm, ChannelKind::Dmr],
        frequency_step_hz: 10,
        features: RadioFeatures {
            dual_zone_lists: false,
            per_channel_radio_id: true,
            scan_lists: true,
            group_lists: true,
            dcs_tones: true,
            talkaround: true,
            named_radio_ids: true,
        },
    }
}

impl RadioModel for D890Uv {
    fn erased_byte(&self) -> u8 {
        ERASED
    }

    fn descriptor(&self) -> RadioModelDescriptor {
        RadioModelDescriptor {
            id: MODEL_ID.to_owned(),
            manufacturer: "AnyTone".to_owned(),
            model: "AT-D890UV".to_owned(),
            family: "anytone-gen2".to_owned(),
            usb: vec![UsbMatch {
                vid: 0x0483,
                pid: 0x5740,
            }],
            needs_explicit_selection: true,
            transfer_bytes: self.transfer_bytes(),
            limits: limits(),
        }
    }

    fn regions(&self) -> &'static [Region] {
        regions()
    }

    fn open(&self, link: Box<dyn SerialLink>) -> Result<Box<dyn RadioSession>, CpsError> {
        Ok(Box::new(AnytoneSession::open(link, MODEL_ID, ACCEPTED)?))
    }

    fn decode(&self, image: &Image) -> Result<Codeplug, CpsError> {
        let mut codeplug = Codeplug::empty();
        codeplug.meta = CodeplugMeta {
            source_model: Some(MODEL_ID.to_owned()),
            ..CodeplugMeta::default()
        };
        decode_radio_ids(image, &mut codeplug);
        decode_contacts(image, &mut codeplug);
        decode_group_lists(image, &Catalog::of(&codeplug), &mut codeplug);
        decode_scan_list_names(image, &mut codeplug);
        decode_channels(image, &Catalog::of(&codeplug), &mut codeplug);
        let catalog = Catalog::of(&codeplug);
        decode_zones(image, &catalog, &mut codeplug);
        link_scan_lists(image, &catalog, &mut codeplug);
        Ok(codeplug)
    }

    fn encode(&self, codeplug: &Codeplug, image: &mut Image) -> Result<ConversionReport, CpsError> {
        let (fitted, mut report) = fit(codeplug, MODEL_ID, &limits());
        report.issues.extend(unverified_fields(&fitted));
        let catalog = Catalog::of(&fitted);
        encode_radio_ids(image, &fitted)?;
        encode_contacts(image, &fitted)?;
        encode_group_lists(image, &catalog, &fitted)?;
        encode_zones(image, &catalog, &fitted)?;
        encode_scan_lists(image, &catalog, &fitted)?;
        encode_channels(image, &catalog, &fitted)?;
        Ok(report)
    }
}

fn unverified_fields(codeplug: &Codeplug) -> Vec<ConversionIssue> {
    let mut issues = Vec::new();
    let shifted = codeplug
        .channels
        .iter()
        .filter(|channel| channel.tx_hz != channel.rx_hz)
        .count();
    if shifted > 0 {
        issues.push(
            ConversionIssue::new(
                IssueSeverity::Note,
                IssueScope::Channel,
                "the transmit-shift direction was read off a radio that holds only simplex \
                 channels, so it is the one field here that has never been checked against \
                 hardware; confirm the split on the radio before transmitting",
            )
            .item(format!("{shifted} entries"))
            .field("tx_hz"),
        );
    }
    let wide = codeplug
        .channels
        .iter()
        .filter(|channel| {
            matches!(&channel.mode, ChannelMode::Fm(fm) if fm.bandwidth == Bandwidth::Wide)
        })
        .count();
    if wide > 0 {
        issues.push(
            ConversionIssue::new(
                IssueSeverity::Note,
                IssueScope::Channel,
                "only the narrow setting has been confirmed on this firmware; check the \
                 bandwidth on the radio",
            )
            .item(format!("{wide} entries"))
            .field("bandwidth"),
        );
    }
    issues
}

fn slot(image: &Image, addr: u32, len: u32) -> Option<&[u8]> {
    image.get(addr, len as usize)
}

fn touched(image: &Image, addr: u32, len: u32, index: usize, count: usize) -> bool {
    index < count || image.get(addr, len as usize).is_some()
}

const ERASED: u8 = 0xff;

fn prepare(data: &mut [u8]) {
    if is_erased(data) {
        data.fill(0);
    }
}

fn clear(data: &mut [u8]) {
    if !is_blank(data) {
        data.fill(ERASED);
    }
}

fn slot_mut(image: &mut Image, addr: u32, len: u32) -> Result<&mut [u8], CpsError> {
    image.allocate(addr, len, ERASED);
    image
        .get_mut(addr, len as usize)
        .ok_or(CpsError::MissingRegion {
            addr,
            len: len as usize,
        })
}

fn decode_radio_ids(image: &Image, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_RADIO_IDS {
        let Some(data) = slot(image, RADIO_IDS + index * RADIO_ID_SIZE, RADIO_ID_SIZE) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let number = get_bcd8_be(data, 0);
        let name = read_utf16(data, 0x0004, NAME_UNITS);
        if number == 0 || number > 0x00ff_ffff || name.is_empty() {
            continue;
        }
        codeplug.radio_ids.push(RadioId {
            name: names.claim(&name, "Radio ID"),
            number,
        });
    }
    let Some(data) = slot(image, PRIMARY_RADIO_ID, RADIO_ID_SIZE) else {
        return;
    };
    let number = get_bcd8_be(data, 0);
    let name = read_utf16(data, 0x0004, NAME_UNITS);
    if number == 0 || number > 0x00ff_ffff {
        return;
    }
    if !codeplug.radio_ids.iter().any(|id| id.number == number) {
        codeplug.radio_ids.push(RadioId {
            name: names.claim(&name, "Radio ID"),
            number,
        });
    }
    let matched = codeplug
        .radio_ids
        .iter()
        .find(|id| id.number == number)
        .map(|id| id.name.clone());
    if matched.as_deref() != Some(name.as_str()) && !name.is_empty() {
        codeplug.extensions.insert(
            MODEL_ID.to_owned(),
            serde_json::json!({ PRIMARY_NAME_KEY: name }),
        );
    }
    codeplug.settings.default_radio_id = matched;
}

fn encode_radio_ids(image: &mut Image, codeplug: &Codeplug) -> Result<(), CpsError> {
    for index in 0..NUM_RADIO_IDS {
        let addr = RADIO_IDS + index * RADIO_ID_SIZE;
        if !touched(
            image,
            addr,
            RADIO_ID_SIZE,
            index as usize,
            codeplug.radio_ids.len(),
        ) {
            continue;
        }
        let id = codeplug.radio_ids.get(index as usize).cloned();
        let data = slot_mut(image, addr, RADIO_ID_SIZE)?;
        let Some(id) = id else {
            clear(data);
            continue;
        };
        prepare(data);
        set_bcd8_be(data, 0, id.number);
        write_utf16(data, 0x0004, &id.name, NAME_UNITS);
    }
    let primary = codeplug
        .settings
        .default_radio_id
        .as_deref()
        .and_then(|name| codeplug.radio_ids.iter().find(|id| id.name == name))
        .or_else(|| codeplug.radio_ids.first())
        .cloned();
    let data = slot_mut(image, PRIMARY_RADIO_ID, RADIO_ID_SIZE)?;
    let Some(id) = primary else {
        clear(data);
        return Ok(());
    };
    prepare(data);
    set_bcd8_be(data, 0, id.number);
    let name = codeplug
        .extensions
        .get(MODEL_ID)
        .and_then(|extension| extension.get(PRIMARY_NAME_KEY))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(id.name.as_str());
    write_utf16(data, 0x0004, name, NAME_UNITS);
    Ok(())
}

fn contact_kind(raw: u8) -> ContactKind {
    match raw {
        1 => ContactKind::Group,
        2 => ContactKind::All,
        _ => ContactKind::Private,
    }
}

fn contact_raw(kind: ContactKind) -> u8 {
    match kind {
        ContactKind::Private => 0,
        ContactKind::Group => 1,
        ContactKind::All => 2,
    }
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
        let number = get_bcd8_be(data, 0x0002);
        let name = read_utf16(data, 0x0006, NAME_UNITS);
        if get_u8(data, 0) > 2 || number == 0 || number > 0x00ff_ffff || name.is_empty() {
            continue;
        }
        codeplug.contacts.push(Contact {
            name: names.claim(&name, "Contact"),
            kind: contact_kind(get_u8(data, 0)),
            number,
            ring: get_u8(data, 0x0001) == 2,
        });
    }
}

fn encode_contacts(image: &mut Image, codeplug: &Codeplug) -> Result<(), CpsError> {
    for index in 0..NUM_CONTACTS {
        let addr = CONTACTS + index * CONTACT_SIZE;
        if !touched(
            image,
            addr,
            CONTACT_SIZE,
            index as usize,
            codeplug.contacts.len(),
        ) {
            continue;
        }
        let contact = codeplug.contacts.get(index as usize).cloned();
        let data = slot_mut(image, addr, CONTACT_SIZE)?;
        let Some(contact) = contact else {
            clear(data);
            continue;
        };
        prepare(data);
        set_u8(data, 0, contact_raw(contact.kind));
        set_u8(data, 0x0001, u8::from(contact.ring) * 2);
        set_bcd8_be(data, 0x0002, contact.number);
        write_utf16(data, 0x0006, &contact.name, NAME_UNITS);
    }
    Ok(())
}

fn group_list_name(data: &[u8]) -> String {
    let mut units = Vec::with_capacity(NAME_UNITS);
    for index in 0..NAME_UNITS {
        let unit = get_u16_le(data, GROUP_NAME_AT + index * 4);
        if unit == 0 || unit == 0xffff {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units).trim().to_owned()
}

fn decode_group_lists(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_GROUP_LISTS {
        let Some(data) = slot(
            image,
            GROUP_LISTS + index * BETWEEN_GROUP_LISTS,
            BETWEEN_GROUP_LISTS,
        ) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = group_list_name(data);
        if name.is_empty() {
            continue;
        }
        let contacts = (0..GROUP_LIST_MEMBERS)
            .map(|member| get_u16_le(data, member as usize * 4))
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
        let addr = GROUP_LISTS + index * BETWEEN_GROUP_LISTS;
        if !touched(
            image,
            addr,
            BETWEEN_GROUP_LISTS,
            index as usize,
            codeplug.group_lists.len(),
        ) {
            continue;
        }
        let list = codeplug.group_lists.get(index as usize).cloned();
        let data = slot_mut(image, addr, BETWEEN_GROUP_LISTS)?;
        let Some(list) = list else {
            clear(data);
            continue;
        };
        prepare(data);
        for member in 0..GROUP_LIST_MEMBERS as usize {
            set_u16_le(data, member * 4, NO_INDEX16);
        }
        for (position, contact) in list
            .contacts
            .iter()
            .take(GROUP_LIST_MEMBERS as usize)
            .enumerate()
        {
            let Some(at) = catalog
                .contacts
                .index_of(contact)
                .and_then(|at| u16::try_from(at).ok())
            else {
                continue;
            };
            set_u16_le(data, position * 4, at);
            set_u16_le(data, position * 4 + 2, at);
        }
        let mut written = 0;
        for unit in list.name.encode_utf16().take(NAME_UNITS) {
            set_u16_le(data, GROUP_NAME_AT + written * 4, unit);
            set_u16_le(data, GROUP_NAME_AT + 2 + written * 4, unit);
            written += 1;
        }
        if written < NAME_UNITS {
            set_u16_le(data, GROUP_NAME_AT + written * 4, 0);
            set_u16_le(data, GROUP_NAME_AT + 2 + written * 4, 0);
        }
    }
    Ok(())
}

fn decode_zones(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_ZONES {
        let Some(name_data) = slot(
            image,
            ZONE_NAMES + index * BETWEEN_ZONE_NAMES,
            BETWEEN_ZONE_NAMES,
        ) else {
            break;
        };
        let name = read_utf16(name_data, 0, NAME_UNITS);
        if name.is_empty() {
            continue;
        }
        let channels = slot(image, ZONE_CHANNELS + index * BETWEEN_ZONES, BETWEEN_ZONES)
            .map(|data| {
                (0..CHANNELS_PER_ZONE)
                    .map(|member| get_u16_le(data, member as usize * 2))
                    .filter(|member| *member != NO_INDEX16)
                    .filter_map(|member| catalog.channels.name_at(u32::from(member)))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        codeplug.zones.push(Zone {
            name: names.claim(&name, "Zone"),
            channels_a: channels,
            channels_b: Vec::new(),
        });
    }
}

fn encode_zones(image: &mut Image, catalog: &Catalog, codeplug: &Codeplug) -> Result<(), CpsError> {
    for index in 0..NUM_ZONES {
        let name_addr = ZONE_NAMES + index * BETWEEN_ZONE_NAMES;
        let channel_addr = ZONE_CHANNELS + index * BETWEEN_ZONES;
        if !touched(
            image,
            name_addr,
            BETWEEN_ZONE_NAMES,
            index as usize,
            codeplug.zones.len(),
        ) {
            continue;
        }
        let zone = codeplug.zones.get(index as usize).cloned();
        let names = slot_mut(image, name_addr, BETWEEN_ZONE_NAMES)?;
        match zone.as_ref() {
            Some(zone) => {
                prepare(names);
                write_utf16(names, 0, &zone.name, NAME_UNITS);
            }
            None => clear(names),
        }
        let channels = slot_mut(image, channel_addr, BETWEEN_ZONES)?;
        let Some(zone) = zone else {
            clear(channels);
            continue;
        };
        channels[..CHANNELS_PER_ZONE as usize * 2].fill(0xff);
        for (position, name) in zone
            .channels_a
            .iter()
            .chain(zone.channels_b.iter())
            .take(CHANNELS_PER_ZONE as usize)
            .enumerate()
        {
            if let Some(at) = catalog
                .channels
                .index_of(name)
                .and_then(|at| u16::try_from(at).ok())
            {
                set_u16_le(channels, position * 2, at);
            }
        }
    }
    Ok(())
}

fn scan_revert_from(raw: u8) -> ScanRevert {
    match raw {
        2 | 6 => ScanRevert::Primary,
        3 | 7 => ScanRevert::Secondary,
        4 => ScanRevert::LastCalled,
        5 => ScanRevert::LastUsed,
        _ => ScanRevert::Selected,
    }
}

fn scan_revert_to(revert: ScanRevert) -> u8 {
    match revert {
        ScanRevert::Selected => 0,
        ScanRevert::Primary => 2,
        ScanRevert::Secondary => 3,
        ScanRevert::LastCalled => 4,
        ScanRevert::LastUsed => 5,
    }
}

fn scan_target(raw: u16, catalog: &Catalog) -> Option<ScanTarget> {
    match raw {
        NO_INDEX16 => None,
        0 => Some(ScanTarget::Selected),
        other => catalog
            .channels
            .name_at(u32::from(other - 1))
            .map(|name| ScanTarget::Channel {
                name: name.to_owned(),
            }),
    }
}

fn scan_target_raw(target: Option<&ScanTarget>, catalog: &Catalog) -> u16 {
    match target {
        None => NO_INDEX16,
        Some(ScanTarget::Selected) => 0,
        Some(ScanTarget::Channel { name }) => catalog
            .channels
            .index_of(name)
            .and_then(|at| u16::try_from(at + 1).ok())
            .unwrap_or(NO_INDEX16),
    }
}

fn decode_scan_list_names(image: &Image, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_SCAN_LISTS {
        let Some(data) = slot(
            image,
            SCAN_LISTS + index * BETWEEN_SCAN_LISTS,
            BETWEEN_SCAN_LISTS,
        ) else {
            break;
        };
        if is_erased(data) {
            continue;
        }
        let name = read_utf16(data, SCAN_NAME_AT, NAME_UNITS);
        if name.is_empty() {
            continue;
        }
        codeplug.scan_lists.push(ScanList {
            name: names.claim(&name, "Scan list"),
            revert: scan_revert_from(get_u8(data, SCAN_REVERT_AT)),
            dwell_ms: Some(u32::from(get_u16_le(data, 0x000c)) * 100),
            hang_ms: Some(u32::from(get_u16_le(data, 0x000a)) * 100),
            ..ScanList::default()
        });
    }
}

fn link_scan_lists(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut position = 0usize;
    for index in 0..NUM_SCAN_LISTS {
        let Some(data) = slot(
            image,
            SCAN_LISTS + index * BETWEEN_SCAN_LISTS,
            BETWEEN_SCAN_LISTS,
        ) else {
            break;
        };
        if is_erased(data) || read_utf16(data, SCAN_NAME_AT, NAME_UNITS).is_empty() {
            continue;
        }
        let Some(list) = codeplug.scan_lists.get_mut(position) else {
            break;
        };
        position += 1;
        let priority = get_u8(data, 0x0001);
        list.channels = (0..SCAN_LIST_MEMBERS)
            .map(|member| get_u16_le(data, SCAN_MEMBERS_AT + member as usize * 2))
            .filter(|member| *member != NO_INDEX16)
            .filter_map(|member| catalog.channels.name_at(u32::from(member)))
            .map(str::to_owned)
            .collect();
        list.primary = (priority == 1 || priority == 3)
            .then(|| scan_target(get_u16_le(data, 0x0002), catalog))
            .flatten();
        list.secondary = (priority == 2 || priority == 3)
            .then(|| scan_target(get_u16_le(data, 0x0004), catalog))
            .flatten();
    }
}

fn encode_scan_lists(
    image: &mut Image,
    catalog: &Catalog,
    codeplug: &Codeplug,
) -> Result<(), CpsError> {
    for index in 0..NUM_SCAN_LISTS {
        let addr = SCAN_LISTS + index * BETWEEN_SCAN_LISTS;
        if !touched(
            image,
            addr,
            BETWEEN_SCAN_LISTS,
            index as usize,
            codeplug.scan_lists.len(),
        ) {
            continue;
        }
        let list = codeplug.scan_lists.get(index as usize).cloned();
        let data = slot_mut(image, addr, BETWEEN_SCAN_LISTS)?;
        let Some(list) = list else {
            clear(data);
            continue;
        };
        prepare(data);
        let priority = u8::from(list.primary.is_some()) + 2 * u8::from(list.secondary.is_some());
        set_u8(data, 0x0001, priority);
        if let Some(primary) = list.primary.as_ref() {
            set_u16_le(data, 0x0002, scan_target_raw(Some(primary), catalog));
        }
        if let Some(secondary) = list.secondary.as_ref() {
            set_u16_le(data, 0x0004, scan_target_raw(Some(secondary), catalog));
        }
        set_u16_le(data, 0x000a, (list.hang_ms.unwrap_or(2900) / 100) as u16);
        set_u16_le(data, 0x000c, (list.dwell_ms.unwrap_or(2900) / 100) as u16);
        write_utf16(data, SCAN_NAME_AT, &list.name, NAME_UNITS);
        for member in 0..SCAN_LIST_MEMBERS as usize {
            set_u16_le(data, SCAN_MEMBERS_AT + member * 2, NO_INDEX16);
        }
        for (position, name) in list
            .channels
            .iter()
            .take(SCAN_LIST_MEMBERS as usize)
            .enumerate()
        {
            if let Some(at) = catalog
                .channels
                .index_of(name)
                .and_then(|at| u16::try_from(at).ok())
            {
                set_u16_le(data, SCAN_MEMBERS_AT + position * 2, at);
            }
        }
        set_u8(data, SCAN_REVERT_AT, scan_revert_to(list.revert));
    }
    Ok(())
}

fn decode_channels(image: &Image, catalog: &Catalog, codeplug: &mut Codeplug) {
    let mut names = UniqueNames::default();
    for index in 0..NUM_CHANNELS {
        let Some(data) = slot(image, channel_address(index), CHANNEL_SIZE) else {
            continue;
        };
        if let Some(decoded) = channel::decode(data, catalog, &mut names) {
            codeplug.channels.push(decoded);
        }
    }
}

fn encode_channels(
    image: &mut Image,
    catalog: &Catalog,
    codeplug: &Codeplug,
) -> Result<(), CpsError> {
    for index in 0..NUM_CHANNELS {
        let addr = channel_address(index);
        if !touched(
            image,
            addr,
            CHANNEL_SIZE,
            index as usize,
            codeplug.channels.len(),
        ) {
            continue;
        }
        let item = codeplug.channels.get(index as usize).cloned();
        let data = slot_mut(image, addr, CHANNEL_SIZE)?;
        let Some(item) = item else {
            clear(data);
            continue;
        };
        prepare(data);
        channel::encode(data, &item, catalog);
    }
    Ok(())
}
