use serde_json::{Value, json};

use super::fields::{
    PRIORITY, bulletin_status, channel_type_name, checksum_ok, decode_payload, downlink_mhz,
    egc_address_len, egc_area, egc_service_long_name, egc_service_name, frame_to_utc_hms,
    hex_upper, ia5, mes_id, parse_stations, round4, sat_les, services_full, services_short,
    tdm_slots, uplink_mhz,
};

const ASSEMBLY_MAX_AGE: u32 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct StdcPacket {
    pub descriptor: u8,
    pub name: &'static str,
    pub checksum_ok: bool,
    pub text: Option<String>,
    pub details: Value,
    pub fec_corrected: Option<u32>,
    pub uw_ber_ppt: Option<u32>,
    pub raw: Vec<u8>,
}

impl StdcPacket {
    pub fn damaged_frame(rejected_packets: Option<usize>) -> Self {
        Self {
            descriptor: 0,
            name: "damaged-frame",
            checksum_ok: false,
            text: None,
            details: json!({ "rejected_packets": rejected_packets }),
            fec_corrected: None,
            uw_ber_ppt: None,
            raw: Vec::new(),
        }
    }

    fn parsed(packet: &[u8], name: &'static str, text: Option<String>, details: Value) -> Self {
        Self {
            descriptor: packet.first().copied().unwrap_or_default(),
            name,
            checksum_ok: true,
            text,
            details,
            fec_corrected: None,
            uw_ber_ppt: None,
            raw: packet.to_vec(),
        }
    }
}

struct EgcPart {
    double_header_part: u8,
    service: u8,
    continuation: bool,
    priority: u8,
    msg_seq: u16,
    pkt_seq: u8,
    presentation: u8,
    address: Vec<u8>,
    payload: Vec<u8>,
}

struct EgcAssembly {
    msg_seq: u16,
    service: u8,
    priority: u8,
    presentation: u8,
    address: Vec<u8>,
    parts: Vec<(u16, Vec<u8>)>,
    double_header: bool,
    age: u32,
}

struct LcnAssembly {
    lcn: u8,
    parts: Vec<(u8, Vec<u8>)>,
    age: u32,
}

#[derive(Default)]
pub struct PacketParser {
    multiframe: Option<(usize, Vec<u8>)>,
    egc: Vec<EgcAssembly>,
    channels: Vec<LcnAssembly>,
}

fn packet_length(buf: &[u8]) -> Option<usize> {
    let descriptor = *buf.first()?;
    if descriptor & 0x80 == 0 {
        Some(usize::from(descriptor & 0x0F) + 1)
    } else if descriptor & 0x40 == 0 {
        buf.get(1).map(|&length| usize::from(length) + 2)
    } else {
        let high = usize::from(*buf.get(1)?);
        let low = usize::from(*buf.get(2)?);
        Some(((high << 8) | low) + 3)
    }
}

fn inner_length(inner: &[u8]) -> usize {
    let Some(&descriptor) = inner.first() else {
        return 0;
    };
    if descriptor & 0x80 == 0 {
        usize::from(descriptor & 0x0F) + 1
    } else if descriptor & 0x40 == 0 {
        inner.get(1).map_or(0, |&length| usize::from(length) + 2)
    } else {
        let high = usize::from(inner.get(1).copied().unwrap_or(0));
        let low = usize::from(inner.get(2).copied().unwrap_or(0));
        ((high << 8) | low) + 3
    }
}

fn merge(details: &mut Value, extra: &Value) {
    if let (Some(object), Some(extra)) = (details.as_object_mut(), extra.as_object()) {
        for (key, value) in extra {
            object.insert(key.clone(), value.clone());
        }
    }
}

fn insert(details: &mut Value, key: &str, value: Value) {
    if let Some(object) = details.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

fn word(body: &[u8], at: usize) -> Option<u16> {
    body.get(at..at + 2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn bulletin_board(body: &[u8]) -> Value {
    let frame_number = word(body, 2).unwrap_or_default();
    let mut details = json!({
        "network_version": body.get(1),
        "frame_number": frame_number,
        "utc_time": frame_to_utc_hms(frame_number),
        "channel_type": body.get(6).map(|b| b >> 5),
    });
    if let Some(&b) = body.get(4) {
        insert(&mut details, "signalling_channel", json!(b >> 2));
    }
    if let Some(&b) = body.get(5) {
        insert(&mut details, "count", json!(((b >> 4) & 0x0F) * 2));
    }
    if let Some(&b) = body.get(6) {
        insert(
            &mut details,
            "channel_type_name",
            json!(channel_type_name(b >> 5)),
        );
        insert(&mut details, "local", json!((b >> 2) & 0x07));
    }
    if let Some(&b) = body.get(7) {
        insert(&mut details, "sat_les", sat_les(b));
    }
    if let Some(&status) = body.get(8) {
        insert(&mut details, "status", bulletin_status(status));
    }
    if let Some(services) = word(body, 9) {
        insert(&mut details, "services", json!(services_full(services)));
    }
    if let Some(&interval) = body.get(11) {
        insert(&mut details, "random_interval", json!(interval));
    }
    details
}

fn logical_channel_assignment(body: &[u8]) -> Value {
    let mut details = json!({
        "mes_id": mes_id(&body[2..5]),
        "lcn": body.get(7),
        "sat_les": sat_les(body[5]),
        "status_bits": body[6],
    });
    let optional = [
        ("frame_length", body.get(8).map(|&b| json!(b))),
        ("duration", body.get(9).map(|&b| json!(b))),
        (
            "downlink_mhz",
            word(body, 10).map(|w| json!(round4(downlink_mhz(w)))),
        ),
        (
            "uplink_mhz",
            word(body, 12).map(|w| json!(round4(uplink_mhz(w)))),
        ),
        ("frame_offset", body.get(14).map(|&b| json!(b))),
        ("packet_descriptor1", body.get(15).map(|&b| json!(b))),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            insert(&mut details, key, value);
        }
    }
    details
}

fn login_ack(body: &[u8]) -> Value {
    let login_ack_len = body[1];
    let mut details = json!({
        "login_ack_len": login_ack_len,
        "les": hex_upper(&body[2..5]),
        "downlink_mhz": round4(downlink_mhz(u16::from_be_bytes([body[5], body[6]]))),
        "station_start": body[7],
    });
    if login_ack_len > 7 && body.len() >= 9 {
        let records = body.get(9..body.len() - 2).unwrap_or_default();
        let stations = parse_stations(records, usize::from(body[8]));
        insert(&mut details, "station_count", json!(body[8]));
        insert(&mut details, "stations", json!(stations));
    }
    details
}

fn confirmation(body: &[u8]) -> (Option<String>, Value) {
    let short_message_len = body[9];
    let text = (short_message_len > 2 && body.len() >= 13).then(|| ia5(&body[11..body.len() - 2]));
    (
        text,
        json!({
            "mes_id": mes_id(&body[2..5]),
            "sat_les": sat_les(body[5]),
            "short_message_len": short_message_len,
        }),
    )
}

fn les_list(body: &[u8]) -> Value {
    let records = body.get(4..body.len() - 2).unwrap_or_default();
    let stations = parse_stations(records, usize::from(body[3]));
    json!({
        "les_list_len": body[1],
        "station_start": body[2],
        "station_count": body[3],
        "stations": stations,
    })
}

fn signalling_channel(body: &[u8]) -> Value {
    let mut details = json!({
        "services": services_short(body[1]),
        "uplink_mhz": round4(uplink_mhz(u16::from_be_bytes([body[2], body[3]]))),
    });
    if let Some(slots) = body.get(4..11) {
        insert(&mut details, "tdm_slots", json!(tdm_slots(slots)));
    }
    details
}

fn individual_poll(body: &[u8]) -> (Option<String>, Value) {
    let text = (body.len() >= 38).then(|| ia5(&body[13..body.len() - 2]));
    (
        text,
        json!({
            "mes_id": mes_id(&body[2..5]),
            "sat_les": sat_les(body[5]),
        }),
    )
}

fn empty_packet_name(descriptor: u8) -> Option<&'static str> {
    Some(match descriptor {
        0x92 => "login-ack",
        0xA8 => "confirmation",
        0xAB => "les-list",
        0x08 => "ack-request",
        0x6C => "signalling-channel",
        0x2A => "inbound-message-ack",
        0x91 => "distress-alert-ack",
        0x9A => "enhanced-data-report-ack",
        0xA0 => "distress-test-request",
        0xAC => "request-status",
        0xAD => "test-result",
        _ => return None,
    })
}

fn stateless_packet(body: &[u8]) -> (&'static str, Option<String>, Value) {
    let descriptor = body[0];
    let length = body.len();
    match descriptor {
        0x7D if length >= 4 => ("bulletin-board", None, bulletin_board(body)),
        0x81 if length >= 10 => (
            "announcement",
            None,
            json!({
                "mes_id": mes_id(&body[2..5]),
                "sat_les": sat_les(body[5]),
                "lcn": body.get(9),
            }),
        ),
        0x83 if length >= 8 => (
            "logical-channel-assignment",
            None,
            logical_channel_assignment(body),
        ),
        0x92 if length >= 8 => ("login-ack", None, login_ack(body)),
        0xA8 if length >= 11 => {
            let (text, details) = confirmation(body);
            ("confirmation", text, details)
        }
        0xAB if length >= 4 => ("les-list", None, les_list(body)),
        0x08 if length >= 5 => (
            "ack-request",
            None,
            json!({
                "sat_les": sat_les(body[1]),
                "lcn": body[2],
                "uplink_mhz": round4(uplink_mhz(u16::from_be_bytes([body[3], body[4]]))),
            }),
        ),
        0x6C if length >= 4 => ("signalling-channel", None, signalling_channel(body)),
        0xA3 if length >= 8 => {
            let (text, details) = individual_poll(body);
            ("individual-poll", text, details)
        }
        _ => match empty_packet_name(descriptor) {
            Some(name) => (name, None, json!({})),
            None => ("unknown", None, json!({ "hex": hex_upper(body) })),
        },
    }
}

fn parse_egc(descriptor: u8, packet: &[u8]) -> Option<EgcPart> {
    if packet.len() < 10 {
        return None;
    }
    let service = packet[2];
    let address_len = egc_address_len(service);
    if packet.len() < 8 + address_len + 2 {
        return None;
    }
    Some(EgcPart {
        double_header_part: match descriptor {
            0xB1 => 1,
            0xB2 => 2,
            _ => 0,
        },
        service,
        continuation: packet[3] & 0x80 != 0,
        priority: (packet[3] >> 5) & 0x3,
        msg_seq: u16::from_be_bytes([packet[4], packet[5]]),
        pkt_seq: packet[6],
        presentation: packet[7],
        address: packet[8..8 + address_len].to_vec(),
        payload: packet[8 + address_len..packet.len() - 2].to_vec(),
    })
}

impl PacketParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn parse_frame(&mut self, frame: &[u8], out: &mut Vec<StdcPacket>) -> usize {
        for assembly in &mut self.egc {
            assembly.age += 1;
        }
        for channel in &mut self.channels {
            channel.age += 1;
        }
        self.egc.retain(|assembly| assembly.age < ASSEMBLY_MAX_AGE);
        self.channels
            .retain(|channel| channel.age < ASSEMBLY_MAX_AGE);
        self.parse_stream(frame, false, out)
    }

    fn parse_stream(
        &mut self,
        buf: &[u8],
        reencapsulated: bool,
        out: &mut Vec<StdcPacket>,
    ) -> usize {
        let mut position = 0;
        let mut rejected = 0;
        while let Some(rest) = buf.get(position..)
            && let Some(&descriptor) = rest.first()
            && descriptor != 0x00
        {
            let Some(length) = packet_length(rest) else {
                break;
            };
            let Some(packet) = rest.get(..length).filter(|_| length >= 3) else {
                break;
            };
            position += length;
            if checksum_ok(packet, reencapsulated) {
                self.handle_packet(packet, out);
            } else {
                rejected += 1;
            }
        }
        rejected
    }

    fn handle_packet(&mut self, packet: &[u8], out: &mut Vec<StdcPacket>) {
        let descriptor = packet[0];
        match descriptor {
            0xB0..=0xB2 => {
                if let Some(done) =
                    parse_egc(descriptor, packet).and_then(|part| self.push_egc(part))
                {
                    out.push(done);
                }
            }
            0xBD => self.start_multiframe(packet),
            0xBE => self.continue_multiframe(packet, out),
            0x27 if packet.len() >= 8 => {
                if let Some(done) = self.finish_lcn(packet[5]) {
                    out.push(done);
                }
                let details = json!({
                    "mes_id": mes_id(&packet[1..4]),
                    "sat_les": sat_les(packet[4]),
                    "lcn": packet[5],
                });
                out.push(StdcPacket::parsed(
                    packet,
                    "logical-channel-clear",
                    None,
                    details,
                ));
            }
            0xAA if packet.len() >= 5 => {
                let details = self.message_data(packet);
                out.push(StdcPacket::parsed(packet, "message-data", None, details));
            }
            _ => {
                let (name, text, details) = stateless_packet(packet);
                out.push(StdcPacket::parsed(packet, name, text, details));
            }
        }
    }

    fn message_data(&mut self, body: &[u8]) -> Value {
        if body.len() >= 7 {
            let lcn = body[3];
            let sequence = body[4];
            let data = &body[5..body.len() - 2];
            let index = match self.channels.iter().position(|c| c.lcn == lcn) {
                Some(index) => index,
                None => {
                    self.channels.push(LcnAssembly {
                        lcn,
                        parts: Vec::new(),
                        age: 0,
                    });
                    self.channels.len() - 1
                }
            };
            let channel = &mut self.channels[index];
            channel.age = 0;
            if !channel.parts.iter().any(|(s, _)| *s == sequence) {
                channel.parts.push((sequence, data.to_vec()));
            }
        }
        json!({
            "sat_les": sat_les(body[2]),
            "lcn": body[3],
            "pkt_seq": body[4],
        })
    }

    fn start_multiframe(&mut self, body: &[u8]) {
        if body.len() > 4 {
            let inner = &body[2..body.len() - 2];
            if !inner.is_empty() {
                self.multiframe = Some((inner_length(inner), inner.to_vec()));
            }
        }
    }

    fn continue_multiframe(&mut self, body: &[u8], out: &mut Vec<StdcPacket>) {
        let Some((needed, accumulated)) = &mut self.multiframe else {
            return;
        };
        if body.len() > 4 {
            accumulated.extend_from_slice(&body[2..body.len() - 2]);
        }
        if accumulated.len() + 2 >= *needed
            && let Some((_, accumulated)) = self.multiframe.take()
        {
            let rejected = self.parse_stream(&accumulated, true, out);
            if rejected > 0 {
                out.push(StdcPacket::damaged_frame(Some(rejected)));
            }
        }
    }

    fn finish_lcn(&mut self, lcn: u8) -> Option<StdcPacket> {
        let index = self.channels.iter().position(|c| c.lcn == lcn)?;
        let mut channel = self.channels.swap_remove(index);
        if channel.parts.is_empty() {
            return None;
        }
        channel.parts.sort_by_key(|(sequence, _)| *sequence);
        let payload: Vec<u8> = channel
            .parts
            .iter()
            .flat_map(|(_, p)| p.iter().copied())
            .collect();
        let (text, extra) = decode_payload(0xFF, &payload);
        let mut details = json!({ "lcn": lcn, "parts": channel.parts.len() });
        merge(&mut details, &extra);
        Some(StdcPacket {
            descriptor: 0xAA,
            name: "message",
            checksum_ok: true,
            text,
            details,
            fec_corrected: None,
            uw_ber_ppt: None,
            raw: payload,
        })
    }

    fn push_egc(&mut self, part: EgcPart) -> Option<StdcPacket> {
        let index = match self.egc.iter().position(|a| a.msg_seq == part.msg_seq) {
            Some(index) => index,
            None => {
                self.egc.push(EgcAssembly {
                    msg_seq: part.msg_seq,
                    service: part.service,
                    priority: part.priority,
                    presentation: part.presentation,
                    address: part.address.clone(),
                    parts: Vec::new(),
                    double_header: false,
                    age: 0,
                });
                self.egc.len() - 1
            }
        };
        let assembly = &mut self.egc[index];
        assembly.age = 0;
        assembly.double_header |= part.double_header_part != 0;
        let key = u16::from(part.pkt_seq) * 2 + u16::from(part.double_header_part == 2);
        if !assembly.parts.iter().any(|(k, _)| *k == key) {
            assembly.parts.push((key, part.payload));
        }
        if part.continuation || part.double_header_part == 1 {
            return None;
        }
        Some(assembled_egc(self.egc.swap_remove(index)))
    }
}

fn assembled_egc(mut done: EgcAssembly) -> StdcPacket {
    done.parts.sort_by_key(|(key, _)| *key);
    let payload: Vec<u8> = done
        .parts
        .iter()
        .flat_map(|(_, p)| p.iter().copied())
        .collect();
    let (text, extra) = decode_payload(done.presentation, &payload);
    let mut details = json!({
        "service": egc_service_name(done.service),
        "service_long": egc_service_long_name(done.service),
        "service_code": done.service,
        "priority": PRIORITY[usize::from(done.priority)],
        "msg_seq": done.msg_seq,
        "address_hex": hex_upper(&done.address),
        "parts": done.parts.len(),
        "header_format": if done.double_header { "double-0xB1+0xB2" } else { "single-0xB0" },
    });
    if let Some(area) = egc_area(done.service, &done.address) {
        insert(&mut details, "area", area);
    }
    merge(&mut details, &extra);
    StdcPacket {
        descriptor: if done.double_header { 0xB1 } else { 0xB0 },
        name: "egc-message",
        checksum_ok: true,
        text,
        details,
        fec_corrected: None,
        uw_ber_ppt: None,
        raw: payload,
    }
}

pub fn details_with_uw_ber(details: &mut Value, uw_ber_ppt: u32) {
    if let Some(object) = details.as_object_mut() {
        object
            .entry("uw_ber_ppt")
            .or_insert_with(|| json!(uw_ber_ppt));
    }
}

#[cfg(test)]
pub fn build_packet(descriptor_and_body: &[u8]) -> Vec<u8> {
    let mut packet = descriptor_and_body.to_vec();
    packet.extend([0, 0]);
    let (first, second) = super::fields::checksum(&packet);
    let length = packet.len();
    packet[length - 2] = first;
    packet[length - 1] = second;
    packet
}

#[cfg(test)]
mod tests;
