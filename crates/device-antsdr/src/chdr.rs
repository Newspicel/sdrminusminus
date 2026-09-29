use sdrmm_device::DeviceError;

pub(crate) const WORD: usize = 4;
const BASE_BYTES: usize = 2 * WORD;
const TIME_BYTES: usize = 2 * WORD;
const SEQ_MASK: u16 = 0x0fff;
const EOB_BIT: u32 = 1 << 28;
const TIME_BIT: u32 = 1 << 29;
const CONTEXT_BIT: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Data,
    Context,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) kind: Kind,
    pub(crate) seq: u16,
    pub(crate) eob: bool,
    pub(crate) sid: u32,
    pub(crate) time: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Packet<'a> {
    pub(crate) header: Header,
    pub(crate) payload: &'a [u8],
}

impl Header {
    pub(crate) const fn context(sid: u32, seq: u16) -> Self {
        Self {
            kind: Kind::Context,
            seq,
            eob: false,
            sid,
            time: None,
        }
    }

    pub(crate) const fn bytes(&self) -> usize {
        if self.time.is_some() {
            BASE_BYTES + TIME_BYTES
        } else {
            BASE_BYTES
        }
    }

    pub(crate) fn write(&self, payload_bytes: usize, out: &mut [u8]) -> Result<usize, DeviceError> {
        let header = self.bytes();
        let total = header + payload_bytes;
        if total > usize::from(u16::MAX) || out.len() < header {
            return Err(DeviceError::Io(format!(
                "a {total} byte packet does not fit the link"
            )));
        }
        let mut first = total as u32 | u32::from(self.seq & SEQ_MASK) << 16;
        if self.eob {
            first |= EOB_BIT;
        }
        if self.time.is_some() {
            first |= TIME_BIT;
        }
        if self.kind == Kind::Context {
            first |= CONTEXT_BIT;
        }
        put(out, 0, first);
        put(out, 1, self.sid);
        if let Some(time) = self.time {
            put(out, 2, (time >> 32) as u32);
            put(out, 3, time as u32);
        }
        Ok(header)
    }
}

pub(crate) fn read(datagram: &[u8]) -> Result<Packet<'_>, DeviceError> {
    if datagram.len() < BASE_BYTES {
        return Err(malformed(datagram.len(), "shorter than a header"));
    }
    let first = word(datagram, 0);
    let total = (first & 0xffff) as usize;
    let timed = first & TIME_BIT != 0;
    let header = if timed {
        BASE_BYTES + TIME_BYTES
    } else {
        BASE_BYTES
    };
    if total < header || total > datagram.len() {
        return Err(malformed(datagram.len(), "length field out of range"));
    }
    let time = timed.then(|| u64::from(word(datagram, 2)) << 32 | u64::from(word(datagram, 3)));
    Ok(Packet {
        header: Header {
            kind: if first & CONTEXT_BIT != 0 {
                Kind::Context
            } else {
                Kind::Data
            },
            seq: ((first >> 16) as u16) & SEQ_MASK,
            eob: first & EOB_BIT != 0,
            sid: word(datagram, 1),
            time,
        },
        payload: &datagram[header..total],
    })
}

fn malformed(len: usize, why: &str) -> DeviceError {
    DeviceError::Io(format!("malformed {len} byte packet: {why}"))
}

pub(crate) fn word(bytes: &[u8], index: usize) -> u32 {
    let at = index * WORD;
    bytes
        .get(at..at + WORD)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

pub(crate) fn put(out: &mut [u8], index: usize, value: u32) {
    let at = index * WORD;
    if let Some(slot) = out.get_mut(at..at + WORD) {
        slot.copy_from_slice(&value.to_le_bytes());
    }
}

pub(crate) const fn next_seq(seq: u16) -> u16 {
    seq.wrapping_add(1) & SEQ_MASK
}

pub(crate) const fn seq_distance(from: u16, to: u16) -> u16 {
    to.wrapping_sub(from) & SEQ_MASK
}

pub(crate) const fn flip(sid: u32) -> u32 {
    sid.rotate_left(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_packet_carries_its_length_sequence_and_stream() {
        let mut out = [0u8; 16];
        let header = Header::context(0x40, 0x123);
        let len = header.write(8, &mut out).expect("fits");
        assert_eq!(len, 8);
        assert_eq!(word(&out, 0), 0x8123_0010);
        assert_eq!(word(&out, 1), 0x40);
    }

    #[test]
    fn a_timed_packet_reads_back_as_written() {
        let mut out = [0u8; 24];
        let header = Header {
            kind: Kind::Data,
            seq: 0xfff,
            eob: true,
            sid: 0xa0,
            time: Some(0x0102_0304_0506_0708),
        };
        let len = header.write(8, &mut out).expect("fits");
        put(&mut out, 4, 7);
        put(&mut out, 5, 9);
        let packet = read(&out).expect("valid");
        assert_eq!(len, 16);
        assert_eq!(packet.header, header);
        assert_eq!(packet.payload.len(), 8);
        assert_eq!(word(packet.payload, 1), 9);
    }

    #[test]
    fn the_length_field_bounds_the_payload_not_the_datagram() {
        let mut out = [0u8; 32];
        Header::context(0x10, 1).write(4, &mut out).expect("fits");
        assert_eq!(read(&out).expect("valid").payload.len(), 4);
    }

    #[test]
    fn a_packet_claiming_more_than_arrived_is_refused() {
        let mut out = [0u8; 8];
        put(&mut out, 0, 64);
        assert!(read(&out).is_err());
        assert!(read(&out[..4]).is_err());
    }

    #[test]
    fn sequence_numbers_wrap_at_twelve_bits() {
        assert_eq!(next_seq(0xfff), 0);
        assert_eq!(seq_distance(0xffe, 1), 3);
        assert_eq!(flip(0x0000_0010), 0x0010_0000);
    }
}
