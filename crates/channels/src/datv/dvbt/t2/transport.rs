use super::{
    DecodeError,
    transport_clock::{Clock, Sink, TimedPacket},
};
use crate::datv::{dvbs::PACKET, dvbs2::bb::crc8};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub high_efficiency: bool,
    pub stream_id: u8,
    pub data_bits: usize,
    pub sync_distance: Option<usize>,
    pub packet_bytes: usize,
    pub deleted_null_packets: bool,
    pub issy: Option<[u8; 3]>,
}

impl Header {
    pub fn parse(bits: &[bool]) -> Result<Self, DecodeError> {
        if bits.len() < 80 {
            return Err(DecodeError::Header);
        }
        let mut bytes = [0; 10];
        for (byte, bits) in bytes.iter_mut().zip(bits[..80].as_chunks::<8>().0) {
            *byte = bits
                .iter()
                .fold(0, |value, &bit| value << 1 | u8::from(bit));
        }
        let mode = crc8(&bytes[..9]) ^ bytes[9];
        if mode > 1 || bytes[0] & 3 != 0 {
            return Err(DecodeError::Header);
        }
        if bytes[0] >> 6 != 3 {
            return Err(DecodeError::Stream);
        }
        let high_efficiency = mode == 1;
        let deleted_null_packets = bytes[0] & 4 != 0;
        let issy = bytes[0] & 8 != 0;
        let upl = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
        let packet_bytes = if high_efficiency {
            PACKET - 1 + usize::from(deleted_null_packets)
        } else {
            let base = PACKET + usize::from(deleted_null_packets);
            if bytes[6] != 0x47
                || !upl.is_multiple_of(8)
                || if issy {
                    ![base + 2, base + 3].contains(&(upl / 8))
                } else {
                    upl / 8 != base
                }
            {
                return Err(DecodeError::Header);
            }
            upl / 8
        };
        let data_bits = usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
        let distance = u16::from_be_bytes([bytes[7], bytes[8]]);
        let sync_distance = (distance != u16::MAX).then_some(usize::from(distance));
        if data_bits > 53_760
            || data_bits > bits.len() - 80
            || sync_distance.is_some_and(|d| d >= data_bits || d >= packet_bytes * 8)
        {
            return Err(DecodeError::Header);
        }
        Ok(Self {
            high_efficiency,
            stream_id: if bytes[0] & 0x20 != 0 { 0 } else { bytes[1] },
            data_bits,
            sync_distance,
            packet_bytes,
            deleted_null_packets,
            issy: (issy && high_efficiency).then_some([bytes[2], bytes[3], bytes[6]]),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub packets: usize,
    pub crc_errors: usize,
    pub discontinuities: usize,
}

pub struct Transport {
    header: Option<Header>,
    packet: [u8; 192],
    filled: usize,
    byte: u8,
    byte_bits: usize,
    synchronized: bool,
    packet_clock: Option<Clock>,
    origin: u64,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            header: None,
            packet: [0; 192],
            filled: 0,
            byte: 0,
            byte_bits: 0,
            synchronized: false,
            packet_clock: None,
            origin: 0,
        }
    }
}

impl Transport {
    pub fn reset(&mut self) {
        self.header = None;
        self.filled = 0;
        self.byte = 0;
        self.byte_bits = 0;
        self.synchronized = false;
        self.packet_clock = None;
    }

    pub fn push(
        &mut self,
        bits: &[bool],
        output: &mut Vec<[u8; PACKET]>,
    ) -> Result<Report, DecodeError> {
        self.push_to(bits, output)
    }

    pub fn push_timed(
        &mut self,
        bits: &[bool],
        origin: u64,
        output: &mut Vec<TimedPacket>,
    ) -> Result<Report, DecodeError> {
        self.origin = origin;
        self.push_to(bits, output)
    }

    pub(super) fn set_origin(&mut self, origin: u64) {
        self.origin = origin;
    }

    pub(super) fn push_to<S: Sink>(
        &mut self,
        bits: &[bool],
        output: &mut S,
    ) -> Result<Report, DecodeError> {
        let result = self.read(bits, output);
        if result.is_err() {
            self.reset();
        }
        result
    }

    fn read<S: Sink>(&mut self, bits: &[bool], output: &mut S) -> Result<Report, DecodeError> {
        let header = Header::parse(bits)?;
        let mut report = Report::default();
        if self.header.is_some_and(|old| {
            old.high_efficiency != header.high_efficiency
                || old.stream_id != header.stream_id
                || old.packet_bytes != header.packet_bytes
                || old.deleted_null_packets != header.deleted_null_packets
        }) {
            self.reset();
            report.discontinuities += 1;
        }
        self.header = Some(header);
        let period = header.packet_bytes * 8;
        if self.synchronized {
            let expected = (period - (self.filled * 8 + self.byte_bits) % period) % period;
            let stated = header.sync_distance.unwrap_or(header.data_bits);
            if (header.sync_distance.is_some() && stated != expected)
                || (header.sync_distance.is_none() && expected < header.data_bits)
            {
                self.reset();
                self.header = Some(header);
                report.discontinuities += 1;
            }
        }
        let start = if self.synchronized {
            0
        } else if let Some(start) = header.sync_distance {
            self.synchronized = true;
            start
        } else {
            return Ok(report);
        };
        for (offset, &bit) in bits[80 + start..80 + header.data_bits].iter().enumerate() {
            if header.high_efficiency && header.sync_distance == Some(start + offset) {
                self.packet_clock = header
                    .issy
                    .and_then(|bytes| Clock::parse(&bytes, self.origin));
            }
            self.byte = self.byte << 1 | u8::from(bit);
            self.byte_bits += 1;
            if self.byte_bits != 8 {
                continue;
            }
            self.byte_bits = 0;
            if self.filled == header.packet_bytes {
                if crc8(&self.packet[1..self.filled]) == self.byte {
                    self.emit(header, output, &mut report)?;
                } else {
                    report.crc_errors += 1;
                }
                self.filled = 0;
            }
            self.packet[self.filled] = self.byte;
            self.filled += 1;
            if header.high_efficiency && self.filled == header.packet_bytes {
                self.emit(header, output, &mut report)?;
                self.filled = 0;
                self.packet_clock = None;
            }
        }
        Ok(report)
    }

    fn emit<S: Sink>(
        &self,
        header: Header,
        output: &mut S,
        report: &mut Report,
    ) -> Result<(), DecodeError> {
        let nulls = if header.deleted_null_packets {
            usize::from(self.packet[header.packet_bytes - 1])
        } else {
            0
        };
        if output.remaining() < nulls + 1 {
            return Err(DecodeError::Capacity);
        }
        let mut null = [0xff; PACKET];
        null[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        for _ in 0..nulls {
            output.packet(null, None)?;
        }
        let mut packet = [0; PACKET];
        packet[0] = 0x47;
        let start = usize::from(!header.high_efficiency);
        packet[1..].copy_from_slice(&self.packet[start..start + PACKET - 1]);
        let clock = if header.high_efficiency {
            self.packet_clock
        } else {
            let end = header.packet_bytes - usize::from(header.deleted_null_packets);
            Clock::parse(&self.packet[PACKET..end], self.origin)
        };
        output.packet(packet, clock)?;
        report.packets += nulls + 1;
        Ok(())
    }
}
