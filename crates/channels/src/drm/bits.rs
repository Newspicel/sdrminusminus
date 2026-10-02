#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
    end: usize,
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            end: bytes.len() * 8,
        }
    }

    #[must_use]
    pub fn position(&self) -> usize {
        self.at
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.end.saturating_sub(self.at)
    }

    pub fn seek(&mut self, position: usize) {
        self.at = position.min(self.end);
    }

    #[must_use]
    pub fn bit_at(&self, position: usize) -> bool {
        self.bytes[position / 8] >> (7 - position % 8) & 1 == 1
    }

    pub fn bit(&mut self) -> Option<bool> {
        if self.at >= self.end {
            return None;
        }
        let bit = self.bit_at(self.at);
        self.at += 1;
        Some(bit)
    }

    pub fn read(&mut self, width: u32) -> Option<u32> {
        if width > 32 || self.remaining() < width as usize {
            return None;
        }
        let mut value = 0u32;
        for _ in 0..width {
            value = value << 1 | u32::from(self.bit_at(self.at));
            self.at += 1;
        }
        Some(value)
    }

    pub fn flag(&mut self) -> Option<bool> {
        self.bit()
    }

    pub fn skip(&mut self, width: usize) -> Option<()> {
        (self.remaining() >= width).then(|| self.at += width)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    used: usize,
}

impl BitWriter {
    #[must_use]
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(bytes),
            used: 0,
        }
    }

    #[must_use]
    pub fn bit_len(&self) -> usize {
        self.used
    }

    pub fn bit(&mut self, bit: bool) {
        if self.used.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if bit {
            self.bytes[self.used / 8] |= 1 << (7 - self.used % 8);
        }
        self.used += 1;
    }

    pub fn put(&mut self, value: u32, width: u32) {
        for shift in (0..width).rev() {
            self.bit(value >> shift & 1 == 1);
        }
    }

    pub fn copy(&mut self, source: &BitReader<'_>, from: usize, to: usize) {
        for position in from..to {
            self.bit(source.bit_at(position));
        }
    }

    pub fn align(&mut self) {
        while !self.used.is_multiple_of(8) {
            self.bit(false);
        }
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[must_use]
pub fn crc(poly: u32, width: u32, bits: impl IntoIterator<Item = bool>) -> u32 {
    let mask = (1u32 << width) - 1;
    let top = 1u32 << (width - 1);
    let mut register = mask;
    for bit in bits {
        let feedback = (register & top != 0) != bit;
        register = (register << 1) & mask;
        if feedback {
            register ^= poly & mask;
        }
    }
    !register & mask
}

pub fn byte_bits(bytes: &[u8]) -> impl Iterator<Item = bool> + '_ {
    bytes
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |shift| byte >> shift & 1 == 1))
}

pub fn pack(bits: &[bool], out: &mut Vec<u8>) {
    out.clear();
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (index, &bit) in chunk.iter().enumerate() {
            byte |= u8::from(bit) << (7 - index);
        }
        out.push(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_matches_known_vectors() {
        assert_eq!(crc(0x1021, 16, byte_bits(b"123456789")), 0xD64E);
        let mut reader = BitReader::new(&[0b1011_0011, 0xFF]);
        assert_eq!(reader.read(3), Some(0b101));
        assert_eq!(reader.read(9), Some(0b1_0011_1111));
        assert_eq!(reader.remaining(), 4);
    }

    #[test]
    fn written_bits_read_back() {
        let mut writer = BitWriter::default();
        writer.put(0b101, 3);
        writer.put(0x3FF, 10);
        writer.align();
        let bytes = writer.into_bytes();
        let mut reader = BitReader::new(&bytes);
        assert_eq!(reader.read(3), Some(0b101));
        assert_eq!(reader.read(10), Some(0x3FF));
        assert_eq!(bytes.len(), 2);
    }
}
