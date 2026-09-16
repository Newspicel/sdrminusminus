use super::Header;

fn layout(header: Header) -> (usize, usize, usize) {
    if !header.mpeg1 {
        return (30, 4, 11);
    }
    let per_channel = header.bitrate / if header.mono { 1 } else { 2 };
    if per_channel <= 48 {
        return (if header.rate == 32000 { 12 } else { 8 }, 2, 32);
    }
    let bands = if per_channel <= 80 || header.rate == 48000 {
        27
    } else {
        30
    };
    (bands, 11, 23)
}

struct ProtectedBits<'a> {
    bytes: &'a [u8],
    position: usize,
    checksum: u16,
}

impl ProtectedBits<'_> {
    fn read(&mut self, count: usize) -> Option<u8> {
        let mut result = 0;
        for _ in 0..count {
            let bit = (self.bytes.get(self.position / 8)? >> (7 - self.position % 8)) & 1;
            result = result << 1 | bit;
            let feedback = self.checksum >> 15 ^ u16::from(bit);
            self.checksum <<= 1;
            if feedback != 0 {
                self.checksum ^= 0x8005;
            }
            self.position += 1;
        }
        Some(result)
    }
}

fn checksum(bytes: &[u8], header: Header) -> Option<u16> {
    let mut bits = ProtectedBits {
        bytes,
        position: 16,
        checksum: 0xffff,
    };
    bits.read(8)?;
    bits.read(8)?;
    bits.position = 48;
    let (bands, four_bit_end, three_bit_end) = layout(header);
    let channels = if header.mono { 1 } else { 2 };
    let joint_start = if bytes[3] >> 6 == 1 {
        usize::from((bytes[3] >> 4) & 3) * 4 + 4
    } else {
        bands
    };
    let mut allocated = [[false; 2]; 32];
    for (band, allocation) in allocated[..bands].iter_mut().enumerate() {
        let width = if band < four_bit_end {
            4
        } else if band < three_bit_end {
            3
        } else {
            2
        };
        if band < joint_start {
            for channel in &mut allocation[..channels] {
                *channel = bits.read(width)? != 0;
            }
        } else {
            allocation.fill(bits.read(width)? != 0);
        }
    }
    for allocation in &allocated[..bands] {
        for &active in &allocation[..channels] {
            if active {
                bits.read(2)?;
            }
        }
    }
    Some(bits.checksum)
}

pub(super) fn valid(bytes: &[u8], header: Header) -> bool {
    bytes.get(1).is_some_and(|b| b & 1 != 0)
        || bytes.get(4..6).is_some_and(|stored| {
            checksum(bytes, header) == Some(u16::from_be_bytes([stored[0], stored[1]]))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_widths_cover_all_five_tables() {
        for (raw, expected) in [
            ([0xff, 0xfd, 0x44, 0xc0], (27, 11, 23)),
            ([0xff, 0xfd, 0xa0, 0xc0], (30, 11, 23)),
            ([0xff, 0xfd, 0x24, 0xc0], (8, 2, 32)),
            ([0xff, 0xfd, 0x28, 0xc0], (12, 2, 32)),
            ([0xff, 0xf5, 0x44, 0xc0], (30, 4, 11)),
        ] {
            assert_eq!(layout(Header::read(&raw).expect("header")), expected);
        }
    }

    #[test]
    fn protected_silence_rejects_damaged_header_allocation_and_crc() {
        let mut frame = [0; 192];
        frame[..6].copy_from_slice(&[0xff, 0xfc, 0x44, 0xc0, 0xfe, 0xb3]);
        let header = Header::read(&frame).expect("header");
        assert!(valid(&frame, header));
        for byte in [2, 3, 4, 5, 6, 15] {
            let mut damaged = frame;
            damaged[byte] ^= 1;
            assert!(!valid(&damaged, Header::read(&damaged).expect("header")));
        }
        assert!(!valid(&frame[..8], header));
    }
}
