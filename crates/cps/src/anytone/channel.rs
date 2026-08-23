use sdrmm_wire::cps::{
    Admit, Bandwidth, Channel, ChannelMode, DmrChannel, FmChannel, Power, TimeSlot, Tone,
};

use crate::{
    bits::{
        get_bcd8_be, get_bit, get_bits, get_u8, get_u16_le, get_u32_le, is_erased, read_utf16,
        set_bcd8_be, set_bit, set_bits, set_u8, set_u16_le, set_u32_le, write_utf16,
    },
    catalog::{Catalog, NameList, UniqueNames},
    tones::{ctcss_from_index, dcs_from_binary, dcs_to_binary, nearest_ctcss_index},
};

pub const CHANNEL_SIZE: usize = 0x80;
pub const NAME_UNITS: usize = 16;

const RX_FREQUENCY: usize = 0x0000;
const TX_OFFSET: usize = 0x0004;
const MODE_FLAGS: usize = 0x0008;
const SIGNALING_FLAGS: usize = 0x0009;
const TX_CTCSS: usize = 0x000a;
const RX_CTCSS: usize = 0x000b;
const TX_DCS: usize = 0x000c;
const RX_DCS: usize = 0x000e;
const CUSTOM_CTCSS: usize = 0x0010;
const CONTACT_INDEX: usize = 0x0014;
const RADIO_ID_INDEX: usize = 0x0018;
const ADMIT_FLAGS: usize = 0x001a;
const SCAN_LIST_INDEX: usize = 0x001b;
const GROUP_LIST_INDEX: usize = 0x001c;
const COLOR_CODE: usize = 0x0020;
const DMR_FLAGS: usize = 0x0021;
const NAME: usize = 0x0044;

const MODE_BIT: u8 = 0;
const LEVEL_BIT: u8 = 2;
const BANDWIDTH_BIT: u8 = 4;
const REPEATER_BIT: u8 = 6;

const TX_SIGNALING_BIT: u8 = 0;
const RX_SIGNALING_BIT: u8 = 2;
const SIGNALING_NONE: u8 = 0;
const SIGNALING_CTCSS: u8 = 1;
const SIGNALING_DCS: u8 = 2;

const ADMIT_BIT: u8 = 0;
const TIME_SLOT_BIT: u8 = 0;

const CUSTOM_CTCSS_INDEX: u8 = 0x33;
const NO_INDEX: u8 = 0xff;

#[must_use]
pub fn is_valid(data: &[u8]) -> bool {
    !is_erased(data) && !read_utf16(data, NAME, NAME_UNITS).is_empty() && rx_hz(data) != 0
}

fn rx_hz(data: &[u8]) -> u64 {
    u64::from(get_bcd8_be(data, RX_FREQUENCY)) * 10
}

fn tx_hz(data: &[u8]) -> u64 {
    let offset = u64::from(get_bcd8_be(data, TX_OFFSET)) * 10;
    match get_bits(data, MODE_FLAGS, REPEATER_BIT, 2) {
        2 => rx_hz(data).saturating_sub(offset),
        1 => rx_hz(data).saturating_add(offset),
        _ => rx_hz(data),
    }
}

fn read_tone(data: &[u8], signaling: u8, ctcss_at: usize, dcs_at: usize) -> Option<Tone> {
    match signaling {
        SIGNALING_CTCSS => {
            let index = get_u8(data, ctcss_at);
            if index == CUSTOM_CTCSS_INDEX {
                return Some(Tone::Ctcss {
                    decihertz: get_u16_le(data, CUSTOM_CTCSS),
                });
            }
            ctcss_from_index(index).map(|decihertz| Tone::Ctcss { decihertz })
        }
        SIGNALING_DCS => {
            let raw = get_u16_le(data, dcs_at);
            Some(Tone::Dcs {
                code: dcs_from_binary(raw % 512),
                inverted: raw >= 512,
            })
        }
        _ => None,
    }
}

fn write_tone(data: &mut [u8], tone: Option<Tone>, mode_bit: u8, ctcss_at: usize, dcs_at: usize) {
    match tone {
        None => set_bits(data, SIGNALING_FLAGS, mode_bit, 2, SIGNALING_NONE),
        Some(Tone::Ctcss { decihertz }) => {
            set_bits(data, SIGNALING_FLAGS, mode_bit, 2, SIGNALING_CTCSS);
            set_u8(data, ctcss_at, nearest_ctcss_index(decihertz));
        }
        Some(Tone::Dcs { code, inverted }) => {
            set_bits(data, SIGNALING_FLAGS, mode_bit, 2, SIGNALING_DCS);
            set_u16_le(
                data,
                dcs_at,
                dcs_to_binary(code) + if inverted { 512 } else { 0 },
            );
        }
    }
}

fn power_from(raw: u8) -> Power {
    match raw {
        1 => Power::Mid,
        2 => Power::High,
        3 => Power::Max,
        _ => Power::Low,
    }
}

fn power_to(power: Power) -> u8 {
    match power {
        Power::Min | Power::Low => 0,
        Power::Mid => 1,
        Power::High => 2,
        Power::Max => 3,
    }
}

fn dmr_admit_from(raw: u8) -> Admit {
    match raw {
        1 => Admit::ChannelFree,
        2 => Admit::DifferentColorCode,
        3 => Admit::ColorCodeFree,
        _ => Admit::Always,
    }
}

fn dmr_admit_to(admit: Admit) -> u8 {
    match admit {
        Admit::Always => 0,
        Admit::ChannelFree | Admit::ToneFree => 1,
        Admit::DifferentColorCode => 2,
        Admit::ColorCodeFree => 3,
    }
}

fn fm_admit_from(raw: u8) -> Admit {
    match raw {
        1 => Admit::ToneFree,
        2 => Admit::ChannelFree,
        _ => Admit::Always,
    }
}

fn fm_admit_to(admit: Admit) -> u8 {
    match admit {
        Admit::Always => 0,
        Admit::ToneFree => 1,
        _ => 2,
    }
}

#[must_use]
pub fn decode(data: &[u8], catalog: &Catalog, names: &mut UniqueNames) -> Option<Channel> {
    if !is_valid(data) {
        return None;
    }
    let raw_name = read_utf16(data, NAME, NAME_UNITS);
    let digital = get_bits(data, MODE_FLAGS, MODE_BIT, 2) != 0;
    let mode = if digital {
        ChannelMode::Dmr(DmrChannel {
            color_code: get_u8(data, COLOR_CODE).min(15),
            time_slot: if get_bit(data, DMR_FLAGS, TIME_SLOT_BIT) {
                TimeSlot::Two
            } else {
                TimeSlot::One
            },
            contact: catalog
                .contacts
                .name_at(get_u32_le(data, CONTACT_INDEX))
                .map(str::to_owned),
            group_list: index_name(get_u8(data, GROUP_LIST_INDEX), &catalog.group_lists),
            radio_id: Some(get_u8(data, RADIO_ID_INDEX))
                .filter(|index| *index != 0)
                .and_then(|index| catalog.radio_ids.name_at(u32::from(index)))
                .map(str::to_owned),
            admit: dmr_admit_from(get_bits(data, ADMIT_FLAGS, ADMIT_BIT, 2)),
        })
    } else {
        ChannelMode::Fm(FmChannel {
            bandwidth: if get_bit(data, MODE_FLAGS, BANDWIDTH_BIT) {
                Bandwidth::Wide
            } else {
                Bandwidth::Narrow
            },
            rx_tone: read_tone(
                data,
                get_bits(data, SIGNALING_FLAGS, RX_SIGNALING_BIT, 2),
                RX_CTCSS,
                RX_DCS,
            ),
            tx_tone: read_tone(
                data,
                get_bits(data, SIGNALING_FLAGS, TX_SIGNALING_BIT, 2),
                TX_CTCSS,
                TX_DCS,
            ),
            squelch: None,
            admit: fm_admit_from(get_bits(data, ADMIT_FLAGS, ADMIT_BIT, 2)),
        })
    };
    Some(Channel {
        name: names.claim(&raw_name, "Channel"),
        rx_hz: rx_hz(data),
        tx_hz: tx_hz(data),
        power: power_from(get_bits(data, MODE_FLAGS, LEVEL_BIT, 2)),
        rx_only: false,
        timeout_s: None,
        scan_list: index_name(get_u8(data, SCAN_LIST_INDEX), &catalog.scan_lists),
        mode,
    })
}

pub fn encode(data: &mut [u8], channel: &Channel, catalog: &Catalog) {
    set_bcd8_be(data, RX_FREQUENCY, (channel.rx_hz / 10) as u32);
    let offset = channel.tx_hz.abs_diff(channel.rx_hz);
    if offset != 0 {
        set_bcd8_be(data, TX_OFFSET, (offset / 10) as u32);
        set_bits(
            data,
            MODE_FLAGS,
            REPEATER_BIT,
            2,
            if channel.tx_hz > channel.rx_hz { 1 } else { 2 },
        );
    } else if get_bcd8_be(data, TX_OFFSET) != 0 {
        set_bcd8_be(data, TX_OFFSET, 0);
        set_bits(data, MODE_FLAGS, REPEATER_BIT, 2, 0);
    }
    set_bits(data, MODE_FLAGS, LEVEL_BIT, 2, power_to(channel.power));
    set_u8(data, SCAN_LIST_INDEX, NO_INDEX);
    set_u8(data, GROUP_LIST_INDEX, NO_INDEX);
    if let Some(index) = channel
        .scan_list
        .as_deref()
        .and_then(|name| catalog.scan_lists.index_of(name))
        .and_then(|index| u8::try_from(index).ok())
    {
        set_u8(data, SCAN_LIST_INDEX, index);
    }

    match &channel.mode {
        ChannelMode::Fm(fm) => {
            set_bits(data, MODE_FLAGS, MODE_BIT, 2, 0);
            set_bit(
                data,
                MODE_FLAGS,
                BANDWIDTH_BIT,
                matches!(fm.bandwidth, Bandwidth::Wide),
            );
            write_tone(data, fm.rx_tone, RX_SIGNALING_BIT, RX_CTCSS, RX_DCS);
            write_tone(data, fm.tx_tone, TX_SIGNALING_BIT, TX_CTCSS, TX_DCS);
            set_bits(data, ADMIT_FLAGS, ADMIT_BIT, 2, fm_admit_to(fm.admit));
        }
        ChannelMode::Dmr(dmr) => {
            set_bits(data, MODE_FLAGS, MODE_BIT, 2, 1);
            set_bit(data, MODE_FLAGS, BANDWIDTH_BIT, false);
            write_tone(data, None, RX_SIGNALING_BIT, RX_CTCSS, RX_DCS);
            write_tone(data, None, TX_SIGNALING_BIT, TX_CTCSS, TX_DCS);
            set_u8(data, COLOR_CODE, dmr.color_code.min(15));
            set_bit(
                data,
                DMR_FLAGS,
                TIME_SLOT_BIT,
                matches!(dmr.time_slot, TimeSlot::Two),
            );
            set_u32_le(
                data,
                CONTACT_INDEX,
                dmr.contact
                    .as_deref()
                    .and_then(|name| catalog.contacts.index_of(name))
                    .unwrap_or(0),
            );
            if let Some(index) = dmr
                .group_list
                .as_deref()
                .and_then(|name| catalog.group_lists.index_of(name))
                .and_then(|index| u8::try_from(index).ok())
            {
                set_u8(data, GROUP_LIST_INDEX, index);
            }
            set_u8(
                data,
                RADIO_ID_INDEX,
                dmr.radio_id
                    .as_deref()
                    .and_then(|name| catalog.radio_ids.index_of(name))
                    .and_then(|index| u8::try_from(index).ok())
                    .unwrap_or(0),
            );
            set_bits(data, ADMIT_FLAGS, ADMIT_BIT, 2, dmr_admit_to(dmr.admit));
        }
    }
    write_utf16(data, NAME, &channel.name, NAME_UNITS);
}

fn index_name(raw: u8, list: &NameList) -> Option<String> {
    (raw != NO_INDEX)
        .then(|| list.name_at(u32::from(raw)))
        .flatten()
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::cps::Codeplug;

    use super::*;

    fn catalog() -> Catalog {
        let mut codeplug = Codeplug::empty();
        codeplug.contacts = vec![
            sdrmm_wire::cps::Contact {
                name: "Local".to_owned(),
                ..Default::default()
            },
            sdrmm_wire::cps::Contact {
                name: "Worldwide".to_owned(),
                ..Default::default()
            },
        ];
        codeplug.group_lists = vec![sdrmm_wire::cps::GroupList {
            name: "TG list".to_owned(),
            contacts: Vec::new(),
        }];
        codeplug.scan_lists = vec![sdrmm_wire::cps::ScanList {
            name: "Local scan".to_owned(),
            ..Default::default()
        }];
        codeplug.radio_ids = vec![
            sdrmm_wire::cps::RadioId {
                name: "Home".to_owned(),
                number: 2_628_001,
            },
            sdrmm_wire::cps::RadioId {
                name: "Portable".to_owned(),
                number: 2_628_002,
            },
        ];
        Catalog::of(&codeplug)
    }

    #[test]
    fn an_fm_repeater_channel_round_trips_through_the_element() {
        let channel = Channel {
            name: "OE1XUU".to_owned(),
            rx_hz: 438_950_000,
            tx_hz: 431_350_000,
            power: Power::High,
            rx_only: false,
            timeout_s: None,
            scan_list: Some("Local scan".to_owned()),
            mode: ChannelMode::Fm(FmChannel {
                bandwidth: Bandwidth::Wide,
                rx_tone: Some(Tone::Ctcss { decihertz: 1230 }),
                tx_tone: Some(Tone::Dcs {
                    code: 23,
                    inverted: true,
                }),
                squelch: None,
                admit: Admit::ChannelFree,
            }),
        };
        let mut data = [0u8; CHANNEL_SIZE];
        encode(&mut data, &channel, &catalog());
        let decoded = decode(&data, &catalog(), &mut UniqueNames::default()).expect("valid");
        assert_eq!(decoded, channel);
    }

    #[test]
    fn a_dmr_channel_keeps_its_references_by_name() {
        let channel = Channel {
            name: "TG Worldwide".to_owned(),
            rx_hz: 439_012_500,
            tx_hz: 431_412_500,
            power: Power::Mid,
            rx_only: false,
            timeout_s: None,
            scan_list: None,
            mode: ChannelMode::Dmr(DmrChannel {
                color_code: 1,
                time_slot: TimeSlot::Two,
                contact: Some("Worldwide".to_owned()),
                group_list: Some("TG list".to_owned()),
                radio_id: Some("Portable".to_owned()),
                admit: Admit::ColorCodeFree,
            }),
        };
        let mut data = [0u8; CHANNEL_SIZE];
        encode(&mut data, &channel, &catalog());
        let decoded = decode(&data, &catalog(), &mut UniqueNames::default()).expect("valid");
        assert_eq!(decoded, channel);
    }

    #[test]
    fn an_erased_element_is_not_mistaken_for_a_channel() {
        assert!(
            decode(
                &[0xff; CHANNEL_SIZE],
                &catalog(),
                &mut UniqueNames::default()
            )
            .is_none()
        );
        assert!(
            decode(
                &[0x00; CHANNEL_SIZE],
                &catalog(),
                &mut UniqueNames::default()
            )
            .is_none()
        );
    }

    #[test]
    fn the_frequency_pair_is_stored_as_a_signed_repeater_offset() {
        let mut data = [0u8; CHANNEL_SIZE];
        let mut channel = Channel {
            name: "Simplex".to_owned(),
            rx_hz: 145_500_000,
            tx_hz: 145_500_000,
            ..Default::default()
        };
        encode(&mut data, &channel, &catalog());
        assert_eq!(get_bits(&data, MODE_FLAGS, REPEATER_BIT, 2), 0);
        assert_eq!(tx_hz(&data), 145_500_000);

        channel.tx_hz = 145_000_000;
        encode(&mut data, &channel, &catalog());
        assert_eq!(get_bits(&data, MODE_FLAGS, REPEATER_BIT, 2), 2);
        assert_eq!(tx_hz(&data), 145_000_000);
    }

    #[test]
    fn the_mode_and_signalling_bits_match_the_bytes_a_real_radio_holds() {
        let mut fm = [0u8; CHANNEL_SIZE];
        fm[MODE_FLAGS] = 0x08;
        fm[SIGNALING_FLAGS] = 0x05;
        fm[TX_CTCSS] = 0x29;
        fm[RX_CTCSS] = 0x29;
        write_utf16(&mut fm, NAME, "MT6 FM", NAME_UNITS);
        set_bcd8_be(&mut fm, RX_FREQUENCY, 44_606_875);
        let decoded = decode(&fm, &catalog(), &mut UniqueNames::default()).expect("valid");
        let ChannelMode::Fm(params) = &decoded.mode else {
            panic!("byte 0x08 bit 0 clear means an FM channel");
        };
        assert_eq!(params.rx_tone, Some(Tone::Ctcss { decihertz: 2035 }));
        assert_eq!(params.tx_tone, Some(Tone::Ctcss { decihertz: 2035 }));

        let mut dmr = [0u8; CHANNEL_SIZE];
        dmr[MODE_FLAGS] = 0x05;
        dmr[COLOR_CODE] = 0x01;
        write_utf16(&mut dmr, NAME, "PMR DMR 1", NAME_UNITS);
        set_bcd8_be(&mut dmr, RX_FREQUENCY, 44_600_625);
        let decoded = decode(&dmr, &catalog(), &mut UniqueNames::default()).expect("valid");
        let ChannelMode::Dmr(params) = &decoded.mode else {
            panic!("byte 0x08 bit 0 set means a DMR channel");
        };
        assert_eq!(params.color_code, 1);
        assert_eq!(decoded.power, Power::Mid);
    }
}
