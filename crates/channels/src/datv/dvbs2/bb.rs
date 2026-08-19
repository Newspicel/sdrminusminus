use sdrmm_dsp::{DVB_DISPERSAL, Prbs};

use crate::datv::dvbs::{PACKET, SYNC};

pub const HEADER_BYTES: usize = 10;
#[cfg(any(test, feature = "test-signals"))]
pub const HEADER_BITS: usize = HEADER_BYTES * 8;
pub const USER_PACKET_BITS: usize = PACKET * 8;
const CRC_POLY: u8 = 0xD5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamKind {
    GenericPacketized,
    GenericContinuous,
    HighEfficiency,
    #[default]
    Transport,
}

impl StreamKind {
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code & 0b11 {
            0b00 => Self::GenericPacketized,
            0b01 => Self::GenericContinuous,
            0b10 => Self::HighEfficiency,
            _ => Self::Transport,
        }
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::GenericPacketized => 0b00,
            Self::GenericContinuous => 0b01,
            Self::HighEfficiency => 0b10,
            Self::Transport => 0b11,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::GenericPacketized => "generic packets",
            Self::GenericContinuous => "GSE",
            Self::HighEfficiency => "GSE-HEM",
            Self::Transport => "MPEG-TS",
        }
    }

    #[must_use]
    pub const fn is_encapsulated(self) -> bool {
        matches!(self, Self::GenericContinuous | Self::HighEfficiency)
    }
}

#[must_use]
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                crc << 1 ^ CRC_POLY
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseBandHeader {
    pub kind: StreamKind,
    pub single: bool,
    pub constant: bool,
    pub isi: u8,
    pub user_packet_bits: u16,
    pub data_field_bits: u16,
    pub sync: u8,
    pub sync_distance: u16,
    pub roll_off: u8,
}

impl Default for BaseBandHeader {
    fn default() -> Self {
        Self {
            kind: StreamKind::Transport,
            single: true,
            constant: true,
            isi: 0,
            user_packet_bits: USER_PACKET_BITS as u16,
            data_field_bits: 0,
            sync: SYNC,
            sync_distance: 0,
            roll_off: 0,
        }
    }
}

impl BaseBandHeader {
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_BYTES || crc8(&bytes[..HEADER_BYTES - 1]) != bytes[HEADER_BYTES - 1]
        {
            return None;
        }
        let single = bytes[0] >> 5 & 1 == 1;
        Some(Self {
            kind: StreamKind::from_code(bytes[0] >> 6),
            single,
            constant: bytes[0] >> 4 & 1 == 1,
            isi: if single { 0 } else { bytes[1] },
            user_packet_bits: u16::from_be_bytes([bytes[2], bytes[3]]),
            data_field_bits: u16::from_be_bytes([bytes[4], bytes[5]]),
            sync: bytes[6],
            sync_distance: u16::from_be_bytes([bytes[7], bytes[8]]),
            roll_off: bytes[0] & 0x03,
        })
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub fn bytes(self) -> [u8; HEADER_BYTES] {
        let mut out = [0u8; HEADER_BYTES];
        out[0] = self.kind.code() << 6
            | u8::from(self.single) << 5
            | u8::from(self.constant) << 4
            | self.roll_off;
        out[1] = if self.single { 0 } else { self.isi };
        out[2..4].copy_from_slice(&self.user_packet_bits.to_be_bytes());
        out[4..6].copy_from_slice(&self.data_field_bits.to_be_bytes());
        out[6] = self.sync;
        out[7..9].copy_from_slice(&self.sync_distance.to_be_bytes());
        out[HEADER_BYTES - 1] = crc8(&out[..HEADER_BYTES - 1]);
        out
    }
}

pub fn scramble(bits: &mut [bool]) {
    Prbs::new(DVB_DISPERSAL).apply_bits(bits);
}

#[must_use]
pub fn pack(bits: &[bool]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk.iter().enumerate().fold(0u8, |byte, (index, &bit)| {
                byte | u8::from(bit) << (7 - index)
            })
        })
        .collect()
}

#[cfg(any(test, feature = "test-signals"))]
pub fn unpack(bytes: &[u8], out: &mut Vec<bool>) {
    for &byte in bytes {
        for shift in (0..8).rev() {
            out.push(byte >> shift & 1 == 1);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseBandData {
    pub header: BaseBandHeader,
    pub field: Vec<u8>,
}

impl BaseBandData {
    #[must_use]
    pub fn transport(&self) -> Vec<[u8; PACKET]> {
        if self.header.kind != StreamKind::Transport
            || usize::from(self.header.user_packet_bits) != USER_PACKET_BITS
            || self.header.sync != SYNC
        {
            return Vec::new();
        }
        let start = usize::from(self.header.sync_distance) / 8;
        self.field[start.min(self.field.len())..]
            .chunks(PACKET)
            .filter(|chunk| chunk.len() == PACKET)
            .map(|chunk| {
                let mut packet = [0u8; PACKET];
                packet.copy_from_slice(chunk);
                packet[0] = SYNC;
                packet
            })
            .collect()
    }
}

pub struct BaseBandFrame {
    length: usize,
}

impl BaseBandFrame {
    #[must_use]
    pub const fn new(length: usize) -> Self {
        Self { length }
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub const fn capacity(&self) -> usize {
        (self.length - HEADER_BITS) / USER_PACKET_BITS
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub const fn field_bytes(&self) -> usize {
        (self.length - HEADER_BITS) / 8
    }

    #[cfg(any(test, feature = "test-signals"))]
    fn assemble(&self, header: BaseBandHeader, body: &[bool]) -> Vec<bool> {
        let mut bits = Vec::with_capacity(self.length);
        unpack(&header.bytes(), &mut bits);
        bits.extend_from_slice(body);
        bits.resize(self.length, false);
        scramble(&mut bits);
        bits
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub fn build(&self, packets: &[[u8; PACKET]], carry: &mut u8) -> Option<Vec<bool>> {
        let count = packets.len().min(self.capacity());
        if count == 0 {
            return None;
        }
        let mut body = Vec::with_capacity(count * USER_PACKET_BITS);
        for packet in &packets[..count] {
            unpack(&[*carry], &mut body);
            unpack(&packet[1..], &mut body);
            *carry = crc8(&packet[1..]);
        }
        Some(self.assemble(
            BaseBandHeader {
                data_field_bits: (count * USER_PACKET_BITS) as u16,
                ..BaseBandHeader::default()
            },
            &body,
        ))
    }

    #[cfg(any(test, feature = "test-signals"))]
    #[must_use]
    pub fn encapsulate(&self, field: &[u8], isi: Option<u8>) -> Option<Vec<bool>> {
        if field.len() > self.field_bytes() {
            return None;
        }
        let mut body = Vec::with_capacity(field.len() * 8);
        unpack(field, &mut body);
        Some(self.assemble(
            BaseBandHeader {
                kind: StreamKind::GenericContinuous,
                single: isi.is_none(),
                isi: isi.unwrap_or(0),
                user_packet_bits: 0,
                data_field_bits: (field.len() * 8) as u16,
                sync: 0,
                sync_distance: 0,
                ..BaseBandHeader::default()
            },
            &body,
        ))
    }

    #[must_use]
    pub fn read(&self, bits: &[bool]) -> Option<BaseBandData> {
        if bits.len() < self.length {
            return None;
        }
        let mut frame = bits[..self.length].to_vec();
        scramble(&mut frame);
        let bytes = pack(&frame);
        let header = BaseBandHeader::parse(&bytes)?;
        let end = HEADER_BYTES + usize::from(header.data_field_bits).div_ceil(8);
        if end > bytes.len() {
            return None;
        }
        Some(BaseBandData {
            header,
            field: bytes[HEADER_BYTES..end].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(count: usize, seed: u32) -> Vec<[u8; PACKET]> {
        let mut state = seed | 1;
        (0..count)
            .map(|index| {
                let mut packet = [0u8; PACKET];
                packet[0] = SYNC;
                packet[1] = 0x41;
                packet[2] = index as u8;
                for byte in &mut packet[3..] {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    *byte = state as u8;
                }
                packet
            })
            .collect()
    }

    #[test]
    fn the_header_crc_covers_the_first_nine_bytes() {
        let header = BaseBandHeader {
            data_field_bits: 15_040,
            roll_off: 1,
            ..BaseBandHeader::default()
        };
        let bytes = header.bytes();
        assert_eq!(BaseBandHeader::parse(&bytes), Some(header));
        let mut damaged = bytes;
        damaged[4] ^= 0x01;
        assert!(BaseBandHeader::parse(&damaged).is_none());
    }

    #[test]
    fn every_stream_kind_and_stream_identifier_survives_the_header() {
        for kind in [
            StreamKind::GenericPacketized,
            StreamKind::GenericContinuous,
            StreamKind::HighEfficiency,
            StreamKind::Transport,
        ] {
            assert_eq!(StreamKind::from_code(kind.code()), kind);
            for isi in [0u8, 1, 137, 255] {
                for constant in [true, false] {
                    let header = BaseBandHeader {
                        kind,
                        single: false,
                        constant,
                        isi,
                        data_field_bits: 1_024,
                        ..BaseBandHeader::default()
                    };
                    assert_eq!(BaseBandHeader::parse(&header.bytes()), Some(header));
                }
            }
        }
        let single = BaseBandHeader {
            single: true,
            isi: 0,
            ..BaseBandHeader::default()
        };
        assert_eq!(BaseBandHeader::parse(&single.bytes()), Some(single));
    }

    #[test]
    fn a_baseband_frame_round_trips_its_transport_packets() {
        let frame = BaseBandFrame::new(32_208);
        let packets = transport(frame.capacity(), 5);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_eq!(bits.len(), 32_208);
        let read = frame.read(&bits).expect("a readable frame");
        assert_eq!(read.header.kind, StreamKind::Transport);
        assert_eq!(read.transport(), packets);
    }

    #[test]
    fn a_partly_filled_frame_carries_only_the_packets_it_was_given() {
        let frame = BaseBandFrame::new(32_208);
        assert_eq!(frame.capacity(), 21);
        let packets = transport(4, 7);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_eq!(
            frame.read(&bits).expect("a readable frame").transport(),
            packets
        );
    }

    #[test]
    fn an_encapsulated_field_comes_back_byte_for_byte() {
        let frame = BaseBandFrame::new(32_208);
        let field: Vec<u8> = (0..1_000).map(|index| (index * 7) as u8).collect();
        let bits = frame.encapsulate(&field, Some(3)).expect("a frame");
        let read = frame.read(&bits).expect("a readable frame");
        assert_eq!(read.header.kind, StreamKind::GenericContinuous);
        assert!(!read.header.single);
        assert_eq!(read.header.isi, 3);
        assert_eq!(read.field, field);
        assert!(read.transport().is_empty());
        assert!(frame.encapsulate(&vec![0; 10_000], None).is_none());
    }

    #[test]
    fn scrambling_hides_the_header_and_is_its_own_inverse() {
        let frame = BaseBandFrame::new(7_032);
        let packets = transport(2, 11);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_ne!(pack(&bits)[6], SYNC);
        assert_eq!(
            frame.read(&bits).expect("a readable frame").transport(),
            packets
        );
    }

    #[test]
    fn a_frame_of_noise_is_refused() {
        let frame = BaseBandFrame::new(7_032);
        let mut state = 0x7f4a_7c15u32;
        let bits: Vec<bool> = (0..7_032)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state & 1 == 1
            })
            .collect();
        assert!(frame.read(&bits).is_none());
    }
}
