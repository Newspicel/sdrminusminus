//! Block codes the digital-voice signalling layers are built from ( wave 3).
/// A systematic single-error-correcting code defined by one parity mask per check bit.
///
/// Bit order is the specification's: `word[0..k]` are the information bits, `word[k..n]` the
/// parity bits, in transmission order.
#[derive(Clone, Copy, Debug)]
pub struct ParityCode {
    k: usize,
    /// One mask per parity bit; bit `i` set means information bit `i` is in that parity sum.
    parity: &'static [u16],
}

impl ParityCode {
    /// Hamming(7,4,3) protecting the DMR CACH TACT field.
    pub const HAMMING_7_4: Self = Self {
        k: 4,
        parity: &[0b0111, 0b1110, 0b1011],
    };

    /// Hamming(13,9,3) — the columns of a DMR BPTC(196,96) block (ETSI TS 102 361-1 B.3.11).
    pub const HAMMING_13_9: Self = Self {
        k: 9,
        parity: &[0b0_0110_1011, 0b0_1101_0111, 0b1_1010_1111, 0b1_0011_0101],
    };

    /// Hamming(15,11,3) — the rows of a DMR BPTC(196,96) block (ETSI TS 102 361-1 B.3.12).
    pub const HAMMING_15_11: Self = Self {
        k: 11,
        parity: &[
            0b001_1010_1111,
            0b011_0101_1110,
            0b110_1011_1100,
            0b100_1101_0111,
        ],
    };

    /// Hamming(16,11,4) — the same code with an extra check bit, protecting the rows of the
    /// BPTC(128,77) block a DMR voice burst carries its embedded link control in.
    pub const HAMMING_16_11: Self = Self {
        k: 11,
        parity: &[
            0b001_1010_1111,
            0b011_0101_1110,
            0b110_1011_1100,
            0b100_1101_0111,
            0b111_0110_0101,
        ],
    };

    pub const HAMMING_10_6: Self = Self {
        k: 6,
        parity: &[0b10_0111, 0b10_1011, 0b01_1101, 0b01_1110],
    };

    pub const HAMMING_17_12: Self = Self {
        k: 12,
        parity: &[
            0b0010_1100_1111,
            0b0101_1001_1111,
            0b1011_0011_1110,
            0b0100_1011_0011,
            0b1001_0110_0111,
        ],
    };

    /// Information bits carried.
    #[must_use]
    pub const fn k(&self) -> usize {
        self.k
    }

    /// Codeword length in bits.
    #[must_use]
    pub const fn n(&self) -> usize {
        self.k + self.parity.len()
    }

    /// Fill `word[k..n]` from the information bits in `word[..k]`.
    ///
    /// # Panics
    /// If `word` is shorter than [`ParityCode::n`].
    pub fn encode(&self, word: &mut [bool]) {
        assert!(word.len() >= self.n(), "word shorter than the codeword");
        for (j, &mask) in self.parity.iter().enumerate() {
            word[self.k + j] = self.sum(word, mask);
        }
    }

    /// Correct `word` in place, returning the number of bits repaired, or `None` when the
    /// syndrome names no single-bit error the code can locate — for the distance-4 member that
    /// is a detected double error, for the distance-3 ones an uncorrectable pattern.
    ///
    /// # Panics
    /// If `word` is shorter than [`ParityCode::n`].
    pub fn decode(&self, word: &mut [bool]) -> Option<u32> {
        assert!(word.len() >= self.n(), "word shorter than the codeword");
        let mut syndrome = 0u32;
        for (j, &mask) in self.parity.iter().enumerate() {
            if self.sum(word, mask) != word[self.k + j] {
                syndrome |= 1 << j;
            }
        }
        if syndrome == 0 {
            return Some(0);
        }
        if syndrome.count_ones() == 1 {
            let j = syndrome.trailing_zeros() as usize;
            word[self.k + j] = !word[self.k + j];
            return Some(1);
        }
        // Otherwise the syndrome is the check-matrix column of the flipped information bit.
        for (i, bit) in word.iter_mut().enumerate().take(self.k) {
            if self.column(i) == syndrome {
                *bit = !*bit;
                return Some(1);
            }
        }
        None
    }

    fn sum(&self, word: &[bool], mask: u16) -> bool {
        let mut acc = false;
        for (i, &bit) in word.iter().enumerate().take(self.k) {
            acc ^= mask >> i & 1 == 1 && bit;
        }
        acc
    }

    /// The check-matrix column for information bit `i`: which parity sums it takes part in.
    fn column(&self, i: usize) -> u32 {
        self.parity
            .iter()
            .enumerate()
            .filter(|(_, mask)| *mask >> i & 1 == 1)
            .fold(0, |acc, (j, _)| acc | 1 << j)
    }
}

/// A shortened cyclic code with an appended even-parity bit, decoded by exhaustive search.
///
/// The codeword is `info` (`k` bits, first transmitted) then `info · x^parity mod gen`
/// (`parity` bits) then one bit making the whole word even weight. Words are `u32`, MSB of the
/// `n`-bit word first on the wire.
#[derive(Clone, Copy, Debug)]
pub struct CyclicCode {
    k: u32,
    parity: u32,
    generator: u64,
    correctable: u32,
}

/// Longest message any code here carries, so the Gray-code walk needs no allocation.
const MAX_MESSAGE_BITS: usize = 16;

impl CyclicCode {
    /// Golay(20,8,8) — the DMR slot type, carrying colour code and data type either side of the
    /// sync (ETSI TS 102 361-1 B.3.3). Corrects three errors and detects four.
    pub const GOLAY_20_8: Self = Self {
        k: 8,
        parity: 11,
        generator: 0xC75,
        correctable: 3,
    };

    /// Golay(18,6,8) protecting each six-bit symbol in a P25 header data unit.
    pub const GOLAY_18_6: Self = Self {
        k: 6,
        parity: 11,
        generator: 0xC75,
        correctable: 3,
    };

    /// QR(16,7,6) — the DMR embedded signalling field in a voice burst (B.3.4). Corrects two.
    pub const QR_16_7: Self = Self {
        k: 7,
        parity: 8,
        generator: 0x139,
        correctable: 2,
    };

    /// The extended binary Golay(24,12,8) the Golay(20,8) above is shortened from — YSF's FICH
    /// is four of these. Corrects three errors and detects four.
    pub const GOLAY_24_12: Self = Self {
        k: 12,
        parity: 11,
        generator: 0xC75,
        correctable: 3,
    };

    pub const BCH_63_16: Self = Self {
        k: 16,
        parity: 47,
        generator: 0xCD93_0BDD_3B2B,
        correctable: 11,
    };

    /// Codeword length in bits.
    #[must_use]
    pub const fn n(&self) -> u32 {
        self.k + self.parity + 1
    }

    /// Mask of the `n` transmitted bits.
    fn mask(&self) -> u64 {
        let n = self.n();
        if n >= 64 { u64::MAX } else { (1 << n) - 1 }
    }

    /// The `n`-bit codeword for `info`, MSB first.
    #[must_use]
    pub fn encode(&self, info: u32) -> u64 {
        let info = u64::from(info) & ((1 << self.k) - 1);
        let mut rem = info << self.parity;
        let deg = 63 - self.generator.leading_zeros();
        for shift in (self.parity..self.k + self.parity).rev() {
            if rem >> shift & 1 == 1 {
                rem ^= self.generator << (shift - deg);
            }
        }
        let word = (info << (self.parity + 1)) | (rem << 1);
        word | u64::from(word.count_ones() % 2 == 1)
    }

    /// Nearest codeword to `word`, as `(information bits, errors corrected)`. `None` when the
    /// nearest one is further away than the code can correct, which is a detected error rather
    /// than a silent guess.
    ///
    /// The search walks the messages in Gray-code order, so each candidate codeword is one XOR
    /// away from the last: the whole 2^k sweep costs one XOR and one population count per
    /// candidate, which is what makes an exhaustive decode affordable even for the 65 536
    /// messages of P25's BCH.
    #[must_use]
    pub fn decode(&self, word: u64) -> Option<(u32, u32)> {
        let word = word & self.mask();
        // The code is linear, so the codeword of a message is the XOR of the codewords of its
        // set bits.
        let mut rows = [0u64; MAX_MESSAGE_BITS];
        for (i, row) in rows.iter_mut().enumerate().take(self.k as usize) {
            *row = self.encode(1 << i);
        }
        let mut codeword = self.encode(0);
        let mut best = (0u32, (codeword ^ word).count_ones());
        for i in 1..1u32 << self.k {
            codeword ^= rows[i.trailing_zeros() as usize];
            let distance = (codeword ^ word).count_ones();
            if distance < best.1 {
                best = (i ^ (i >> 1), distance);
            }
        }
        (best.1 <= self.correctable).then_some(best)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that makes a code the code it claims to be. A wrong parity table still
    /// round-trips against itself; only the distance says whether it is the published one.
    fn min_distance_parity(code: &ParityCode) -> u32 {
        let mut best = u32::MAX;
        for info in 1..1u32 << code.k() {
            let mut word = vec![false; code.n()];
            for (i, bit) in word.iter_mut().enumerate().take(code.k()) {
                *bit = info >> i & 1 == 1;
            }
            code.encode(&mut word);
            best = best.min(word.iter().filter(|b| **b).count() as u32);
        }
        best
    }

    #[test]
    fn the_hamming_family_has_its_published_distances() {
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_13_9), 3);
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_7_4), 3);
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_15_11), 3);
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_16_11), 4);
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_10_6), 3);
        assert_eq!(min_distance_parity(&ParityCode::HAMMING_17_12), 3);
    }

    #[test]
    fn parity_codes_repair_any_single_bit() {
        for code in [
            ParityCode::HAMMING_7_4,
            ParityCode::HAMMING_13_9,
            ParityCode::HAMMING_15_11,
            ParityCode::HAMMING_16_11,
            ParityCode::HAMMING_10_6,
            ParityCode::HAMMING_17_12,
        ] {
            let mut clean = vec![false; code.n()];
            for (i, bit) in clean.iter_mut().enumerate().take(code.k()) {
                *bit = i % 3 == 0;
            }
            code.encode(&mut clean);
            for flip in 0..code.n() {
                let mut word = clean.clone();
                word[flip] = !word[flip];
                assert_eq!(code.decode(&mut word), Some(1), "bit {flip}");
                assert_eq!(word, clean, "bit {flip} was repaired to a different word");
            }
        }
    }

    /// Distance 4 buys detection, not correction: the extended member must refuse a double
    /// error rather than "repair" it into a third wrong word.
    #[test]
    fn the_extended_hamming_detects_double_errors() {
        let code = ParityCode::HAMMING_16_11;
        let mut clean = vec![false; code.n()];
        clean[0] = true;
        clean[4] = true;
        code.encode(&mut clean);
        let mut word = clean.clone();
        word[2] = !word[2];
        word[9] = !word[9];
        assert_eq!(code.decode(&mut word), None);
    }

    #[test]
    fn the_cyclic_codes_have_their_published_distances() {
        for (code, distance) in [(CyclicCode::GOLAY_20_8, 8), (CyclicCode::QR_16_7, 6)] {
            let min = (1..1u32 << code.k)
                .map(|info| code.encode(info).count_ones())
                .min()
                .unwrap();
            assert_eq!(min, distance);
        }
    }

    /// The DMR slot type as ETSI publishes it: eight information bits followed by the remainder
    /// modulo 0xC75 and an even-parity bit. These two are the first entries of the encoding
    /// table every DMR implementation ships, and pin the bit order this code is packed in.
    #[test]
    fn golay_20_8_matches_the_published_slot_type_words() {
        assert_eq!(CyclicCode::GOLAY_20_8.encode(0x01) >> 1 & 0x7FF, 0x475);
        assert_eq!(CyclicCode::GOLAY_20_8.encode(0x02) >> 1 & 0x7FF, 0x49F);
    }

    #[test]
    fn cyclic_codes_correct_up_to_their_limit_and_refuse_beyond_it() {
        for (code, limit) in [(CyclicCode::GOLAY_20_8, 3), (CyclicCode::QR_16_7, 2)] {
            let info = 0b101;
            let clean = code.encode(info);
            for errors in 0..=limit {
                let mut word = clean;
                for bit in 0..errors {
                    word ^= 1 << (bit * 5 % code.n());
                }
                assert_eq!(code.decode(word), Some((info, errors)), "{errors} errors");
            }
            // Both codes have even minimum distance, so one error past the correction limit
            // lands strictly between two codewords: the decoder must say so, not guess.
            let mut word = clean;
            for bit in 0..=limit {
                word ^= 1 << (bit * 5 % code.n());
            }
            assert_eq!(code.decode(word), None, "{} errors", limit + 1);
        }
    }
}
