use std::collections::BTreeMap;

use sdrmm_dsp::crc32_mpeg;

use super::dvbs::{PACKET, SYNC};

pub const PROGRAM: u16 = 1;
pub const PMT_PID: u16 = 0x0100;
pub const VIDEO_PID: u16 = 0x0101;
pub const AUDIO_PID: u16 = 0x0102;

const PAT_PID: u16 = 0x0000;
const SDT_PID: u16 = 0x0011;
const NULL_PID: u16 = 0x1FFF;
const PAT_TABLE: u8 = 0x00;
const PMT_TABLE: u8 = 0x02;
const SDT_TABLE: u8 = 0x42;
const MAX_SECTION: usize = 1_024;
const MAX_PES: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Mpeg2Video,
    H264Video,
    H265Video,
    Mpeg1Audio,
    Mpeg2Audio,
    AacAudio,
    Ac3Audio,
    Other(u8),
}

impl StreamKind {
    #[must_use]
    pub const fn from_type(value: u8) -> Self {
        match value {
            0x01 | 0x02 => Self::Mpeg2Video,
            0x1B => Self::H264Video,
            0x24 => Self::H265Video,
            0x03 => Self::Mpeg1Audio,
            0x04 => Self::Mpeg2Audio,
            0x0F | 0x11 => Self::AacAudio,
            0x81 | 0x06 => Self::Ac3Audio,
            other => Self::Other(other),
        }
    }

    #[must_use]
    pub const fn is_video(self) -> bool {
        matches!(self, Self::Mpeg2Video | Self::H264Video | Self::H265Video)
    }

    #[must_use]
    pub const fn is_audio(self) -> bool {
        matches!(
            self,
            Self::Mpeg1Audio | Self::Mpeg2Audio | Self::AacAudio | Self::Ac3Audio
        )
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Mpeg2Video => "MPEG-2 video",
            Self::H264Video => "H.264 video",
            Self::H265Video => "H.265 video",
            Self::Mpeg1Audio => "MPEG-1 audio",
            Self::Mpeg2Audio => "MPEG-2 audio",
            Self::AacAudio => "AAC audio",
            Self::Ac3Audio => "AC-3 audio",
            Self::Other(_) => "private data",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementaryStream {
    pub pid: u16,
    pub kind: StreamKind,
    pub language: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub number: u16,
    pub pmt_pid: u16,
    pub pcr_pid: u16,
    pub name: Option<String>,
    pub provider: Option<String>,
    pub streams: Vec<ElementaryStream>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PesUnit {
    pub pid: u16,
    pub stream_id: u8,
    pub pts: Option<u64>,
    pub payload: Vec<u8>,
}

#[derive(Default)]
struct SectionBuffer {
    data: Vec<u8>,
    want: usize,
}

impl SectionBuffer {
    fn push(&mut self, payload: &[u8], start: bool, out: &mut Vec<Vec<u8>>) {
        let mut body = payload;
        if start {
            let Some((&pointer, rest)) = body.split_first() else {
                return;
            };
            if usize::from(pointer) > rest.len() {
                return;
            }
            self.data.clear();
            self.want = 0;
            body = &rest[usize::from(pointer)..];
        } else if self.data.is_empty() {
            return;
        }
        self.data.extend_from_slice(body);
        loop {
            if self.want == 0 {
                if self.data.len() < 3 {
                    return;
                }
                let length = usize::from(u16::from_be_bytes([self.data[1] & 0x0F, self.data[2]]));
                self.want = length + 3;
                if self.want > MAX_SECTION {
                    self.data.clear();
                    self.want = 0;
                    return;
                }
            }
            if self.data.len() < self.want {
                return;
            }
            let section: Vec<u8> = self.data.drain(..self.want).collect();
            self.want = 0;
            if section_is_valid(&section) {
                out.push(section);
            }
            if self.data.first().is_none_or(|&byte| byte == 0xFF) {
                self.data.clear();
                return;
            }
        }
    }
}

fn section_is_valid(section: &[u8]) -> bool {
    section.len() > 4 && crc32_mpeg(section) == 0
}

#[derive(Default)]
struct PesBuffer {
    data: Vec<u8>,
    active: bool,
}

fn read_timestamp(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 5 {
        return None;
    }
    let value = u64::from(bytes[0] >> 1 & 0x07) << 30
        | u64::from(u16::from_be_bytes([bytes[1], bytes[2]]) >> 1) << 15
        | u64::from(u16::from_be_bytes([bytes[3], bytes[4]]) >> 1);
    Some(value)
}

fn parse_pes(pid: u16, data: &[u8]) -> Option<PesUnit> {
    if data.len() < 9 || data[..3] != [0x00, 0x00, 0x01] {
        return None;
    }
    let stream_id = data[3];
    let declared = usize::from(u16::from_be_bytes([data[4], data[5]]));
    let end = if declared == 0 {
        data.len()
    } else {
        (6 + declared).min(data.len())
    };
    let header_len = usize::from(data[8]);
    let start = 9 + header_len;
    if start > end {
        return None;
    }
    let pts = (data[7] & 0x80 != 0)
        .then(|| read_timestamp(&data[9..]))
        .flatten();
    Some(PesUnit {
        pid,
        stream_id,
        pts,
        payload: data[start..end].to_vec(),
    })
}

fn text(bytes: &[u8]) -> String {
    bytes
        .iter()
        .filter(|&&byte| byte >= 0x20 && byte != 0x7F)
        .map(|&byte| char::from(byte))
        .collect::<String>()
        .trim()
        .to_owned()
}

#[derive(Default)]
pub struct TsDemux {
    sections: BTreeMap<u16, SectionBuffer>,
    pes: BTreeMap<u16, PesBuffer>,
    programs: BTreeMap<u16, Program>,
    pmt_pids: BTreeMap<u16, u16>,
    selected: Option<u16>,
    continuity: BTreeMap<u16, u8>,
    pub discontinuities: u32,
    pub packets: u64,
    scratch: Vec<Vec<u8>>,
}

impl TsDemux {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn programs(&self) -> impl Iterator<Item = &Program> {
        self.programs.values()
    }

    pub fn select(&mut self, program: Option<u16>) {
        if self.selected != program {
            self.selected = program;
            self.pes.clear();
        }
    }

    #[must_use]
    pub fn program(&self) -> Option<&Program> {
        match self.selected {
            Some(number) => self.programs.get(&number),
            None => self.programs.values().find(|program| {
                program
                    .streams
                    .iter()
                    .any(|stream| stream.kind.is_video() || stream.kind.is_audio())
            }),
        }
    }

    pub fn reset(&mut self) {
        self.sections.clear();
        self.pes.clear();
        self.programs.clear();
        self.pmt_pids.clear();
        self.continuity.clear();
        self.discontinuities = 0;
        self.packets = 0;
    }

    pub fn push(&mut self, packet: &[u8; PACKET], units: &mut Vec<PesUnit>) {
        if packet[0] != SYNC {
            return;
        }
        let pid = u16::from_be_bytes([packet[1] & 0x1F, packet[2]]);
        if pid == NULL_PID {
            return;
        }
        self.packets += 1;
        let start = packet[1] & 0x40 != 0;
        let scrambled = packet[3] & 0xC0 != 0;
        let has_adaptation = packet[3] & 0x20 != 0;
        let has_payload = packet[3] & 0x10 != 0;
        let counter = packet[3] & 0x0F;
        self.track_continuity(pid, counter, has_payload);
        let mut offset = 4;
        if has_adaptation {
            let length = usize::from(packet[4]);
            if 5 + length > PACKET {
                return;
            }
            offset = 5 + length;
        }
        if !has_payload || scrambled || offset >= PACKET {
            return;
        }
        let payload = &packet[offset..];
        if self.is_section_pid(pid) {
            self.push_section(pid, payload, start);
        } else if self.is_selected_stream(pid) {
            self.push_pes(pid, payload, start, units);
        }
    }

    fn track_continuity(&mut self, pid: u16, counter: u8, has_payload: bool) {
        if !has_payload {
            return;
        }
        if let Some(previous) = self.continuity.insert(pid, counter)
            && (previous + 1) & 0x0F != counter
        {
            self.discontinuities += 1;
        }
    }

    fn is_section_pid(&self, pid: u16) -> bool {
        pid == PAT_PID || pid == SDT_PID || self.pmt_pids.contains_key(&pid)
    }

    fn is_selected_stream(&self, pid: u16) -> bool {
        self.program()
            .is_some_and(|program| program.streams.iter().any(|stream| stream.pid == pid))
    }

    fn push_section(&mut self, pid: u16, payload: &[u8], start: bool) {
        let mut sections = std::mem::take(&mut self.scratch);
        sections.clear();
        self.sections
            .entry(pid)
            .or_default()
            .push(payload, start, &mut sections);
        for section in &sections {
            self.apply_section(pid, section);
        }
        self.scratch = sections;
    }

    fn apply_section(&mut self, pid: u16, section: &[u8]) {
        match (pid, section[0]) {
            (PAT_PID, PAT_TABLE) => self.apply_pat(section),
            (SDT_PID, SDT_TABLE) => self.apply_sdt(section),
            (_, PMT_TABLE) => self.apply_pmt(pid, section),
            _ => {}
        }
    }

    fn apply_pat(&mut self, section: &[u8]) {
        let body = &section[8..section.len() - 4];
        self.pmt_pids.clear();
        for entry in body.as_chunks::<4>().0 {
            let number = u16::from_be_bytes([entry[0], entry[1]]);
            let pmt_pid = u16::from_be_bytes([entry[2] & 0x1F, entry[3]]);
            if number == 0 {
                continue;
            }
            self.pmt_pids.insert(pmt_pid, number);
            let program = self.programs.entry(number).or_default();
            program.number = number;
            program.pmt_pid = pmt_pid;
        }
        self.programs
            .retain(|number, _| self.pmt_pids.values().any(|value| value == number));
    }

    fn apply_pmt(&mut self, pid: u16, section: &[u8]) {
        let Some(&number) = self.pmt_pids.get(&pid) else {
            return;
        };
        if section.len() < 16 {
            return;
        }
        let pcr_pid = u16::from_be_bytes([section[8] & 0x1F, section[9]]);
        let info_len = usize::from(u16::from_be_bytes([section[10] & 0x0F, section[11]]));
        let mut at = 12 + info_len;
        let end = section.len() - 4;
        let mut streams = Vec::new();
        while at + 5 <= end {
            let kind = StreamKind::from_type(section[at]);
            let stream_pid = u16::from_be_bytes([section[at + 1] & 0x1F, section[at + 2]]);
            let descriptors = usize::from(u16::from_be_bytes([
                section[at + 3] & 0x0F,
                section[at + 4],
            ]));
            let next = at + 5 + descriptors;
            if next > end {
                break;
            }
            streams.push(ElementaryStream {
                pid: stream_pid,
                kind,
                language: language_descriptor(&section[at + 5..next]),
            });
            at = next;
        }
        let program = self.programs.entry(number).or_default();
        program.number = number;
        program.pmt_pid = pid;
        program.pcr_pid = pcr_pid;
        program.streams = streams;
    }

    fn apply_sdt(&mut self, section: &[u8]) {
        if section.len() < 15 {
            return;
        }
        let end = section.len() - 4;
        let mut at = 11;
        while at + 5 <= end {
            let number = u16::from_be_bytes([section[at], section[at + 1]]);
            let descriptors = usize::from(u16::from_be_bytes([
                section[at + 3] & 0x0F,
                section[at + 4],
            ]));
            let next = at + 5 + descriptors;
            if next > end {
                break;
            }
            if let Some((provider, name)) = service_descriptor(&section[at + 5..next])
                && let Some(program) = self.programs.get_mut(&number)
            {
                program.provider = provider;
                program.name = name;
            }
            at = next;
        }
    }

    fn push_pes(&mut self, pid: u16, payload: &[u8], start: bool, units: &mut Vec<PesUnit>) {
        let buffer = self.pes.entry(pid).or_default();
        if start {
            if buffer.active
                && let Some(unit) = parse_pes(pid, &buffer.data)
            {
                units.push(unit);
            }
            buffer.data.clear();
            buffer.active = true;
        } else if !buffer.active {
            return;
        }
        if buffer.data.len() + payload.len() > MAX_PES {
            buffer.data.clear();
            buffer.active = false;
            return;
        }
        buffer.data.extend_from_slice(payload);
    }
}

fn language_descriptor(descriptors: &[u8]) -> Option<String> {
    let mut at = 0;
    while at + 2 <= descriptors.len() {
        let length = usize::from(descriptors[at + 1]);
        let body = descriptors.get(at + 2..at + 2 + length)?;
        if descriptors[at] == 0x0A && body.len() >= 3 {
            return Some(text(&body[..3]));
        }
        at += 2 + length;
    }
    None
}

fn service_descriptor(descriptors: &[u8]) -> Option<(Option<String>, Option<String>)> {
    let mut at = 0;
    while at + 2 <= descriptors.len() {
        let length = usize::from(descriptors[at + 1]);
        let body = descriptors.get(at + 2..at + 2 + length)?;
        if descriptors[at] == 0x48 && body.len() >= 2 {
            let provider_len = usize::from(body[1]);
            let provider = body.get(2..2 + provider_len)?;
            let name_len = usize::from(*body.get(2 + provider_len)?);
            let name = body.get(3 + provider_len..3 + provider_len + name_len)?;
            return Some((
                Some(text(provider)).filter(|value| !value.is_empty()),
                Some(text(name)).filter(|value| !value.is_empty()),
            ));
        }
        at += 2 + length;
    }
    None
}

pub struct TsWriter {
    counters: BTreeMap<u16, u8>,
}

impl TsWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            counters: BTreeMap::new(),
        }
    }

    pub fn section(&mut self, pid: u16, section: &[u8], out: &mut Vec<[u8; PACKET]>) {
        let mut body = vec![0u8];
        body.extend_from_slice(section);
        let crc = crc32_mpeg(section);
        let _ = crc;
        self.payload(pid, &body, out);
    }

    pub fn pes(
        &mut self,
        pid: u16,
        stream_id: u8,
        pts: u64,
        payload: &[u8],
        out: &mut Vec<[u8; PACKET]>,
    ) {
        let mut unit = vec![0x00, 0x00, 0x01, stream_id, 0, 0, 0x80, 0x80, 5];
        unit.push(0x21 | (pts >> 29) as u8 & 0x0E);
        unit.extend_from_slice(&(((pts >> 15) as u16 & 0x7FFF) << 1 | 1).to_be_bytes());
        unit.extend_from_slice(&((pts as u16 & 0x7FFF) << 1 | 1).to_be_bytes());
        unit.extend_from_slice(payload);
        let length = (unit.len() - 6).min(0xFFFF) as u16;
        unit[4..6].copy_from_slice(&length.to_be_bytes());
        self.payload(pid, &unit, out);
    }

    fn payload(&mut self, pid: u16, body: &[u8], out: &mut Vec<[u8; PACKET]>) {
        let mut at = 0usize;
        let mut start = true;
        while at < body.len() {
            let counter = self.counters.entry(pid).or_default();
            let mut packet = [0xFFu8; PACKET];
            packet[0] = SYNC;
            packet[1] = (pid >> 8) as u8 & 0x1F | if start { 0x40 } else { 0x00 };
            packet[2] = pid as u8;
            packet[3] = 0x10 | *counter;
            *counter = (*counter + 1) & 0x0F;
            let remaining = body.len() - at;
            let take = remaining.min(PACKET - 4);
            let head = if remaining < PACKET - 4 {
                let stuffing = PACKET - 4 - remaining;
                packet[3] |= 0x20;
                packet[4] = (stuffing - 1) as u8;
                if stuffing > 1 {
                    packet[5] = 0x00;
                }
                4 + stuffing
            } else {
                4
            };
            packet[head..head + take].copy_from_slice(&body[at..at + take]);
            at += take;
            start = false;
            out.push(packet);
        }
    }
}

impl Default for TsWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn null_packet() -> [u8; PACKET] {
    let mut packet = [0xFFu8; PACKET];
    packet[0] = SYNC;
    packet[1] = 0x1F;
    packet[2] = 0xFF;
    packet[3] = 0x10;
    packet
}

#[must_use]
pub fn build_section(table: u8, id: u16, version: u8, body: &[u8]) -> Vec<u8> {
    let mut section = vec![table, 0, 0];
    section.extend_from_slice(&id.to_be_bytes());
    section.push(0xC1 | version << 1);
    section.push(0);
    section.push(0);
    section.extend_from_slice(body);
    let length = (section.len() - 3 + 4) as u16;
    section[1] = 0xB0 | (length >> 8) as u8;
    section[2] = length as u8;
    let crc = crc32_mpeg(&section);
    section.extend_from_slice(&crc.to_be_bytes());
    section
}

#[must_use]
pub fn pat() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&PROGRAM.to_be_bytes());
    body.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
    build_section(PAT_TABLE, 1, 0, &body)
}

#[must_use]
pub fn pmt() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes());
    body.extend_from_slice(&0xF000u16.to_be_bytes());
    body.extend_from_slice(&[0x02]);
    body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes());
    body.extend_from_slice(&0xF000u16.to_be_bytes());
    body.extend_from_slice(&[0x03]);
    body.extend_from_slice(&(0xE000 | AUDIO_PID).to_be_bytes());
    body.extend_from_slice(&0xF006u16.to_be_bytes());
    body.extend_from_slice(&[0x0A, 0x04, b'e', b'n', b'g', 0x00]);
    build_section(PMT_TABLE, PROGRAM, 0, &body)
}

#[must_use]
pub fn sdt(provider: &str, name: &str) -> Vec<u8> {
    let mut body = vec![0x00, 0x01, 0xFF];
    body.extend_from_slice(&PROGRAM.to_be_bytes());
    body.push(0x00);
    let descriptor = {
        let mut value = vec![0x48, 0, 0x01];
        value.push(provider.len() as u8);
        value.extend_from_slice(provider.as_bytes());
        value.push(name.len() as u8);
        value.extend_from_slice(name.as_bytes());
        value[1] = (value.len() - 2) as u8;
        value
    };
    body.push(0x80 | (descriptor.len() >> 8) as u8);
    body.push(descriptor.len() as u8);
    body.extend_from_slice(&descriptor);
    let mut section = build_section(SDT_TABLE, 1, 0, &body);
    section[1] |= 0x80;
    let length = section.len() - 3;
    section[1] = 0xF0 | (length >> 8) as u8;
    section[2] = length as u8;
    let crc = crc32_mpeg(&section[..section.len() - 4]);
    let end = section.len() - 4;
    section[end..].copy_from_slice(&crc.to_be_bytes());
    section
}

#[cfg(test)]
mod tests {
    use super::*;

    fn multiplex(video: &[u8], audio: &[u8]) -> Vec<[u8; PACKET]> {
        let mut writer = TsWriter::new();
        let mut packets = Vec::new();
        writer.section(PAT_PID, &pat(), &mut packets);
        writer.section(PMT_PID, &pmt(), &mut packets);
        writer.section(SDT_PID, &sdt("BBC", "One"), &mut packets);
        writer.pes(VIDEO_PID, 0xE0, 90_000, video, &mut packets);
        writer.pes(AUDIO_PID, 0xC0, 90_000, audio, &mut packets);
        writer.section(PAT_PID, &pat(), &mut packets);
        writer.section(PMT_PID, &pmt(), &mut packets);
        writer.pes(VIDEO_PID, 0xE0, 93_600, video, &mut packets);
        writer.pes(VIDEO_PID, 0xE0, 97_200, video, &mut packets);
        packets.push(null_packet());
        packets
    }

    #[test]
    fn the_mpeg_crc_matches_the_reference_check_value() {
        assert_eq!(crc32_mpeg(b"123456789"), 0x0376_E6E7);
        assert_eq!(crc32_mpeg(&pat()), 0);
    }

    #[test]
    fn the_program_table_names_its_streams() {
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in multiplex(b"video", b"audio") {
            demux.push(&packet, &mut units);
        }
        let program = demux.program().expect("a program must be discovered");
        assert_eq!(program.number, PROGRAM);
        assert_eq!(program.pcr_pid, VIDEO_PID);
        assert_eq!(program.streams.len(), 2);
        assert_eq!(program.streams[0].kind, StreamKind::Mpeg2Video);
        assert_eq!(program.streams[1].kind, StreamKind::Mpeg1Audio);
        assert_eq!(program.streams[1].language.as_deref(), Some("eng"));
    }

    #[test]
    fn the_service_description_names_the_programme() {
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in multiplex(b"video", b"audio") {
            demux.push(&packet, &mut units);
        }
        let program = demux.program().expect("a program must be discovered");
        assert_eq!(program.name.as_deref(), Some("One"));
        assert_eq!(program.provider.as_deref(), Some("BBC"));
    }

    #[test]
    fn packetized_elementary_streams_come_back_whole() {
        let video: Vec<u8> = (0..2_000u16).map(|value| value as u8).collect();
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in multiplex(&video, b"audio") {
            demux.push(&packet, &mut units);
        }
        let frames: Vec<&PesUnit> = units.iter().filter(|unit| unit.pid == VIDEO_PID).collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload.len(), video.len());
        assert!(frames[0].payload == video);
        assert_eq!(frames[0].stream_id, 0xE0);
        assert_eq!(frames[0].pts, Some(90_000));
        assert_eq!(frames[1].pts, Some(93_600));
    }

    #[test]
    fn a_section_with_a_broken_crc_is_dropped() {
        let mut writer = TsWriter::new();
        let mut packets = Vec::new();
        let mut damaged = pat();
        let last = damaged.len() - 1;
        damaged[last] ^= 0x01;
        writer.section(PAT_PID, &damaged, &mut packets);
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in packets {
            demux.push(&packet, &mut units);
        }
        assert!(demux.program().is_none());
    }

    #[test]
    fn a_gap_in_the_continuity_counter_is_counted() {
        let long: Vec<u8> = (0..600u16).map(|value| value as u8).collect();
        let mut packets = multiplex(&long, b"audio");
        let dropped = packets
            .iter()
            .position(|packet| {
                u16::from_be_bytes([packet[1] & 0x1F, packet[2]]) == VIDEO_PID
                    && packet[1] & 0x40 == 0
            })
            .expect("a continuation packet must exist");
        packets.remove(dropped);
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        for packet in packets {
            demux.push(&packet, &mut units);
        }
        assert_eq!(demux.discontinuities, 1);
    }

    #[test]
    fn null_packets_are_ignored() {
        let mut demux = TsDemux::new();
        let mut units = Vec::new();
        demux.push(&null_packet(), &mut units);
        assert_eq!(demux.packets, 0);
        assert!(units.is_empty());
    }
}
