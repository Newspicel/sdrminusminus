use super::bb::{BaseBandData, StreamKind, USER_PACKET_BITS, crc8};
use crate::datv::dvbs::{PACKET, SYNC};

pub struct Transport {
    packet: [u8; PACKET],
    filled: usize,
    synchronized: bool,
}

impl Transport {
    pub const fn new() -> Self {
        Self {
            packet: [0; PACKET],
            filled: 0,
            synchronized: false,
        }
    }

    pub fn reset(&mut self) {
        self.filled = 0;
        self.synchronized = false;
    }

    pub fn push(&mut self, frame: &BaseBandData, output: &mut Vec<[u8; PACKET]>) -> u32 {
        let header = frame.header;
        if header.kind != StreamKind::Transport
            || usize::from(header.user_packet_bits) != USER_PACKET_BITS
            || header.sync != SYNC
            || !header.data_field_bits.is_multiple_of(8)
            || (header.sync_distance != u16::MAX && !header.sync_distance.is_multiple_of(8))
        {
            self.reset();
            return 1;
        }
        let distance = usize::from(header.sync_distance) / 8;
        if header.sync_distance != u16::MAX && distance >= PACKET {
            self.reset();
            return 1;
        }
        let mut errors = 0;
        if self.synchronized
            && header.sync_distance != u16::MAX
            && distance != (PACKET - self.filled) % PACKET
        {
            self.reset();
            errors += 1;
        }
        let start = if self.synchronized {
            0
        } else {
            if header.sync_distance == u16::MAX || distance >= frame.field.len() {
                return errors;
            }
            self.synchronized = true;
            distance
        };
        for &byte in &frame.field[start..] {
            if self.filled == PACKET {
                if crc8(&self.packet[1..]) == byte {
                    output.push(self.packet);
                } else {
                    errors += 1;
                }
                self.filled = 0;
            }
            if self.filled == 0 {
                self.packet[0] = SYNC;
            } else {
                self.packet[self.filled] = byte;
            }
            self.filled += 1;
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datv::dvbs2::bb::BaseBandHeader;

    fn frame(field: &[u8], offset: usize) -> BaseBandData {
        BaseBandData {
            header: BaseBandHeader {
                data_field_bits: (field.len() * 8) as u16,
                sync_distance: (((PACKET - offset % PACKET) % PACKET) * 8) as u16,
                ..Default::default()
            },
            field: field.to_vec(),
        }
    }

    #[test]
    fn packets_cross_every_possible_baseband_boundary_and_crc_failures_are_dropped() {
        let packets: Vec<_> = (0..7)
            .map(|i| {
                let mut packet = [i; PACKET];
                packet[0] = SYNC;
                packet
            })
            .collect();
        let mut adapted = Vec::new();
        let mut carry = 0;
        for packet in &packets {
            adapted.push(carry);
            adapted.extend_from_slice(&packet[1..]);
            carry = crc8(&packet[1..]);
        }
        adapted.push(carry);
        for split in 1..2 * PACKET {
            let mut receiver = Transport::new();
            let mut output = Vec::new();
            assert_eq!(receiver.push(&frame(&adapted[..split], 0), &mut output), 0);
            assert_eq!(
                receiver.push(&frame(&adapted[split..], split), &mut output),
                0
            );
            assert_eq!(output, packets, "split {split}");
        }
        adapted[2 * PACKET + 7] ^= 0x40;
        let mut receiver = Transport::new();
        let mut output = Vec::new();
        assert_eq!(receiver.push(&frame(&adapted, 0), &mut output), 1);
        assert_eq!(output.len(), packets.len() - 1);
        assert!(!output.contains(&packets[2]));
    }

    #[test]
    fn acquisition_discards_the_partial_packet_before_sync_distance() {
        let mut receiver = Transport::new();
        let packet = crate::datv::ts::null_packet();
        let mut bytes = vec![0; 37];
        bytes.push(0);
        bytes.extend_from_slice(&packet[1..]);
        bytes.push(crc8(&packet[1..]));
        let mut data = frame(&bytes, PACKET - 37);
        let mut output = Vec::new();
        assert_eq!(receiver.push(&data, &mut output), 0);
        assert_eq!(output, [packet]);
        data.header.data_field_bits -= 1;
        assert_eq!(receiver.push(&data, &mut output), 1);
    }
}
