pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn read(&mut self, n: usize) -> Option<u32> {
        debug_assert!(n <= 32);
        if self.pos + n > self.data.len() * 8 {
            return None;
        }
        let mut v: u32 = 0;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - self.pos % 8)) & 1;
            v = (v << 1) | bit as u32;
            self.pos += 1;
        }
        Some(v)
    }
}

pub fn sign_extend(v: u32, n: usize) -> i32 {
    debug_assert!((1..=32).contains(&n));
    let shift = 32 - n;
    ((v << shift) as i32) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first() {
        let mut r = BitReader::new(&[0b1010_1100, 0b0101_0011]);
        assert_eq!(r.read(3), Some(0b101));
        assert_eq!(r.read(5), Some(0b01100));
        assert_eq!(r.read(8), Some(0b0101_0011));
        assert_eq!(r.read(1), None);
    }

    #[test]
    fn sign_extension() {
        assert_eq!(sign_extend(0b1_1111_1111_1111_1111_1111, 21), -1);
        assert_eq!(sign_extend(0b0_1111_1111_1111_1111_1111, 21), 0xF_FFFF);
        assert_eq!(sign_extend(0x800, 12), -2048);
    }
}
