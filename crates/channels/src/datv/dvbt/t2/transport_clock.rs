use super::DecodeError;
use crate::datv::dvbs::PACKET;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    Counter { value: u32, bits: u8 },
    Presentation(u64),
}

impl Clock {
    pub fn parse(bytes: &[u8], origin: u64) -> Option<Self> {
        let first = *bytes.first()?;
        if first & 0x80 == 0 && bytes.len() == 2 {
            Some(Self::Counter {
                value: u32::from(u16::from_be_bytes([first, bytes[1]])),
                bits: 15,
            })
        } else if first & 0xc0 == 0x80 && bytes.len() == 3 {
            Some(Self::Counter {
                value: (u32::from(first & 0x3f) << 16)
                    | (u32::from(bytes[1]) << 8)
                    | u32::from(bytes[2]),
                bits: 22,
            })
        } else if first & 0xf0 == 0xd0 && matches!(bytes.len(), 2 | 3) {
            let exponent = u32::from(first & 0x0f) * 2 + u32::from(bytes[1] >> 7);
            let mantissa =
                u64::from(bytes[1] & 0x7f) * 256 + u64::from(bytes.get(2).copied().unwrap_or(0));
            Some(Self::Presentation(
                origin + ((mantissa << exponent) + 128) / 256,
            ))
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedPacket {
    pub data: [u8; PACKET],
    pub clock: Option<Clock>,
}

pub(super) trait Sink {
    fn remaining(&self) -> usize;
    fn packet(&mut self, data: [u8; PACKET], clock: Option<Clock>) -> Result<(), DecodeError>;
}

impl Sink for Vec<[u8; PACKET]> {
    fn remaining(&self) -> usize {
        self.capacity() - self.len()
    }
    fn packet(&mut self, data: [u8; PACKET], _: Option<Clock>) -> Result<(), DecodeError> {
        if self.remaining() == 0 {
            return Err(DecodeError::Capacity);
        }
        self.push(data);
        Ok(())
    }
}

impl Sink for Vec<TimedPacket> {
    fn remaining(&self) -> usize {
        self.capacity() - self.len()
    }
    fn packet(&mut self, data: [u8; PACKET], clock: Option<Clock>) -> Result<(), DecodeError> {
        if self.remaining() == 0 {
            return Err(DecodeError::Capacity);
        }
        self.push(TimedPacket { data, clock });
        Ok(())
    }
}
