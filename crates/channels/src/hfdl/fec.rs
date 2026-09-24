pub const SEQUENCE_LEN: usize = 127;
pub const TRAINING_LEN: usize = 15;
pub const SEGMENT_SYMBOLS: usize = 30;
const INTERLEAVER_ROWS: usize = 40;
const INTERLEAVER_ROW_STEP: usize = 9;
const SCRAMBLER_CYCLE: usize = 120;
const SCRAMBLER_SEED: [u8; 15] = [1, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1];

pub const A: [u8; SEQUENCE_LEN] = chips(
    b"0101101110111100011101000101011100000011110110011000100100111001111100100000100011010101001101101001010000101100001100101111111",
);
pub const M: [u8; SEQUENCE_LEN] = chips(
    b"0111011011110100010110010111110001000000110011011000111001110101110000100110000010101011010010010100111100100011010100001111111",
);
pub const T: [u8; TRAINING_LEN] = chips(b"000100110101111");
const SCRAMBLE: [u8; SCRAMBLER_CYCLE] = scrambler_cycle();

const fn chips<const N: usize>(text: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut index = 0;
    while index < N {
        out[index] = text[index] - b'0';
        index += 1;
    }
    out
}

const fn scrambler_cycle() -> [u8; SCRAMBLER_CYCLE] {
    let mut state = SCRAMBLER_SEED;
    let mut out = [0u8; SCRAMBLER_CYCLE];
    let mut index = 0;
    while index < SCRAMBLER_CYCLE {
        let bit = state[0] ^ state[14];
        let mut shift = 14;
        while shift > 0 {
            state[shift] = state[shift - 1];
            shift -= 1;
        }
        state[0] = bit;
        out[index] = bit;
        index += 1;
    }
    out
}

pub fn scramble_flip(data_symbol: usize) -> bool {
    SCRAMBLE[data_symbol % SCRAMBLER_CYCLE] == 1
}

pub fn m1_chip(setting: &Setting, index: usize) -> u8 {
    M[(setting.m1_shift + index) % SEQUENCE_LEN]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub m1_shift: usize,
    pub bps: u32,
    pub bits_per_symbol: u32,
    pub rate_quarter: bool,
    pub double_slot: bool,
}

const fn setting(
    m1_shift: usize,
    bps: u32,
    bits_per_symbol: u32,
    rate_quarter: bool,
    double_slot: bool,
) -> Setting {
    Setting {
        m1_shift,
        bps,
        bits_per_symbol,
        rate_quarter,
        double_slot,
    }
}

pub const SETTINGS: [Setting; 8] = [
    setting(72, 300, 1, true, false),
    setting(82, 600, 1, false, false),
    setting(113, 1200, 2, false, false),
    setting(123, 1800, 3, false, false),
    setting(61, 300, 1, true, true),
    setting(103, 600, 1, false, true),
    setting(93, 1200, 2, false, true),
    setting(9, 1800, 3, false, true),
];

impl Setting {
    pub fn data_segments(&self) -> usize {
        if self.double_slot { 168 } else { 72 }
    }

    pub fn chips(&self) -> usize {
        self.data_segments() * SEGMENT_SYMBOLS * self.bits_per_symbol as usize
    }

    #[cfg(test)]
    pub fn payload_bits(&self) -> usize {
        if self.rate_quarter {
            self.chips() / 4
        } else {
            self.chips() / 2
        }
    }

    #[cfg(test)]
    pub fn payload_bytes(&self) -> usize {
        self.payload_bits() / 8
    }

    fn col_shift(&self) -> usize {
        if self.double_slot { 23 } else { 17 }
    }

    fn cols(&self) -> usize {
        self.chips() / INTERLEAVER_ROWS
    }
}

fn push_indices(chips: usize, cols: usize, shift: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(chips);
    let (mut row, mut col) = (0usize, 0usize);
    for _ in 0..chips {
        out.push(row * cols + col);
        row += 1;
        if row == INTERLEAVER_ROWS {
            row = 0;
            col = (col + 1) % cols;
        }
        col = (col + cols - shift % cols) % cols;
    }
    out
}

fn pop_indices(chips: usize, cols: usize) -> Vec<usize> {
    let mut out = Vec::with_capacity(chips);
    let (mut row, mut col) = (0usize, 0usize);
    for _ in 0..chips {
        out.push(row * cols + col);
        row = (row + INTERLEAVER_ROW_STEP) % INTERLEAVER_ROWS;
        if row == 0 {
            col = (col + 1) % cols;
        }
    }
    out
}

pub fn deinterleave(soft: &[f32], setting: &Setting) -> Vec<f32> {
    let chips = setting.chips();
    let push = push_indices(chips, setting.cols(), setting.col_shift());
    let pop = pop_indices(chips, setting.cols());
    let mut table = vec![0.0f32; chips];
    for (&position, &value) in push.iter().zip(soft) {
        table[position] = value;
    }
    pop.iter().map(|&position| table[position]).collect()
}

#[cfg(test)]
pub fn interleave(chips_in: &[u8], setting: &Setting) -> Vec<u8> {
    let chips = setting.chips();
    let push = push_indices(chips, setting.cols(), setting.col_shift());
    let pop = pop_indices(chips, setting.cols());
    let mut table = vec![0u8; chips];
    for (&position, &chip) in pop.iter().zip(chips_in) {
        table[position] = chip;
    }
    push.iter().map(|&position| table[position]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequences_match_xng() {
        assert_eq!(xng_mode_hfdl::fec::bits_of(xng_mode_hfdl::fec::A_BITS), A);
        assert_eq!(xng_mode_hfdl::fec::bits_of(xng_mode_hfdl::fec::M_BITS), M);
        assert_eq!(xng_mode_hfdl::fec::bits_of(xng_mode_hfdl::fec::T_BITS), T);
    }

    #[test]
    fn scrambler_matches_xng() {
        let reference = xng_mode_hfdl::fec::scramble_flips(5_040);
        let ours: Vec<u8> = (0..5_040).map(|i| u8::from(scramble_flip(i))).collect();
        assert_eq!(ours, reference);
    }

    #[test]
    fn interleaver_indices_are_permutations() {
        for setting in &SETTINGS {
            let chips = setting.chips();
            for indices in [
                push_indices(chips, setting.cols(), setting.col_shift()),
                pop_indices(chips, setting.cols()),
            ] {
                let mut sorted = indices.clone();
                sorted.sort_unstable();
                assert!(sorted.iter().enumerate().all(|(i, &v)| i == v));
            }
        }
    }

    #[test]
    fn interleave_roundtrip() {
        for setting in &SETTINGS {
            let chips: Vec<u8> = (0..setting.chips())
                .map(|i| (i % 2) as u8 ^ ((i / 7) % 2) as u8)
                .collect();
            let air = interleave(&chips, setting);
            let soft: Vec<f32> = air
                .iter()
                .map(|&b| if b == 1 { 1.0 } else { -1.0 })
                .collect();
            let back = deinterleave(&soft, setting);
            let hard: Vec<u8> = back.iter().map(|&v| u8::from(v > 0.0)).collect();
            assert_eq!(hard, chips);
        }
    }

    #[test]
    fn scrambler_tiles_120() {
        assert!((0..120).all(|i| scramble_flip(i) == scramble_flip(i + 120)));
    }
}
