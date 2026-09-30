use sdrmm_dsp::golay23_correct;

const INTERLEAVE: [(u8, u8); 72] = [
    (0, 10),
    (0, 22),
    (3, 11),
    (2, 9),
    (1, 10),
    (1, 22),
    (0, 11),
    (0, 23),
    (1, 8),
    (1, 20),
    (0, 9),
    (0, 21),
    (3, 10),
    (2, 8),
    (1, 9),
    (1, 21),
    (3, 8),
    (2, 6),
    (1, 7),
    (1, 19),
    (0, 8),
    (0, 20),
    (3, 9),
    (2, 7),
    (0, 6),
    (0, 18),
    (3, 7),
    (2, 5),
    (1, 6),
    (1, 18),
    (0, 7),
    (0, 19),
    (1, 4),
    (1, 16),
    (0, 5),
    (0, 17),
    (3, 6),
    (2, 4),
    (1, 5),
    (1, 17),
    (3, 4),
    (2, 2),
    (1, 3),
    (1, 15),
    (0, 4),
    (0, 16),
    (3, 5),
    (2, 3),
    (0, 2),
    (0, 14),
    (3, 3),
    (2, 1),
    (1, 2),
    (1, 14),
    (0, 3),
    (0, 15),
    (1, 0),
    (1, 12),
    (0, 1),
    (0, 13),
    (3, 2),
    (2, 0),
    (1, 1),
    (1, 13),
    (3, 0),
    (3, 12),
    (2, 10),
    (1, 11),
    (0, 0),
    (0, 12),
    (3, 1),
    (3, 13),
];

const GOLAY_CHECK_BITS: u32 = 11;
const SCRAMBLED_BITS: u32 = 23;
const INFO_BITS: u32 = 49;
const C1_SHIFT: u32 = 25;
const C0_SHIFT: u32 = C1_SHIFT + 12;
const C2_SHIFT: u32 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Info(u64);

impl Info {
    pub(super) fn field(self, positions: &[u8]) -> usize {
        positions.iter().fold(0, |field, &position| {
            let bit = self.0 >> (INFO_BITS - 1 - u32::from(position)) & 1;
            field << 1 | bit as usize
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Frame {
    pub(super) info: Info,
    pub(super) errors: u32,
}

pub(super) fn decode(air: &[bool; 72]) -> Frame {
    let [c0, c1, c2, c3] = code_vectors(air);
    let (c0, c0_errors) = golay_data(c0 >> 1);
    let (c1, c1_errors) = golay_data(c1 ^ scrambler(c0));
    let info = u64::from(c0) << C0_SHIFT
        | u64::from(c1) << C1_SHIFT
        | u64::from(c2) << C2_SHIFT
        | u64::from(c3);
    Frame {
        info: Info(info),
        errors: c0_errors + c1_errors,
    }
}

fn code_vectors(air: &[bool; 72]) -> [u32; 4] {
    let mut code = [0; 4];
    for (&bit, &(row, column)) in air.iter().zip(&INTERLEAVE) {
        code[usize::from(row)] |= u32::from(bit) << column;
    }
    code
}

fn golay_data(word: u32) -> (u32, u32) {
    let (codeword, _) = golay23_correct(word);
    let data_errors = ((codeword ^ word) >> GOLAY_CHECK_BITS).count_ones();
    (codeword >> GOLAY_CHECK_BITS, data_errors)
}

fn scrambler(seed: u32) -> u32 {
    let mut state = seed << 4;
    (0..SCRAMBLED_BITS).rev().fold(0, |mask, bit| {
        state = (173 * state + 13_849) & 0xFFFF;
        mask | (state >> 15) << bit
    })
}

#[cfg(test)]
pub(super) fn encode(info: Info) -> [bool; 72] {
    use sdrmm_dsp::golay23_encode;

    let c0_data = (info.0 >> C0_SHIFT) as u16 & 0x0FFF;
    let c1_data = (info.0 >> C1_SHIFT) as u16 & 0x0FFF;
    let code = [
        golay23_encode(c0_data) << 1,
        golay23_encode(c1_data) ^ scrambler(u32::from(c0_data)),
        (info.0 >> C2_SHIFT) as u32 & 0x07FF,
        info.0 as u32 & 0x3FFF,
    ];
    std::array::from_fn(|i| {
        let (row, column) = INTERLEAVE[i];
        code[usize::from(row)] >> column & 1 == 1
    })
}

#[cfg(test)]
pub(super) fn damage(mut air: [bool; 72], places: &[(u8, u8)]) -> [bool; 72] {
    for place in places {
        if let Some(index) = INTERLEAVE.iter().position(|pair| pair == place) {
            air[index] = !air[index];
        }
    }
    air
}

#[cfg(test)]
impl Info {
    pub(super) fn from_bits(bits: u64) -> Self {
        Self(bits & ((1 << INFO_BITS) - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interleave_covers_every_code_vector_bit_once() {
        let mut seen = [0u32; 4];
        for (row, column) in INTERLEAVE {
            let bit = 1 << column;
            assert_eq!(seen[usize::from(row)] & bit, 0, "({row}, {column})");
            seen[usize::from(row)] |= bit;
        }
        assert_eq!(seen, [0xFF_FFFF, 0x7F_FFFF, 0x7FF, 0x3FFF]);
    }

    #[test]
    fn a_clean_frame_round_trips() {
        let info = Info::from_bits(0x1_2345_6789_ABCD);
        let frame = decode(&encode(info));
        assert_eq!(frame.info, info);
        assert_eq!(frame.errors, 0);
    }

    #[test]
    fn golay_repairs_and_counts_data_errors() {
        let info = Info::from_bits(0x0_F0F0_0F0F_5A5A);
        let frame = decode(&damage(encode(info), &[(0, 23), (1, 0)]));
        assert_eq!(frame.info, info);
        assert_eq!(frame.errors, 1);
    }

    #[test]
    fn fields_read_msb_first() {
        let info = Info::from_bits(0b101 << (INFO_BITS - 3));
        assert_eq!(info.field(&[0, 1, 2]), 0b101);
        assert_eq!(info.field(&[2, 0]), 0b11);
    }
}
