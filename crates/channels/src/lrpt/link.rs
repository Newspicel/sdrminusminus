use sdrmm_dsp::fec::conv7::ConvCode;

pub const ASM: u32 = 0x1ACF_FC1D;
pub const ASM_BYTES: usize = 4;
pub const CADU_BYTES: usize = 1024;
pub const CODED_BYTES: usize = CADU_BYTES - ASM_BYTES;
pub const INTERLEAVE: usize = 4;
pub const VCDU_BYTES: usize = 892;
pub const CADU_BITS: usize = CADU_BYTES * 8;
pub const CODED_BITS: usize = CADU_BITS * 2;
pub const ASM_CODED_BITS: usize = ASM_BYTES * 8 * 2;
pub const ENCODER_MEMORY: usize = 6;

pub const VCDU_HEADER: usize = 6;
pub const INSERT_ZONE: usize = 2;
pub const MPDU_HEADER: usize = 2;
pub const MPDU_DATA: usize = VCDU_BYTES - VCDU_HEADER - INSERT_ZONE - MPDU_HEADER;
pub const NO_PACKET_START: u16 = 0x7FF;
pub const SPACECRAFT_ID: u8 = 0x93;
pub const IMAGE_VCID: u8 = 5;
pub const FILL_VCID: u8 = 63;

pub const PACKET_HEADER: usize = 6;
pub const SEQUENCE_MODULO: u32 = 1 << 14;

const POLY_FIRST: u16 = 0o171;
const POLY_SECOND: u16 = 0o133;

#[must_use]
pub fn conv_code() -> ConvCode {
    ConvCode::new(&[POLY_FIRST, POLY_SECOND])
}

#[must_use]
pub fn pn_sequence() -> [u8; CODED_BYTES] {
    let mut bits = [true; 8 * CODED_BYTES];
    for index in 8..bits.len() {
        bits[index] = bits[index - 1] ^ bits[index - 3] ^ bits[index - 5] ^ bits[index - 8];
    }
    std::array::from_fn(|byte| {
        bits[byte * 8..byte * 8 + 8]
            .iter()
            .fold(0u8, |acc, &bit| acc << 1 | u8::from(bit))
    })
}

pub fn asm_bits() -> impl Iterator<Item = bool> {
    (0..32).rev().map(|shift| ASM >> shift & 1 == 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PairMap {
    pub swap: bool,
    pub negate_first: bool,
    pub negate_second: bool,
}

impl PairMap {
    pub const ALL: [Self; 8] = {
        let mut all = [Self {
            swap: false,
            negate_first: false,
            negate_second: false,
        }; 8];
        let mut index = 0;
        while index < 8 {
            all[index] = Self {
                swap: index & 4 != 0,
                negate_first: index & 1 != 0,
                negate_second: index & 2 != 0,
            };
            index += 1;
        }
        all
    };

    #[must_use]
    pub fn apply<T: Copy + std::ops::Neg<Output = T>>(self, first: T, second: T) -> (T, T) {
        let (a, b) = if self.swap {
            (second, first)
        } else {
            (first, second)
        };
        (
            if self.negate_first { -a } else { a },
            if self.negate_second { -b } else { b },
        )
    }

    fn apply_bits(self, first: bool, second: bool) -> (bool, bool) {
        let (a, b) = if self.swap {
            (second, first)
        } else {
            (first, second)
        };
        (a ^ self.negate_first, b ^ self.negate_second)
    }

    fn preimage(self, wanted: (bool, bool)) -> (bool, bool) {
        [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .find(|&(a, b)| self.apply_bits(a, b) == wanted)
            .unwrap_or(wanted)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AsmPattern {
    pub map: PairMap,
    pub value: u64,
    pub mask: u64,
}

impl AsmPattern {
    #[must_use]
    pub fn mismatches(&self, window: u64) -> u32 {
        ((window ^ self.value) & self.mask).count_ones()
    }
}

#[must_use]
pub fn coded_asm() -> u64 {
    let code = conv_code();
    let bits: Vec<bool> = asm_bits().collect();
    let mut coded = Vec::with_capacity(ASM_CODED_BITS);
    code.encode(&bits, &mut coded);
    coded
        .iter()
        .fold(0u64, |acc, &bit| acc << 1 | u64::from(bit))
}

#[must_use]
pub fn asm_patterns() -> [AsmPattern; 8] {
    let coded = coded_asm();
    let mask = u64::MAX >> (2 * ENCODER_MEMORY);
    PairMap::ALL.map(|map| {
        let mut value = 0u64;
        for pair in 0..ASM_CODED_BITS / 2 {
            let shift = ASM_CODED_BITS - 2 - 2 * pair;
            let wanted = (coded >> (shift + 1) & 1 == 1, coded >> shift & 1 == 1);
            let (a, b) = map.preimage(wanted);
            value |= (u64::from(a) << 1 | u64::from(b)) << shift;
        }
        AsmPattern {
            map,
            value: value & mask,
            mask,
        }
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VcduHeader {
    pub vcid: u8,
    pub counter: u32,
    pub first_header: u16,
}

impl VcduHeader {
    #[must_use]
    pub fn parse(vcdu: &[u8]) -> Self {
        Self {
            vcid: vcdu[1] & 0x3F,
            counter: u32::from(vcdu[2]) << 16 | u32::from(vcdu[3]) << 8 | u32::from(vcdu[4]),
            first_header: u16::from(vcdu[8] & 0x07) << 8 | u16::from(vcdu[9]),
        }
    }

    pub fn write(&self, vcdu: &mut [u8]) {
        vcdu[0] = 0x40 | SPACECRAFT_ID >> 2;
        vcdu[1] = (SPACECRAFT_ID & 0x03) << 6 | self.vcid & 0x3F;
        vcdu[2] = (self.counter >> 16) as u8;
        vcdu[3] = (self.counter >> 8) as u8;
        vcdu[4] = self.counter as u8;
        vcdu[5] = 0;
        vcdu[6] = 0;
        vcdu[7] = 0;
        vcdu[8] = (self.first_header >> 8) as u8 & 0x07;
        vcdu[9] = self.first_header as u8;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub apid: u16,
    pub sequence: u16,
    pub length: usize,
}

impl PacketHeader {
    #[must_use]
    pub fn parse(header: &[u8]) -> Self {
        Self {
            apid: u16::from(header[0] & 0x07) << 8 | u16::from(header[1]),
            sequence: u16::from(header[2] & 0x3F) << 8 | u16::from(header[3]),
            length: (usize::from(header[4]) << 8 | usize::from(header[5])) + 1,
        }
    }

    pub fn write(&self, header: &mut [u8]) {
        header[0] = 0x08 | (self.apid >> 8) as u8 & 0x07;
        header[1] = self.apid as u8;
        header[2] = 0xC0 | (self.sequence >> 8) as u8 & 0x3F;
        header[3] = self.sequence as u8;
        let length = self.length.saturating_sub(1);
        header[4] = (length >> 8) as u8;
        header[5] = length as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pn_sequence_starts_like_ccsds() {
        let pn = pn_sequence();
        assert_eq!(&pn[..8], &[0xFF, 0x48, 0x0E, 0xC0, 0x9A, 0x0D, 0x70, 0xBC]);
        assert_eq!(pn[255..263], pn[..8]);
    }

    #[test]
    fn the_coded_asm_matches_the_meteor_correlator_word() {
        let mask = u64::MAX >> (2 * ENCODER_MEMORY);
        assert_eq!(coded_asm() & mask, !0xFCA2_B63D_B00D_9794u64 & mask);
    }

    #[test]
    fn every_pair_map_gives_a_distinct_pattern() {
        let patterns = asm_patterns();
        for (index, a) in patterns.iter().enumerate() {
            for b in &patterns[index + 1..] {
                assert!(a.mismatches(b.value) >= 20);
            }
        }
    }

    #[test]
    fn headers_round_trip() {
        let mut vcdu = [0u8; 10];
        let header = VcduHeader {
            vcid: IMAGE_VCID,
            counter: 0x12_3456,
            first_header: 0x2AB,
        };
        header.write(&mut vcdu);
        assert_eq!(VcduHeader::parse(&vcdu), header);
        let mut bytes = [0u8; 6];
        let packet = PacketHeader {
            apid: 65,
            sequence: 0x3FFF,
            length: 900,
        };
        packet.write(&mut bytes);
        assert_eq!(PacketHeader::parse(&bytes), packet);
    }
}
