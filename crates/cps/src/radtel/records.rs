use sdrmm_wire::cps::{ALL_CALL_NUMBER, Contact, ContactKind};

use super::layout::{is_vacant, put_text, text};
use crate::bits::{
    get_bcd8_le, get_u8, get_u16_le, get_u32_le, set_bcd8_le, set_u8, set_u16_le, set_u32_le,
};

pub const NAME_LEN: usize = 16;
pub const GROUP_LIST_NAME_LEN: usize = 14;
pub const GROUP_LIST_MEMBERS: usize = 32;
pub const ZONE_MEMBERS: usize = 200;

const UNUSED_MEMBER: u16 = 0xffff;
const ALL_CALL_RAW: u32 = 0xaaaa_aaaa;
const HIGHEST_DMR_ID: u32 = 0x00ff_ffff;
const RADIO_ID_AT: usize = 0x180;

const CONTACT_KIND_AT: usize = 0x00;
const CONTACT_NUMBER_AT: usize = 0x01;
const CONTACT_NAME_AT: usize = 0x05;

const GROUP_LIST_NAME_AT: usize = 0x00;
const GROUP_LIST_MEMBERS_AT: usize = 0x10;

const ZONE_HOME_A_AT: usize = 0x00;
const ZONE_HOME_B_AT: usize = 0x02;
const ZONE_NAME_AT: usize = 0x04;
const ZONE_MEMBERS_AT: usize = 0x14;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub name: String,
    pub members: Vec<u16>,
}

pub fn radio_id(settings: &[u8]) -> Option<u32> {
    let number = get_bcd8_le(settings, RADIO_ID_AT);
    (number != 0 && number <= HIGHEST_DMR_ID).then_some(number)
}

pub fn put_radio_id(settings: &mut [u8], number: Option<u32>) {
    set_bcd8_le(settings, RADIO_ID_AT, number.unwrap_or(0));
}

fn named(record: &[u8], at: usize, len: usize) -> Option<String> {
    if is_vacant(record) {
        return None;
    }
    let name = text(record, at, len);
    (!name.is_empty()).then_some(name)
}

pub fn contact(record: &[u8]) -> Option<Contact> {
    let name = named(record, CONTACT_NAME_AT, NAME_LEN)?;
    let kind = match get_u8(record, CONTACT_KIND_AT) {
        1 => ContactKind::Group,
        2 => ContactKind::All,
        _ => ContactKind::Private,
    };
    let number = if get_u32_le(record, CONTACT_NUMBER_AT) == ALL_CALL_RAW {
        ALL_CALL_NUMBER
    } else {
        get_bcd8_le(record, CONTACT_NUMBER_AT)
    };
    Some(Contact {
        name,
        kind,
        number,
        ring: false,
    })
}

pub fn put_contact(record: &mut [u8], contact: &Contact) {
    let kind = match contact.kind {
        ContactKind::Private => 0,
        ContactKind::Group => 1,
        ContactKind::All => 2,
    };
    set_u8(record, CONTACT_KIND_AT, kind);
    if contact.kind == ContactKind::All {
        set_u32_le(record, CONTACT_NUMBER_AT, ALL_CALL_RAW);
    } else {
        set_bcd8_le(record, CONTACT_NUMBER_AT, contact.number);
    }
    put_text(record, CONTACT_NAME_AT, NAME_LEN, &contact.name);
}

fn members(record: &[u8], at: usize, count: usize) -> Vec<u16> {
    (0..count)
        .map(|slot| get_u16_le(record, at + slot * 2))
        .filter(|member| *member != UNUSED_MEMBER)
        .collect()
}

fn put_members(record: &mut [u8], at: usize, count: usize, members: &[u16]) {
    for slot in 0..count {
        let member = members.get(slot).copied().unwrap_or(UNUSED_MEMBER);
        set_u16_le(record, at + slot * 2, member);
    }
}

pub fn group_list(record: &[u8]) -> Option<Listing> {
    Some(Listing {
        name: named(record, GROUP_LIST_NAME_AT, GROUP_LIST_NAME_LEN)?,
        members: members(record, GROUP_LIST_MEMBERS_AT, GROUP_LIST_MEMBERS),
    })
}

pub fn put_group_list(record: &mut [u8], listing: &Listing) {
    put_text(
        record,
        GROUP_LIST_NAME_AT,
        GROUP_LIST_NAME_LEN,
        &listing.name,
    );
    put_members(
        record,
        GROUP_LIST_MEMBERS_AT,
        GROUP_LIST_MEMBERS,
        &listing.members,
    );
}

pub fn zone(record: &[u8]) -> Option<Listing> {
    Some(Listing {
        name: named(record, ZONE_NAME_AT, NAME_LEN)?,
        members: members(record, ZONE_MEMBERS_AT, ZONE_MEMBERS),
    })
}

pub fn put_zone(record: &mut [u8], listing: &Listing) {
    set_u16_le(record, ZONE_HOME_A_AT, 0);
    set_u16_le(record, ZONE_HOME_B_AT, 0);
    put_text(record, ZONE_NAME_AT, NAME_LEN, &listing.name);
    put_members(record, ZONE_MEMBERS_AT, ZONE_MEMBERS, &listing.members);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_radio_id_is_packed_bcd_and_zero_means_none() {
        let mut settings = [0xffu8; 0x400];
        assert_eq!(radio_id(&settings), None);
        put_radio_id(&mut settings, Some(2_628_001));
        assert_eq!(
            &settings[RADIO_ID_AT..RADIO_ID_AT + 4],
            &[0x01, 0x80, 0x62, 0x02]
        );
        assert_eq!(radio_id(&settings), Some(2_628_001));
        put_radio_id(&mut settings, None);
        assert_eq!(radio_id(&settings), None);
    }

    #[test]
    fn an_all_call_contact_keeps_its_marker() {
        let mut record = [0u8; 0x15];
        let all = Contact {
            name: "All".to_owned(),
            kind: ContactKind::All,
            number: ALL_CALL_NUMBER,
            ring: false,
        };
        put_contact(&mut record, &all);
        assert_eq!(&record[1..5], &[0xaa; 4]);
        assert_eq!(contact(&record), Some(all));
    }

    #[test]
    fn a_zone_writes_its_members_and_marks_the_rest_unused() {
        let mut record = [0u8; 0x208];
        let listing = Listing {
            name: "Home".to_owned(),
            members: vec![3, 0],
        };
        put_zone(&mut record, &listing);
        assert_eq!(
            &record[ZONE_MEMBERS_AT..ZONE_MEMBERS_AT + 6],
            &[3, 0, 0, 0, 0xff, 0xff]
        );
        assert_eq!(zone(&record), Some(listing));
    }

    #[test]
    fn a_nameless_or_erased_record_is_empty() {
        assert_eq!(group_list(&[0xffu8; 0x50]), None);
        assert_eq!(group_list(&[0u8; 0x50]), None);
    }
}
