use std::collections::BTreeMap;

use super::{
    fic::FIB_BYTES,
    protection::{Eep, Protection, eep_bitrate_kbps, uep_bitrate_kbps, uep_size_cu},
};

const LABEL_BYTES: usize = 16;
const END_MARKER: u8 = 0xFF;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audio {
    #[default]
    Mp2,
    AacPlus,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubChannel {
    pub id: u8,
    pub start_cu: u16,
    pub size_cu: u16,
    pub bitrate_kbps: u16,
    pub protection: Protection,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub id: u32,
    pub label: Option<String>,
    pub subchannel: Option<u8>,
    pub audio: Audio,
    pub data: bool,
    pub mot_app: Option<u8>,
    pub packet_id: Option<u16>,
    pub data_kind: u8,
}

#[derive(Clone, Debug)]
struct PacketComponent {
    subchannel: u8,
    address: u16,
    kind: u8,
    data_groups: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Ensemble {
    pub id: Option<u16>,
    pub label: Option<String>,
    pub services: BTreeMap<u32, Service>,
    pub subchannels: BTreeMap<u8, SubChannel>,
    packets: BTreeMap<u16, PacketComponent>,
    fec: BTreeMap<u8, bool>,
}

impl Ensemble {
    pub fn clear(&mut self) {
        self.id = None;
        self.label = None;
        self.services.clear();
        self.subchannels.clear();
        self.packets.clear();
        self.fec.clear();
    }

    pub fn playable(&self) -> impl Iterator<Item = (&Service, &SubChannel)> {
        self.services.values().filter_map(|service| {
            let id = service.subchannel.or_else(|| {
                service
                    .packet_id
                    .and_then(|id| self.packets.get(&id).map(|packet| packet.subchannel))
            })?;
            Some((service, self.subchannels.get(&id)?))
        })
    }

    #[cfg(test)]
    #[must_use]
    pub fn pick(&self, wanted: Option<u32>) -> Option<(&Service, &SubChannel)> {
        match wanted {
            Some(id) => self.playable().find(|(service, _)| service.id == id),
            None => self.playable().find(|(service, _)| !service.data),
        }
    }

    pub fn absorb(&mut self, fib: &[u8; FIB_BYTES]) {
        let mut at = 0usize;
        while at < FIB_BYTES - 2 {
            let header = fib[at];
            if header == END_MARKER {
                return;
            }
            let kind = header >> 5;
            let length = usize::from(header & 0x1F);
            let body = match fib.get(at + 1..at + 1 + length) {
                Some(body) if length > 0 => body,
                _ => return,
            };
            match kind {
                0 => self.figure_zero(body),
                1 => self.figure_one(body),
                _ => {}
            }
            at += 1 + length;
        }
    }

    fn figure_zero(&mut self, body: &[u8]) {
        let Some((&header, data)) = body.split_first() else {
            return;
        };
        if header & 0xc0 != 0 {
            return;
        }
        let long_ids = header & 0x20 != 0;
        match header & 0x1F {
            0 => self.ensemble_information(data),
            1 => self.subchannel_organization(data),
            2 => self.service_organization(data, long_ids),
            3 => self.packet_components(data),
            14 => {
                for &entry in data {
                    self.fec.insert(entry >> 2, entry & 3 == 1);
                }
            }
            13 => self.user_applications(data, long_ids),
            _ => {}
        }
    }

    pub fn packet_config(&self, service: &Service) -> Option<super::packet::Config> {
        if !service.data {
            return None;
        }
        if let Some(id) = service.packet_id {
            let packet = self.packets.get(&id)?;
            Some(super::packet::Config {
                address: packet.address,
                kind: packet.kind,
                data_groups: packet.data_groups,
                fec: self.fec.get(&packet.subchannel).copied().unwrap_or(false),
            })
        } else {
            Some(super::packet::Config {
                address: 0,
                kind: service.data_kind,
                data_groups: false,
                fec: false,
            })
        }
    }

    fn packet_components(&mut self, mut bytes: &[u8]) {
        while bytes.len() >= 5 {
            let length = if bytes[1] & 1 != 0 { 7 } else { 5 };
            if bytes.len() < length {
                return;
            }
            let id = (u16::from(bytes[0]) << 4) | u16::from(bytes[1] >> 4);
            self.packets.insert(
                id,
                PacketComponent {
                    subchannel: bytes[3] >> 2,
                    address: (u16::from(bytes[3] & 3) << 8) | u16::from(bytes[4]),
                    kind: bytes[2] & 63,
                    data_groups: bytes[2] & 128 == 0,
                },
            );
            bytes = &bytes[length..];
        }
    }

    fn user_applications(&mut self, mut bytes: &[u8], long_ids: bool) {
        let width = if long_ids { 4 } else { 2 };
        while bytes.len() > width {
            let id = bytes[..width]
                .iter()
                .fold(0u32, |id, byte| (id << 8) | u32::from(*byte));
            let component = bytes[width] >> 4;
            let count = bytes[width] & 15;
            bytes = &bytes[width + 1..];
            for _ in 0..count {
                if bytes.len() < 2 {
                    return;
                }
                let kind = (u16::from(bytes[0]) << 3) | u16::from(bytes[1] >> 5);
                let length = usize::from(bytes[1] & 31);
                let Some(application) = bytes.get(2..2 + length) else {
                    return;
                };
                if kind == 2 && component == 0 && application.len() >= 2 {
                    let app = application[0] & 31;
                    if (4..=30).contains(&app) {
                        self.services
                            .entry(id)
                            .or_insert_with(|| Service {
                                id,
                                ..Service::default()
                            })
                            .mot_app = Some(app);
                    }
                }
                bytes = &bytes[2 + length..];
            }
        }
    }

    fn ensemble_information(&mut self, data: &[u8]) {
        if data.len() >= 2 {
            self.id = Some(u16::from_be_bytes([data[0], data[1]]));
        }
    }

    fn subchannel_organization(&mut self, data: &[u8]) {
        let mut at = 0usize;
        while at + 3 <= data.len() {
            let id = data[at] >> 2;
            let start_cu = u16::from(data[at] & 0x03) << 8 | u16::from(data[at + 1]);
            let long_form = data[at + 2] & 0x80 != 0;
            let (size_cu, bitrate, protection, used) = if long_form {
                if at + 4 > data.len() {
                    return;
                }
                let option = data[at + 2] >> 4 & 0x07;
                if option > 1 {
                    return;
                }
                let level = (data[at + 2] >> 2 & 0x03) + 1;
                let size = u16::from(data[at + 2] & 0x03) << 8 | u16::from(data[at + 3]);
                let profile = if option == 0 { Eep::A } else { Eep::B };
                let Some(bitrate) = eep_bitrate_kbps(size, profile, level) else {
                    return;
                };
                let Some(protection) = Protection::eep(bitrate, profile, level) else {
                    return;
                };
                (size, bitrate, protection, 4)
            } else {
                let index = data[at + 2] & 0x3F;
                let (Some(size), Some(bitrate), Some(protection)) = (
                    uep_size_cu(index),
                    uep_bitrate_kbps(index),
                    Protection::uep(index),
                ) else {
                    return;
                };
                (size, bitrate, protection, 3)
            };
            self.subchannels.insert(
                id,
                SubChannel {
                    id,
                    start_cu,
                    size_cu,
                    bitrate_kbps: bitrate,
                    protection,
                },
            );
            at += used;
        }
    }

    fn service_organization(&mut self, data: &[u8], long_ids: bool) {
        let identifier = if long_ids { 4 } else { 2 };
        let mut at = 0usize;
        while at + identifier < data.len() {
            let id = if long_ids {
                u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
            } else {
                u32::from(u16::from_be_bytes([data[at], data[at + 1]]))
            };
            let components = usize::from(data[at + identifier] & 0x0F);
            let first = at + identifier + 1;
            if first + 2 * components > data.len() {
                return;
            }
            let service = self.services.entry(id).or_insert_with(|| Service {
                id,
                ..Service::default()
            });
            for index in 0..components {
                let descriptor = &data[first + 2 * index..first + 2 * index + 2];
                let transport = descriptor[0] >> 6;
                let primary = descriptor[1] & 0x02 != 0;
                if !(primary || index == 0) {
                    continue;
                }
                match transport {
                    0 => {
                        let kind = descriptor[0] & 0x3F;
                        service.subchannel = Some(descriptor[1] >> 2);
                        service.audio = if kind == 63 {
                            Audio::AacPlus
                        } else {
                            Audio::Mp2
                        };
                        service.data = false;
                        service.packet_id = None;
                    }
                    1 => {
                        service.subchannel = Some(descriptor[1] >> 2);
                        service.data = true;
                        service.data_kind = descriptor[0] & 63;
                        service.packet_id = None;
                    }
                    3 => {
                        service.packet_id = Some(
                            (u16::from(descriptor[0] & 63) << 6) | u16::from(descriptor[1] >> 2),
                        );
                        service.data = true;
                        service.subchannel = None;
                    }
                    _ => {}
                }
            }
            at = first + 2 * components;
        }
    }

    fn figure_one(&mut self, body: &[u8]) {
        let Some((&header, data)) = body.split_first() else {
            return;
        };
        if header & 0x08 != 0 {
            return;
        }
        match header & 0x07 {
            0 => {
                if data.len() >= 2 + LABEL_BYTES {
                    self.id = Some(u16::from_be_bytes([data[0], data[1]]));
                    self.label = decoded_label(&data[2..2 + LABEL_BYTES], header >> 4);
                }
            }
            1 => {
                if data.len() >= 2 + LABEL_BYTES {
                    let id = u32::from(u16::from_be_bytes([data[0], data[1]]));
                    let text = decoded_label(&data[2..2 + LABEL_BYTES], header >> 4);
                    self.services
                        .entry(id)
                        .or_insert_with(|| Service {
                            id,
                            ..Service::default()
                        })
                        .label = text;
                }
            }
            5 if data.len() >= 4 + LABEL_BYTES => {
                let id = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                let text = decoded_label(&data[4..4 + LABEL_BYTES], header >> 4);
                self.services
                    .entry(id)
                    .or_insert_with(|| Service {
                        id,
                        ..Service::default()
                    })
                    .label = text;
            }
            _ => {}
        }
    }
}

fn decoded_label(bytes: &[u8], charset: u8) -> Option<String> {
    let text = super::pad::label::text(bytes, charset).ok()?;
    let text = text.trim_end_matches([' ', '\0']).to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
#[must_use]
pub fn label(bytes: &[u8]) -> Option<String> {
    let text = ebu_text(bytes);
    let trimmed = text.trim().to_owned();
    (!trimmed.is_empty()).then_some(trimmed)
}

pub fn ebu_text(bytes: &[u8]) -> String {
    const LOW: [&str; 32] = [
        "", "Ę", "Į", "Ų", "Ă", "Ė", "Ď", "Ș", "Ț", "Ċ", "", "", "Ġ", "Ĺ", "Ż", "Ń", "ą", "ę", "į",
        "ų", "ă", "ė", "ď", "ș", "ț", "ċ", "Ň", "Ě", "ġ", "ĺ", "ż", "",
    ];
    const HIGH: [&str; 133] = [
        "«", "ů", "»", "Ľ", "Ħ", "á", "à", "é", "è", "í", "ì", "ó", "ò", "ú", "ù", "Ñ", "Ç", "Ş",
        "ß", "¡", "Ÿ", "â", "ä", "ê", "ë", "î", "ï", "ô", "ö", "û", "ü", "ñ", "ç", "ş", "ğ", "ı",
        "ÿ", "Ķ", "Ņ", "©", "Ģ", "Ğ", "ě", "ň", "ő", "Ő", "€", "£", "$", "Ā", "Ē", "Ī", "Ū", "ķ",
        "ņ", "Ļ", "ģ", "ļ", "İ", "ń", "ű", "Ű", "¿", "ľ", "°", "ā", "ē", "ī", "ū", "Á", "À", "É",
        "È", "Í", "Ì", "Ó", "Ò", "Ú", "Ù", "Ř", "Č", "Š", "Ž", "Ð", "Ŀ", "Â", "Ä", "Ê", "Ë", "Î",
        "Ï", "Ô", "Ö", "Û", "Ü", "ř", "č", "š", "ž", "đ", "ŀ", "Ã", "Å", "Æ", "Œ", "ŷ", "Ý", "Õ",
        "Ø", "Þ", "Ŋ", "Ŕ", "Ć", "Ś", "Ź", "Ť", "ð", "ã", "å", "æ", "œ", "ŵ", "ý", "õ", "ø", "þ",
        "ŋ", "ŕ", "ć", "ś", "ź", "ť", "ħ",
    ];
    let mut text = String::with_capacity(bytes.len());
    for &byte in bytes {
        match byte {
            0..=31 => text.push_str(LOW[usize::from(byte)]),
            0x7b..=0xff => text.push_str(HIGH[usize::from(byte - 0x7b)]),
            0x24 => text.push('ł'),
            0x5c => text.push('Ů'),
            0x5e => text.push('Ł'),
            0x60 => text.push('Ą'),
            other => text.push(char::from(other)),
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dab::fic::append_fib_crc;

    fn fib(figures: &[(u8, Vec<u8>)]) -> [u8; FIB_BYTES] {
        let mut body = Vec::new();
        for (kind, data) in figures {
            body.push(kind << 5 | data.len() as u8);
            body.extend_from_slice(data);
        }
        body.resize(FIB_BYTES - 2, END_MARKER);
        append_fib_crc(&mut body);
        let mut fib = [0u8; FIB_BYTES];
        fib.copy_from_slice(&body);
        fib
    }

    fn subchannel_figure(id: u8, start_cu: u16, level: u8, size_cu: u16) -> (u8, Vec<u8>) {
        (
            0,
            vec![
                0x01,
                id << 2 | (start_cu >> 8) as u8,
                start_cu as u8,
                0x80 | (level - 1) << 2 | (size_cu >> 8) as u8,
                size_cu as u8,
            ],
        )
    }

    fn service_figure(id: u16, subchannel: u8, aac: bool) -> (u8, Vec<u8>) {
        (
            0,
            vec![
                0x02,
                (id >> 8) as u8,
                id as u8,
                0x01,
                if aac { 63 } else { 0 },
                subchannel << 2 | 0x02,
            ],
        )
    }

    fn label_figure(extension: u8, id: &[u8], text: &str) -> (u8, Vec<u8>) {
        let mut data = vec![extension];
        data.extend_from_slice(id);
        let mut padded = text.as_bytes().to_vec();
        padded.resize(LABEL_BYTES, b' ');
        data.extend_from_slice(&padded);
        data.extend_from_slice(&0xFF00u16.to_be_bytes());
        (1, data)
    }

    #[test]
    fn the_ensemble_label_and_identifier_are_read() {
        let mut ensemble = Ensemble::default();
        ensemble.absorb(&fib(&[label_figure(0, &[0x10, 0xC2], "BBC National")]));
        assert_eq!(ensemble.id, Some(0x10C2));
        assert_eq!(ensemble.label.as_deref(), Some("BBC National"));
    }

    #[test]
    fn a_subchannel_definition_becomes_a_bit_rate_and_a_protection_profile() {
        let mut ensemble = Ensemble::default();
        ensemble.absorb(&fib(&[subchannel_figure(3, 84, 3, 48)]));
        let subchannel = &ensemble.subchannels[&3];
        assert_eq!(subchannel.start_cu, 84);
        assert_eq!(subchannel.size_cu, 48);
        assert_eq!(subchannel.bitrate_kbps, 64);
        assert_eq!(subchannel.protection.frame_bits(), 24 * 64);
    }

    #[test]
    fn a_service_binds_to_its_subchannel_and_audio_generation() {
        let mut ensemble = Ensemble::default();
        ensemble.absorb(&fib(&[
            subchannel_figure(3, 84, 3, 48),
            service_figure(0xC221, 3, true),
        ]));
        ensemble.absorb(&fib(&[label_figure(1, &[0xC2, 0x21], "Radio 1")]));
        let (service, subchannel) = ensemble.pick(None).expect("a playable service");
        assert_eq!(service.id, 0xC221);
        assert_eq!(service.label.as_deref(), Some("Radio 1"));
        assert_eq!(service.audio, Audio::AacPlus);
        assert_eq!(subchannel.id, 3);
    }

    #[test]
    fn a_classic_audio_service_is_reported_as_layer_two() {
        let mut ensemble = Ensemble::default();
        ensemble.absorb(&fib(&[
            subchannel_figure(1, 0, 3, 48),
            service_figure(0xC0FF, 1, false),
        ]));
        let (service, _) = ensemble.pick(None).expect("a playable service");
        assert_eq!(service.audio, Audio::Mp2);
    }

    #[test]
    fn selecting_by_identifier_returns_that_service() {
        let mut ensemble = Ensemble::default();
        ensemble.absorb(&fib(&[
            subchannel_figure(1, 0, 3, 48),
            subchannel_figure(2, 48, 3, 48),
        ]));
        ensemble.absorb(&fib(&[service_figure(0xC001, 1, true)]));
        ensemble.absorb(&fib(&[service_figure(0xC002, 2, true)]));
        let (service, subchannel) = ensemble.pick(Some(0xC002)).expect("the chosen service");
        assert_eq!(service.id, 0xC002);
        assert_eq!(subchannel.id, 2);
        assert!(ensemble.pick(Some(0xDEAD)).is_none());
    }

    #[test]
    fn packet_components_bind_in_either_figure_order_and_expose_fec() {
        for reversed in [false, true] {
            let mut ensemble = Ensemble::default();
            let mut figures = vec![
                (0, vec![2, 0xc0, 1, 1, 0xc0, 0x46]),
                (0, vec![3, 1, 0x10, 60, 12, 17]),
            ];
            if reversed {
                figures.reverse();
            }
            for figure in figures {
                ensemble.absorb(&fib(&[figure]));
            }
            ensemble.absorb(&fib(&[subchannel_figure(3, 120, 3, 48)]));
            ensemble.absorb(&fib(&[(0, vec![14, 13])]));
            let (service, subchannel) = ensemble.pick(Some(0xc001)).expect("packet service");
            assert!(service.data);
            assert_eq!(subchannel.id, 3);
            assert_eq!(
                ensemble.packet_config(service),
                Some(super::super::packet::Config {
                    address: 17,
                    kind: 60,
                    data_groups: true,
                    fec: true
                })
            );
            assert!(ensemble.pick(None).is_none());
        }
    }

    #[test]
    fn a_truncated_figure_is_ignored_rather_than_read_past_its_end() {
        let mut ensemble = Ensemble::default();
        let mut body = vec![0x20u8 | 6, 0x02, 0xC0];
        body.resize(FIB_BYTES - 2, END_MARKER);
        append_fib_crc(&mut body);
        let mut fib = [0u8; FIB_BYTES];
        fib.copy_from_slice(&body);
        ensemble.absorb(&fib);
        assert!(ensemble.services.is_empty());
    }

    #[test]
    fn label_bytes_outside_ascii_map_into_the_ebu_repertoire() {
        assert_eq!(label(b"Radio  ").as_deref(), Some("Radio"));
        assert_eq!(label(&[0x8Bu8, b'a']).as_deref(), Some("Ça"));
        assert!(label(b"    ").is_none());
    }
}
