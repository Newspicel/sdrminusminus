const LFSR15_INIT: [u8; 15] = [1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1];

#[derive(Clone, Debug)]
pub struct Scrambler {
    state: [u8; 15],
}

impl Scrambler {
    pub fn new() -> Self {
        Self { state: LFSR15_INIT }
    }

    #[inline]
    pub fn next_bit(&mut self) -> u8 {
        let out = self.state[0] ^ self.state[14];
        self.state.rotate_right(1);
        self.state[0] = out;
        out
    }

    #[cfg(test)]
    pub fn apply(&mut self, bits: &mut [u8]) {
        for b in bits {
            *b ^= self.next_bit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Scrambler;

    #[test]
    fn first_three_bits_keep_reserved_symbol_zero() {
        let mut s = Scrambler::new();
        let mut reserved = [0u8, 0, 0];
        s.apply(&mut reserved);
        assert_eq!(reserved, [0, 0, 0]);
    }

    #[test]
    fn keystream_matches_spec_derivation() {
        let expected = "000100110001101111000100001001010000111110001100";
        let mut s = Scrambler::new();
        let got: String = (0..48).map(|_| char::from(b'0' + s.next_bit())).collect();
        assert_eq!(got, expected);
    }
}
