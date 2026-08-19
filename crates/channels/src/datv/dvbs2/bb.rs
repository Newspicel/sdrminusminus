use sdrmm_dsp::{DVB_DISPERSAL, Prbs};

use crate::datv::dvbs::{PACKET, SYNC};

pub const HEADER_BYTES: usize = 10;
pub const HEADER_BITS: usize = HEADER_BYTES * 8;
pub const USER_PACKET_BITS: usize = PACKET * 8;
const CRC_POLY: u8 = 0xD5;
const TRANSPORT_STREAM: u8 = 0b11;

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
    pub user_packet_bits: u16,
    pub data_field_bits: u16,
    pub sync: u8,
    pub sync_distance: u16,
    pub roll_off: u8,
}

impl BaseBandHeader {
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < HEADER_BYTES || crc8(&bytes[..HEADER_BYTES - 1]) != bytes[HEADER_BYTES - 1]
        {
            return None;
        }
        if bytes[0] >> 6 != TRANSPORT_STREAM {
            return None;
        }
        Some(Self {
            user_packet_bits: u16::from_be_bytes([bytes[2], bytes[3]]),
            data_field_bits: u16::from_be_bytes([bytes[4], bytes[5]]),
            sync: bytes[6],
            sync_distance: u16::from_be_bytes([bytes[7], bytes[8]]),
            roll_off: bytes[0] & 0x03,
        })
    }

    #[must_use]
    pub fn bytes(self) -> [u8; HEADER_BYTES] {
        let mut out = [0u8; HEADER_BYTES];
        out[0] = TRANSPORT_STREAM << 6 | 0b11 << 4 | self.roll_off;
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

pub fn unpack(bytes: &[u8], out: &mut Vec<bool>) {
    for &byte in bytes {
        for shift in (0..8).rev() {
            out.push(byte >> shift & 1 == 1);
        }
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

    #[must_use]
    pub const fn capacity(&self) -> usize {
        (self.length - HEADER_BITS) / USER_PACKET_BITS
    }

    #[must_use]
    pub fn build(&self, packets: &[[u8; PACKET]], carry: &mut u8) -> Option<Vec<bool>> {
        let count = packets.len().min(self.capacity());
        if count == 0 {
            return None;
        }
        let header = BaseBandHeader {
            user_packet_bits: USER_PACKET_BITS as u16,
            data_field_bits: (count * USER_PACKET_BITS) as u16,
            sync: SYNC,
            sync_distance: 0,
            roll_off: 0,
        };
        let mut bits = Vec::with_capacity(self.length);
        unpack(&header.bytes(), &mut bits);
        for packet in &packets[..count] {
            unpack(&[*carry], &mut bits);
            unpack(&packet[1..], &mut bits);
            *carry = crc8(&packet[1..]);
        }
        bits.resize(self.length, false);
        scramble(&mut bits);
        Some(bits)
    }

    #[must_use]
    pub fn read(&self, bits: &[bool]) -> Option<Vec<[u8; PACKET]>> {
        if bits.len() < self.length {
            return None;
        }
        let mut frame = bits[..self.length].to_vec();
        scramble(&mut frame);
        let bytes = pack(&frame);
        let header = BaseBandHeader::parse(&bytes)?;
        if usize::from(header.user_packet_bits) != USER_PACKET_BITS || header.sync != SYNC {
            return None;
        }
        let start = HEADER_BYTES + usize::from(header.sync_distance) / 8;
        let end = HEADER_BYTES + usize::from(header.data_field_bits) / 8;
        if start > end || end > bytes.len() {
            return None;
        }
        Some(
            bytes[start..end]
                .chunks(PACKET)
                .filter(|chunk| chunk.len() == PACKET)
                .map(|chunk| {
                    let mut packet = [0u8; PACKET];
                    packet.copy_from_slice(chunk);
                    packet[0] = SYNC;
                    packet
                })
                .collect(),
        )
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
            user_packet_bits: 1_504,
            data_field_bits: 15_040,
            sync: SYNC,
            sync_distance: 0,
            roll_off: 1,
        };
        let bytes = header.bytes();
        assert_eq!(BaseBandHeader::parse(&bytes), Some(header));
        let mut damaged = bytes;
        damaged[4] ^= 0x01;
        assert!(BaseBandHeader::parse(&damaged).is_none());
    }

    #[test]
    fn a_baseband_frame_round_trips_its_transport_packets() {
        let frame = BaseBandFrame::new(32_208);
        let packets = transport(frame.capacity(), 5);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_eq!(bits.len(), 32_208);
        let read = frame.read(&bits).expect("a readable frame");
        assert_eq!(read, packets);
    }

    #[test]
    fn a_partly_filled_frame_carries_only_the_packets_it_was_given() {
        let frame = BaseBandFrame::new(32_208);
        assert_eq!(frame.capacity(), 21);
        let packets = transport(4, 7);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_eq!(frame.read(&bits).expect("a readable frame"), packets);
    }

    #[test]
    fn scrambling_hides_the_header_and_is_its_own_inverse() {
        let frame = BaseBandFrame::new(7_032);
        let packets = transport(2, 11);
        let mut carry = SYNC;
        let bits = frame.build(&packets, &mut carry).expect("a frame");
        assert_ne!(pack(&bits)[6], SYNC);
        assert_eq!(frame.read(&bits).expect("a readable frame"), packets);
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
