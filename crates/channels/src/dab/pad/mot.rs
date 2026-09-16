use std::collections::BTreeMap;

use sdrmm_wire::BroadcastData;

use super::{crc_ok, label::text};

const MAX_OBJECT: usize = 2 * 1024 * 1024;
const MAX_SEGMENTS: usize = 4096;

#[derive(Default)]
struct Entity {
    segments: BTreeMap<usize, Vec<u8>>,
    last: Option<usize>,
    length: usize,
}

impl Entity {
    fn push(&mut self, index: usize, last: bool, bytes: &[u8]) -> Result<(), &'static str> {
        if index >= MAX_SEGMENTS {
            return Err("MOT segment number exceeds limit");
        }
        if let Some(existing) = self.segments.get(&index) {
            if existing != bytes {
                return Err("Conflicting MOT retransmission");
            }
            return Ok(());
        }
        if self.length + bytes.len() > MAX_OBJECT {
            return Err("MOT object exceeds size limit");
        }
        if last {
            if self.last.is_some_and(|old| old != index) {
                return Err("Conflicting final MOT segment");
            }
            self.last = Some(index);
        }
        self.length += bytes.len();
        self.segments.insert(index, bytes.to_vec());
        Ok(())
    }

    fn complete(&self) -> Option<Vec<u8>> {
        let last = self.last?;
        if (0..=last).any(|i| !self.segments.contains_key(&i)) {
            return None;
        }
        Some(
            (0..=last)
                .flat_map(|i| self.segments[&i].iter().copied())
                .collect(),
        )
    }
}

#[derive(Default)]
struct Object {
    header: Entity,
    body: Entity,
    delivered: bool,
}

#[derive(Default)]
pub struct Mot {
    objects: BTreeMap<u16, Object>,
}

impl Mot {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<BroadcastData>, &'static str> {
        if bytes.len() < 11 || !crc_ok(bytes) {
            return Err("MOT data-group CRC failure");
        }
        if bytes[0] & 0x70 != 0x70 || !matches!(bytes[0] & 15, 3 | 4) {
            return Err("Unsupported MOT data-group header");
        }
        let at = if bytes[0] & 128 != 0 { 4 } else { 2 };
        let header = bytes
            .get(at..at + 3)
            .ok_or("Truncated MOT session header")?;
        let last = header[0] & 128 != 0;
        let index = (usize::from(header[0] & 127) << 8) | usize::from(header[1]);
        let access_length = usize::from(header[2] & 15);
        if header[2] & 16 == 0 || access_length < 2 {
            return Err("MOT transport identifier missing");
        }
        let access = bytes
            .get(at + 3..at + 3 + access_length)
            .ok_or("Truncated MOT transport identifier")?;
        let id = u16::from_be_bytes([access[0], access[1]]);
        let at = at + 3 + access_length;
        let segment = bytes
            .get(at..bytes.len() - 2)
            .filter(|s| s.len() >= 2)
            .ok_or("Truncated MOT segment")?;
        let length = (usize::from(segment[0] & 31) << 8) | usize::from(segment[1]);
        if length != segment.len() - 2 {
            return Err("MOT segment length mismatch");
        }
        if !self.objects.contains_key(&id) && self.objects.len() >= 8 {
            if let Some(completed) = self
                .objects
                .iter()
                .find_map(|(&id, object)| object.delivered.then_some(id))
            {
                self.objects.remove(&completed);
            } else {
                return Err("Too many incomplete MOT objects");
            }
        }
        let object = self.objects.entry(id).or_default();
        let entity = if bytes[0] & 15 == 3 {
            &mut object.header
        } else {
            &mut object.body
        };
        entity.push(index, last, &segment[2..])?;
        if object.delivered {
            return Ok(None);
        }
        let (Some(header), Some(body)) = (object.header.complete(), object.body.complete()) else {
            return Ok(None);
        };
        let result = parse_header(&header, body, id)?;
        object.delivered = result.is_some();
        Ok(result)
    }
}

fn parse_header(
    header: &[u8],
    body: Vec<u8>,
    id: u16,
) -> Result<Option<BroadcastData>, &'static str> {
    if header.len() < 7 {
        return Err("Truncated MOT object header");
    }
    let body_length = (usize::from(header[0]) << 20)
        | (usize::from(header[1]) << 12)
        | (usize::from(header[2]) << 4)
        | usize::from(header[3] >> 4);
    let header_length = (usize::from(header[3] & 15) << 9)
        | (usize::from(header[4]) << 1)
        | usize::from(header[5] >> 7);
    if header_length != header.len() || body_length != body.len() {
        return Err("MOT object length mismatch");
    }
    let kind = (header[5] & 127) >> 1;
    let subtype = (u16::from(header[5] & 1) << 8) | u16::from(header[6]);
    let media_type = match (kind, subtype) {
        (1, 0) => "text/plain",
        (1, 1) => "text/html",
        (2, 0) => "image/gif",
        (2, 1) => "image/jpeg",
        (2, 2) => "image/bmp",
        (2, 3) => "image/png",
        _ => "application/octet-stream",
    };
    let mut name = format!("MOT-{id:04X}");
    let mut at = 7;
    let mut triggered = true;
    while at < header.len() {
        let parameter = header[at];
        at += 1;
        let length = match parameter >> 6 {
            0 => 0,
            1 => 1,
            2 => 4,
            _ => {
                let first = *header.get(at).ok_or("Truncated MOT parameter length")?;
                at += 1;
                if first & 128 == 0 {
                    usize::from(first)
                } else {
                    let second = *header.get(at).ok_or("Truncated MOT parameter length")?;
                    at += 1;
                    (usize::from(first & 127) << 8) | usize::from(second)
                }
            }
        };
        let value = header
            .get(at..at + length)
            .ok_or("Truncated MOT parameter")?;
        match parameter & 63 {
            5 => {
                triggered = value.first().ok_or("Empty MOT trigger time")? & 128 == 0;
            }
            12 => {
                let (&charset, bytes) = value.split_first().ok_or("Empty MOT content name")?;
                name = if charset >> 4 == 4 {
                    bytes.iter().map(|&byte| char::from(byte)).collect()
                } else {
                    text(bytes, charset >> 4)?
                };
            }
            _ => {}
        }
        at += length;
    }
    Ok(triggered.then(|| BroadcastData {
        protocol: None,
        label: Vec::new(),
        service_id: None,
        name,
        media_type: media_type.to_owned(),
        bytes: body,
    }))
}
