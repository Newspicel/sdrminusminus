use sdrmm_wire::cps::{
    Admit, Bandwidth, Channel, ChannelMode, DmrChannel, FmChannel, Power, TimeSlot, Tone,
};

use super::layout::{SlotNames, is_vacant, put_text, text};
use crate::{
    bits::{
        get_bcd8_le, get_bits, get_u8, get_u16_le, get_u32_le, set_bcd8_le, set_bits, set_u8,
        set_u16_le, set_u32_le,
    },
    tones::{dcs_from_binary, dcs_to_binary},
};

#[derive(Clone, Copy, Debug)]
struct Field {
    byte: usize,
    shift: u8,
    width: u8,
}

impl Field {
    const fn new(byte: usize, shift: u8, width: u8) -> Self {
        Self { byte, shift, width }
    }

    fn get(self, record: &[u8]) -> u8 {
        get_bits(record, self.byte, self.shift, self.width)
    }

    fn is_set(self, record: &[u8]) -> bool {
        self.get(record) != 0
    }

    fn set(self, record: &mut [u8], value: u8) {
        set_bits(record, self.byte, self.shift, self.width, value);
    }

    fn flag(self, record: &mut [u8], value: bool) {
        self.set(record, u8::from(value));
    }
}

const PROMISCUOUS: Field = Field::new(0, 0, 1);
const SECOND_SLOT: Field = Field::new(0, 1, 1);
const OWN_ID_ENABLED: Field = Field::new(0, 3, 1);
const TX_MODE: Field = Field::new(0, 4, 2);
const ANALOGUE: Field = Field::new(0, 6, 1);
const COLOUR_CODE: Field = Field::new(1, 4, 4);
const HIGH_POWER: Field = Field::new(2, 6, 1);
const FM_ADMIT: Field = Field::new(3, 3, 2);
const DMR_ADMIT: Field = Field::new(3, 5, 2);
const NARROW: Field = Field::new(4, 6, 1);

const RX_FREQ_AT: usize = 0x05;
const TX_FREQ_AT: usize = 0x09;
const RX_TONE_AT: usize = 0x0d;
const TX_TONE_AT: usize = 0x0f;
const CONTACT_AT: usize = 0x11;
const GROUP_LIST_AT: usize = 0x14;
const OWN_ID_AT: usize = 0x16;
const NAME_AT: usize = 0x20;
pub const NAME_LEN: usize = 16;

const RX_ONLY: u8 = 1;
const HZ_PER_UNIT: u64 = 10;

const TONE_NONE: u16 = 0x0fff;
const TONE_CODE_MASK: u16 = 0x0fff;
const TONE_CTCSS: u16 = 1;
const TONE_DCS_NORMAL: u16 = 2;
const TONE_DCS_INVERTED: u16 = 3;

pub fn tone_from_word(word: u16) -> Option<Tone> {
    let code = word & TONE_CODE_MASK;
    match word >> 12 {
        TONE_CTCSS => Some(Tone::Ctcss { decihertz: code }),
        TONE_DCS_NORMAL => Some(Tone::Dcs {
            code: dcs_from_binary(code),
            inverted: false,
        }),
        TONE_DCS_INVERTED => Some(Tone::Dcs {
            code: dcs_from_binary(code),
            inverted: true,
        }),
        _ => None,
    }
}

pub fn tone_word(tone: Option<Tone>) -> u16 {
    match tone {
        None => TONE_NONE,
        Some(Tone::Ctcss { decihertz }) => (TONE_CTCSS << 12) | (decihertz & TONE_CODE_MASK),
        Some(Tone::Dcs { code, inverted }) => {
            let kind = if inverted {
                TONE_DCS_INVERTED
            } else {
                TONE_DCS_NORMAL
            };
            (kind << 12) | dcs_to_binary(code)
        }
    }
}

fn hz(record: &[u8], at: usize) -> u64 {
    u64::from(get_u32_le(record, at)) * HZ_PER_UNIT
}

fn put_hz(record: &mut [u8], at: usize, hz: u64) {
    set_u32_le(
        record,
        at,
        u32::try_from(hz / HZ_PER_UNIT).unwrap_or(u32::MAX),
    );
}

fn fm_admit(raw: u8) -> Admit {
    match raw {
        1 => Admit::ChannelFree,
        2 => Admit::ToneFree,
        _ => Admit::Always,
    }
}

fn fm_admit_raw(admit: Admit) -> u8 {
    match admit {
        Admit::ChannelFree => 1,
        Admit::ToneFree => 2,
        _ => 0,
    }
}

fn dmr_admit(raw: u8) -> Admit {
    match raw {
        1 => Admit::ChannelFree,
        2 => Admit::ColorCodeFree,
        _ => Admit::Always,
    }
}

fn dmr_admit_raw(admit: Admit) -> u8 {
    match admit {
        Admit::ChannelFree => 1,
        Admit::ColorCodeFree | Admit::DifferentColorCode => 2,
        _ => 0,
    }
}

pub struct Links<'a> {
    pub contacts: &'a SlotNames,
    pub group_lists: &'a SlotNames,
}

pub struct Decoded {
    pub channel: Channel,
    pub own_id: Option<u32>,
}

pub fn decode(record: &[u8], links: &Links) -> Option<Decoded> {
    if is_vacant(record) {
        return None;
    }
    let name = text(record, NAME_AT, NAME_LEN);
    if name.is_empty() {
        return None;
    }
    let (mode, own_id) = if ANALOGUE.is_set(record) {
        (ChannelMode::Fm(decode_fm(record)), None)
    } else {
        decode_dmr(record, links)
    };
    let channel = Channel {
        name,
        rx_hz: hz(record, RX_FREQ_AT),
        tx_hz: hz(record, TX_FREQ_AT),
        power: if HIGH_POWER.is_set(record) {
            Power::High
        } else {
            Power::Low
        },
        rx_only: TX_MODE.get(record) == RX_ONLY,
        timeout_s: None,
        scan_list: None,
        mode,
    };
    Some(Decoded { channel, own_id })
}

fn decode_fm(record: &[u8]) -> FmChannel {
    FmChannel {
        bandwidth: if NARROW.is_set(record) {
            Bandwidth::Narrow
        } else {
            Bandwidth::Wide
        },
        rx_tone: tone_from_word(get_u16_le(record, RX_TONE_AT)),
        tx_tone: tone_from_word(get_u16_le(record, TX_TONE_AT)),
        squelch: None,
        admit: fm_admit(FM_ADMIT.get(record)),
    }
}

fn decode_dmr(record: &[u8], links: &Links) -> (ChannelMode, Option<u32>) {
    let group_list = match get_u8(record, GROUP_LIST_AT) {
        0 => None,
        slot => links.group_lists.name(usize::from(slot) - 1),
    };
    let dmr = DmrChannel {
        color_code: COLOUR_CODE.get(record),
        time_slot: if SECOND_SLOT.is_set(record) {
            TimeSlot::Two
        } else {
            TimeSlot::One
        },
        contact: links
            .contacts
            .name(usize::from(get_u16_le(record, CONTACT_AT))),
        group_list,
        radio_id: None,
        admit: dmr_admit(DMR_ADMIT.get(record)),
    };
    let own_id = OWN_ID_ENABLED
        .is_set(record)
        .then(|| get_bcd8_le(record, OWN_ID_AT));
    (ChannelMode::Dmr(dmr), own_id)
}

pub struct Targets {
    pub contact: Option<u16>,
    pub group_list: Option<u8>,
    pub own_id: Option<u32>,
}

pub fn encode(record: &mut [u8], channel: &Channel, targets: &Targets) {
    PROMISCUOUS.flag(record, false);
    TX_MODE.set(record, if channel.rx_only { RX_ONLY } else { 0 });
    HIGH_POWER.flag(record, channel.power >= Power::Mid);
    put_hz(record, RX_FREQ_AT, channel.rx_hz);
    put_hz(record, TX_FREQ_AT, channel.tx_hz);
    put_text(record, NAME_AT, NAME_LEN, &channel.name);
    match &channel.mode {
        ChannelMode::Fm(fm) => encode_fm(record, fm),
        ChannelMode::Dmr(dmr) => encode_dmr(record, dmr, targets),
    }
}

fn encode_fm(record: &mut [u8], fm: &FmChannel) {
    ANALOGUE.flag(record, true);
    NARROW.flag(record, fm.bandwidth == Bandwidth::Narrow);
    FM_ADMIT.set(record, fm_admit_raw(fm.admit));
    DMR_ADMIT.set(record, 0);
    set_u16_le(record, RX_TONE_AT, tone_word(fm.rx_tone));
    set_u16_le(record, TX_TONE_AT, tone_word(fm.tx_tone));
    SECOND_SLOT.flag(record, false);
    COLOUR_CODE.set(record, 0);
    OWN_ID_ENABLED.flag(record, false);
    set_u16_le(record, CONTACT_AT, 0);
    set_u8(record, GROUP_LIST_AT, 0);
    set_bcd8_le(record, OWN_ID_AT, 0);
}

fn encode_dmr(record: &mut [u8], dmr: &DmrChannel, targets: &Targets) {
    ANALOGUE.flag(record, false);
    NARROW.flag(record, true);
    FM_ADMIT.set(record, 0);
    DMR_ADMIT.set(record, dmr_admit_raw(dmr.admit));
    set_u16_le(record, RX_TONE_AT, TONE_NONE);
    set_u16_le(record, TX_TONE_AT, TONE_NONE);
    SECOND_SLOT.flag(record, dmr.time_slot == TimeSlot::Two);
    COLOUR_CODE.set(record, dmr.color_code);
    set_u16_le(record, CONTACT_AT, targets.contact.unwrap_or(0));
    set_u8(
        record,
        GROUP_LIST_AT,
        targets.group_list.map_or(0, |slot| slot.saturating_add(1)),
    );
    OWN_ID_ENABLED.flag(record, targets.own_id.is_some());
    set_bcd8_le(record, OWN_ID_AT, targets.own_id.unwrap_or(0));
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: usize = 0x30;

    fn named_slots() -> SlotNames {
        let mut names = SlotNames::default();
        for slot in 0..8 {
            names.remember(slot, &format!("slot {slot}"));
        }
        names
    }

    #[test]
    fn subtones_round_trip_through_their_word() {
        assert_eq!(tone_word(None), 0x0fff);
        assert_eq!(tone_from_word(0x0fff), None);
        assert_eq!(tone_from_word(0x4123), None);
        let ctcss = Some(Tone::Ctcss { decihertz: 1230 });
        assert_eq!(tone_word(ctcss), 0x14ce);
        assert_eq!(tone_from_word(0x14ce), ctcss);
        let normal = Some(Tone::Dcs {
            code: 23,
            inverted: false,
        });
        assert_eq!(tone_word(normal), 0x2013);
        assert_eq!(tone_from_word(0x2013), normal);
        let inverted = Some(Tone::Dcs {
            code: 754,
            inverted: true,
        });
        assert_eq!(tone_word(inverted), 0x31ec);
        assert_eq!(tone_from_word(0x31ec), inverted);
    }

    #[test]
    fn an_fm_channel_keeps_every_field_it_has() {
        let channel = Channel {
            name: "OE1XUU".to_owned(),
            rx_hz: 438_950_000,
            tx_hz: 431_350_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                rx_tone: Some(Tone::Ctcss { decihertz: 1230 }),
                tx_tone: Some(Tone::Dcs {
                    code: 23,
                    inverted: true,
                }),
                squelch: None,
                admit: Admit::ToneFree,
            }),
        };
        let mut record = [0u8; RECORD];
        let targets = Targets {
            contact: None,
            group_list: None,
            own_id: None,
        };
        encode(&mut record, &channel, &targets);
        assert_eq!(record[0], 0x40);
        assert_eq!(
            &record[RX_FREQ_AT..RX_FREQ_AT + 4],
            &43_895_000u32.to_le_bytes()
        );
        let none = SlotNames::default();
        let links = Links {
            contacts: &none,
            group_lists: &none,
        };
        let decoded = decode(&record, &links).expect("decode");
        assert_eq!(decoded.channel, channel);
        assert_eq!(decoded.own_id, None);
    }

    #[test]
    fn a_dmr_channel_points_at_its_slots_and_own_id() {
        let channel = Channel {
            name: "DMR".to_owned(),
            rx_hz: 438_500_000,
            tx_hz: 430_900_000,
            power: Power::Low,
            rx_only: true,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Dmr(DmrChannel {
                color_code: 7,
                time_slot: TimeSlot::Two,
                contact: Some("slot 4".to_owned()),
                group_list: Some("slot 2".to_owned()),
                radio_id: None,
                admit: Admit::ColorCodeFree,
            }),
        };
        let mut record = [0u8; RECORD];
        let targets = Targets {
            contact: Some(4),
            group_list: Some(2),
            own_id: Some(2_628_002),
        };
        encode(&mut record, &channel, &targets);
        assert_eq!(record[0], 0b0001_1010);
        assert_eq!(record[1], 0x70);
        assert_eq!(record[3], 0x40);
        assert_eq!(record[4], 0x40);
        assert_eq!(record[GROUP_LIST_AT], 3);
        assert_eq!(&record[OWN_ID_AT..OWN_ID_AT + 4], &[0x02, 0x80, 0x62, 0x02]);
        let names = named_slots();
        let links = Links {
            contacts: &names,
            group_lists: &names,
        };
        let decoded = decode(&record, &links).expect("decode");
        assert_eq!(decoded.channel, channel);
        assert_eq!(decoded.own_id, Some(2_628_002));
    }

    #[test]
    fn an_erased_or_nameless_slot_is_empty() {
        let none = SlotNames::default();
        let links = Links {
            contacts: &none,
            group_lists: &none,
        };
        assert!(decode(&[0xffu8; RECORD], &links).is_none());
        assert!(decode(&[0u8; RECORD], &links).is_none());
    }
}
