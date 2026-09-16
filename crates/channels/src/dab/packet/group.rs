use std::collections::BTreeMap;

use crate::dab::pad::crc_ok;

#[derive(Default)]
struct Entity {
    segments: BTreeMap<u16, Vec<u8>>,
    last: Option<u16>,
    bytes: usize,
}

#[derive(Default)]
pub struct Groups {
    pending: BTreeMap<u16, Entity>,
}

impl Groups {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>, &'static str> {
        if bytes.len() < 2 {
            return Err("Truncated DAB data-group header");
        }
        let header = bytes[0];
        let end = if header & 64 != 0 {
            if bytes.len() < 4 || !crc_ok(bytes) {
                return Err("DAB data-group CRC failure");
            }
            bytes.len() - 2
        } else {
            bytes.len()
        };
        let mut at = 2 + usize::from(header & 128 != 0) * 2;
        let segment = if header & 32 != 0 {
            let field = bytes
                .get(at..at + 2)
                .ok_or("Truncated DAB segment header")?;
            at += 2;
            Some((
                u16::from_be_bytes([field[0] & 127, field[1]]),
                field[0] & 128 != 0,
            ))
        } else {
            None
        };
        let mut transport = None;
        if header & 16 != 0 {
            let access = *bytes.get(at).ok_or("Truncated DAB user-access field")?;
            at += 1;
            let length = usize::from(access & 15);
            let field = bytes
                .get(at..at + length)
                .ok_or("Truncated DAB user-access data")?;
            at += length;
            if access & 16 != 0 {
                transport = Some(u16::from_be_bytes(
                    field
                        .get(..2)
                        .ok_or("Truncated DAB transport identifier")?
                        .try_into()
                        .map_err(|_| "Invalid transport identifier")?,
                ));
            }
        }
        let payload = bytes
            .get(at..end)
            .ok_or("Truncated DAB data-group payload")?;
        let Some((index, last)) = segment else {
            return Ok(Some(payload.to_vec()));
        };
        let id = transport.ok_or("Segmented DAB data requires a transport identifier")?;
        if index >= 4096 {
            return Err("DAB data segment number exceeds limit");
        }
        if !self.pending.contains_key(&id) && self.pending.len() >= 8 {
            return Err("Too many incomplete DAB data groups");
        }
        let entity = self.pending.entry(id).or_default();
        if let Some(old) = entity.segments.get(&index) {
            if old != payload {
                self.pending.remove(&id);
                return Err("Conflicting DAB data retransmission");
            }
        } else {
            if entity.bytes + payload.len() > 65535 {
                self.pending.remove(&id);
                return Err("DAB datagram exceeds 65535 bytes");
            }
            entity.bytes += payload.len();
            entity.segments.insert(index, payload.to_vec());
        }
        if last {
            if entity.last.is_some_and(|old| old != index) {
                self.pending.remove(&id);
                return Err("Conflicting DAB final segment");
            }
            entity.last = Some(index);
        }
        let Some(last) = entity.last else {
            return Ok(None);
        };
        if (0..=last).any(|i| !entity.segments.contains_key(&i)) {
            return Ok(None);
        }
        let result = (0..=last)
            .flat_map(|i| entity.segments[&i].iter().copied())
            .collect();
        self.pending.remove(&id);
        Ok(Some(result))
    }
}
