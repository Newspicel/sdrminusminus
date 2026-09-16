use sdrmm_dsp::{DVB_PRIMITIVE, ReedSolomon, crc32_mpeg};
use sdrmm_wire::BroadcastData;

use super::pad::{Event, crc_ok, mot::Mot};

mod group;

#[cfg(test)]
mod tests;

const APPLICATION: usize = 2256;
const FEC_FRAME: usize = 2472;
const MAX_GROUP: usize = 16384;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub address: u16,
    pub kind: u8,
    pub data_groups: bool,
    pub fec: bool,
}

pub struct PacketData {
    config: Config,
    pending: Vec<u8>,
    packet_pending: Vec<u8>,
    fec_aligned: bool,
    group: Vec<u8>,
    active: bool,
    continuity: Option<(u8, u32)>,
    mot: Mot,
    groups: group::Groups,
    rs: ReedSolomon,
    sequence: u64,
}

impl PacketData {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            pending: Vec::with_capacity(2 * FEC_FRAME),
            packet_pending: Vec::with_capacity(16384),
            fec_aligned: false,
            group: Vec::with_capacity(MAX_GROUP),
            active: false,
            continuity: None,
            mot: Mot::default(),
            groups: group::Groups::default(),
            rs: ReedSolomon::new(DVB_PRIMITIVE, 0, 16),
            sequence: 0,
        }
    }

    pub fn push(&mut self, bytes: &[u8], events: &mut Vec<Event>) {
        if self.config.address == 0 {
            self.deliver(bytes, events);
            return;
        }
        if !self.config.fec {
            self.packets(bytes, events);
            return;
        }
        self.pending.extend_from_slice(bytes);
        let mut at = 0;
        while at + FEC_FRAME <= self.pending.len() {
            let headers = (0..9)
                .filter(|&i| {
                    let pos = at + APPLICATION + i * 24;
                    self.pending[pos] == ((i as u8) << 2 | 3) && self.pending[pos + 1] == 254
                })
                .count();
            if headers < 7 {
                if self.fec_aligned {
                    events.push(Event::Error("DAB packet FEC alignment lost"));
                    self.fec_aligned = false;
                    self.active = false;
                    self.packet_pending.clear();
                }
                at += 24;
                continue;
            }
            self.fec_aligned = true;
            let mut corrected = [0u8; APPLICATION];
            let mut valid = true;
            for row in 0..12 {
                let mut word = [0u8; 204];
                for (column, byte) in word.iter_mut().take(188).enumerate() {
                    *byte = self.pending[at + column * 12 + row];
                }
                for column in 0..16 {
                    let index = column * 12 + row;
                    word[188 + column] =
                        self.pending[at + APPLICATION + (index / 22) * 24 + 2 + index % 22];
                }
                if self.rs.decode(&mut word).is_none() {
                    valid = false;
                    break;
                }
                for column in 0..188 {
                    corrected[column * 12 + row] = word[column];
                }
            }
            if valid {
                self.packets(&corrected, events);
            } else {
                self.active = false;
                events.push(Event::Error("Uncorrectable DAB packet-mode FEC frame"));
            }
            at += FEC_FRAME;
        }
        self.pending.drain(..at);
    }

    fn packets(&mut self, bytes: &[u8], events: &mut Vec<Event>) {
        let mut pending = std::mem::take(&mut self.packet_pending);
        pending.extend_from_slice(bytes);
        let mut at = 0;
        while pending.len() - at >= 24 {
            let length = (usize::from(pending[at] >> 6) + 1) * 24;
            let Some(packet) = pending.get(at..at + length) else {
                break;
            };
            let address = (u16::from(packet[0] & 3) << 8) | u16::from(packet[1]);
            if address == self.config.address {
                if !crc_ok(packet) {
                    self.active = false;
                    events.push(Event::Error("DAB packet CRC failure"));
                } else {
                    self.packet(packet, events);
                }
            }
            at += length;
        }
        pending.drain(..at);
        self.packet_pending = pending;
    }

    fn packet(&mut self, bytes: &[u8], events: &mut Vec<Event>) {
        let continuity = (bytes[0] >> 4) & 3;
        let first = bytes[0] & 8 != 0;
        let last = bytes[0] & 4 != 0;
        let length = usize::from(bytes[2] & 127);
        if bytes[2] & 128 != 0 {
            events.push(Event::Error("DAB conditional-access command packet"));
            return;
        }
        if length > bytes.len() - 5 {
            self.active = false;
            events.push(Event::Error("DAB packet payload exceeds its length"));
            return;
        }
        let fingerprint = crc32_mpeg(bytes);
        if self.continuity == Some((continuity, fingerprint)) {
            return;
        }
        if self
            .continuity
            .is_some_and(|(old, _)| (old + 1) & 3 != continuity)
        {
            self.active = false;
            events.push(Event::Error("DAB packet continuity gap"));
        }
        self.continuity = Some((continuity, fingerprint));
        let payload = &bytes[3..3 + length];
        if !self.config.data_groups {
            self.deliver(payload, events);
            return;
        }
        if first {
            self.group.clear();
            self.active = true;
        }
        if !self.active {
            return;
        }
        if self.group.len() + length > MAX_GROUP {
            self.active = false;
            events.push(Event::Error("DAB data group exceeds size limit"));
            return;
        }
        self.group.extend_from_slice(payload);
        if last {
            self.active = false;
            let group = std::mem::take(&mut self.group);
            if self.config.kind == 60 {
                match self.mot.push(&group) {
                    Ok(Some(object)) => events.push(Event::Object(object)),
                    Ok(None) => {}
                    Err(error) => events.push(Event::Error(error)),
                }
            } else {
                match self.groups.push(&group) {
                    Ok(Some(payload)) => self.deliver(&payload, events),
                    Ok(None) => {}
                    Err(error) => events.push(Event::Error(error)),
                }
            }
            self.group = group;
            self.group.clear();
        }
    }

    fn deliver(&mut self, bytes: &[u8], events: &mut Vec<Event>) {
        if bytes.is_empty() {
            return;
        }
        let protocol = if self.config.kind == 59 {
            match bytes[0] >> 4 {
                4 if bytes.len() >= 20
                    && usize::from(u16::from_be_bytes([bytes[2], bytes[3]])) == bytes.len() =>
                {
                    Some(0x0800)
                }
                6 if bytes.len() >= 40
                    && 40 + usize::from(u16::from_be_bytes([bytes[4], bytes[5]]))
                        == bytes.len() =>
                {
                    Some(0x86dd)
                }
                _ => {
                    events.push(Event::Error("Invalid DAB IP datagram"));
                    return;
                }
            }
        } else {
            None
        };
        self.sequence = self.sequence.wrapping_add(1);
        events.push(Event::Object(BroadcastData {
            protocol,
            label: Vec::new(),
            service_id: None,
            name: format!("DAB-{}-{}.bin", self.config.address, self.sequence),
            media_type: "application/octet-stream".to_owned(),
            bytes: bytes.to_vec(),
        }));
    }
}
