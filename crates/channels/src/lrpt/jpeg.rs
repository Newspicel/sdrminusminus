use std::sync::LazyLock;

pub const MCU_SIDE: usize = 8;
pub const MCU_PIXELS: usize = MCU_SIDE * MCU_SIDE;
pub const MCUS_PER_PACKET: usize = 14;

pub type Block = [u8; MCU_PIXELS];

pub const ZIGZAG: [usize; MCU_PIXELS] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const LUMA_QUANT: [u16; MCU_PIXELS] = [
    16, 11, 10, 16, 24, 40, 51, 61, 12, 12, 14, 19, 26, 58, 60, 55, 14, 13, 16, 24, 40, 57, 69, 56,
    14, 17, 22, 29, 51, 87, 80, 62, 18, 22, 37, 56, 68, 109, 103, 77, 24, 35, 55, 64, 81, 104, 113,
    92, 49, 64, 78, 87, 103, 121, 120, 101, 72, 92, 95, 98, 112, 100, 103, 99,
];

const DC_COUNTS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_SYMBOLS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_COUNTS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7D];
pub const AC_SYMBOLS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
    0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xA1, 0x08, 0x23, 0x42, 0xB1, 0xC1, 0x15, 0x52, 0xD1, 0xF0,
    0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0A, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x25, 0x26, 0x27, 0x28,
    0x29, 0x2A, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3A, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
    0x4A, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
    0x8A, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0xA2, 0xA3, 0xA4, 0xA5, 0xA6, 0xA7,
    0xA8, 0xA9, 0xAA, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xC2, 0xC3, 0xC4, 0xC5,
    0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xE1, 0xE2,
    0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8,
    0xF9, 0xFA,
];

const END_OF_BLOCK: u8 = 0x00;
const ZERO_RUN: u8 = 0xF0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code {
    pub bits: u16,
    pub len: u8,
}

pub struct Huffman {
    max_code: [i32; 17],
    offset: [i32; 17],
    symbols: [u8; 256],
    codes: [Code; 256],
}

impl Huffman {
    fn new(counts: &[u8; 16], symbols: &[u8]) -> Self {
        let mut table = Self {
            max_code: [-1; 17],
            offset: [0; 17],
            symbols: [0; 256],
            codes: [Code { bits: 0, len: 0 }; 256],
        };
        let mut code = 0i32;
        let mut index = 0usize;
        for len in 1..=16 {
            let count = usize::from(counts[len - 1]);
            table.offset[len] = index as i32 - code;
            for _ in 0..count {
                let symbol = symbols[index];
                table.symbols[index] = symbol;
                table.codes[usize::from(symbol)] = Code {
                    bits: code as u16,
                    len: len as u8,
                };
                index += 1;
                code += 1;
            }
            table.max_code[len] = if count == 0 { -1 } else { code - 1 };
            code <<= 1;
        }
        table
    }

    #[must_use]
    pub fn code(&self, symbol: u8) -> Code {
        self.codes[usize::from(symbol)]
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Option<u8> {
        let mut code = 0i32;
        for len in 1..=16 {
            code = code << 1 | i32::from(reader.bit()?);
            if code <= self.max_code[len] {
                return self
                    .symbols
                    .get(usize::try_from(code + self.offset[len]).ok()?)
                    .copied();
            }
        }
        None
    }
}

pub static DC_TABLE: LazyLock<Huffman> = LazyLock::new(|| Huffman::new(&DC_COUNTS, &DC_SYMBOLS));
pub static AC_TABLE: LazyLock<Huffman> = LazyLock::new(|| Huffman::new(&AC_COUNTS, &AC_SYMBOLS));

static COSINES: LazyLock<[[f32; MCU_SIDE]; MCU_SIDE]> = LazyLock::new(|| {
    std::array::from_fn(|x| {
        std::array::from_fn(|u| {
            let scale = if u == 0 {
                std::f32::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            scale * ((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI / 16.0).cos() / 2.0
        })
    })
});

#[must_use]
pub fn quant_table(quality: u8) -> [u16; MCU_PIXELS] {
    let quality = f32::from(quality.max(1));
    let factor = if quality > 20.0 && quality < 50.0 {
        5_000.0 / quality
    } else {
        200.0 - 2.0 * quality
    };
    LUMA_QUANT.map(|base| ((factor / 100.0 * f32::from(base)).round() as u16).max(1))
}

pub fn inverse_dct(coefficients: &[f32; MCU_PIXELS], pixels: &mut Block) {
    let cos = &*COSINES;
    let mut rows = [0f32; MCU_PIXELS];
    for y in 0..MCU_SIDE {
        for u in 0..MCU_SIDE {
            rows[y * MCU_SIDE + u] = (0..MCU_SIDE)
                .map(|v| cos[y][v] * coefficients[v * MCU_SIDE + u])
                .sum();
        }
    }
    for y in 0..MCU_SIDE {
        for x in 0..MCU_SIDE {
            let value: f32 = (0..MCU_SIDE)
                .map(|u| cos[x][u] * rows[y * MCU_SIDE + u])
                .sum();
            pixels[y * MCU_SIDE + x] = (value + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

#[must_use]
pub fn forward_dct(pixels: &Block) -> [f32; MCU_PIXELS] {
    let cos = &*COSINES;
    let mut rows = [0f32; MCU_PIXELS];
    for y in 0..MCU_SIDE {
        for u in 0..MCU_SIDE {
            rows[y * MCU_SIDE + u] = (0..MCU_SIDE)
                .map(|x| cos[x][u] * (f32::from(pixels[y * MCU_SIDE + x]) - 128.0))
                .sum();
        }
    }
    std::array::from_fn(|index| {
        let (v, u) = (index / MCU_SIDE, index % MCU_SIDE);
        (0..MCU_SIDE)
            .map(|y| cos[y][v] * rows[y * MCU_SIDE + u])
            .sum()
    })
}

pub struct BitReader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bit(&mut self) -> Option<bool> {
        let byte = self.data.get(self.at / 8)?;
        let bit = byte >> (7 - self.at % 8) & 1 == 1;
        self.at += 1;
        Some(bit)
    }

    fn bits(&mut self, count: u8) -> Option<u16> {
        (0..count).try_fold(0u16, |acc, _| Some(acc << 1 | u16::from(self.bit()?)))
    }
}

#[must_use]
pub fn magnitude_category(value: i32) -> u8 {
    (32 - value.unsigned_abs().leading_zeros()) as u8
}

#[must_use]
pub fn magnitude_bits(value: i32, category: u8) -> u16 {
    if value >= 0 {
        value as u16
    } else {
        (value + (1 << category) - 1) as u16
    }
}

fn extend(raw: u16, category: u8) -> i32 {
    if category == 0 {
        return 0;
    }
    let raw = i32::from(raw);
    if raw < 1 << (category - 1) {
        raw - (1 << category) + 1
    } else {
        raw
    }
}

pub struct McuDecoder<'a> {
    reader: BitReader<'a>,
    quant: [u16; MCU_PIXELS],
    previous_dc: i32,
}

impl<'a> McuDecoder<'a> {
    #[must_use]
    pub fn new(data: &'a [u8], quality: u8) -> Self {
        Self {
            reader: BitReader::new(data),
            quant: quant_table(quality),
            previous_dc: 0,
        }
    }

    pub fn next_block(&mut self, pixels: &mut Block) -> Option<()> {
        let mut coefficients = [0f32; MCU_PIXELS];
        let category = DC_TABLE.decode(&mut self.reader)?;
        if category > 11 {
            return None;
        }
        let raw = self.reader.bits(category)?;
        self.previous_dc += extend(raw, category);
        coefficients[0] = self.previous_dc as f32 * f32::from(self.quant[0]);
        let mut index = 1;
        while index < MCU_PIXELS {
            let symbol = AC_TABLE.decode(&mut self.reader)?;
            if symbol == END_OF_BLOCK {
                break;
            }
            let (run, size) = (usize::from(symbol >> 4), symbol & 0x0F);
            index += if symbol == ZERO_RUN { 16 } else { run };
            if symbol == ZERO_RUN {
                continue;
            }
            let natural = *ZIGZAG.get(index)?;
            let raw = self.reader.bits(size)?;
            coefficients[natural] = extend(raw, size) as f32 * f32::from(self.quant[natural]);
            index += 1;
        }
        inverse_dct(&coefficients, pixels);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_zigzag_is_a_permutation() {
        let mut seen = [false; MCU_PIXELS];
        for &index in &ZIGZAG {
            seen[index] = true;
        }
        assert!(seen.iter().all(|&hit| hit));
    }

    #[test]
    fn the_ac_table_covers_every_run_and_size_once() {
        let mut seen = std::collections::HashSet::new();
        for &symbol in &AC_SYMBOLS {
            assert!(seen.insert(symbol));
            let size = symbol & 0x0F;
            assert!(size <= 10 && (size > 0 || symbol == 0 || symbol == 0xF0));
        }
        assert_eq!(seen.len(), 162);
        assert_eq!(
            AC_COUNTS.iter().map(|&c| usize::from(c)).sum::<usize>(),
            162
        );
    }

    #[test]
    fn the_dct_pair_round_trips() {
        let block: Block = std::array::from_fn(|index| (index * 3 % 256) as u8);
        let mut back = [0u8; MCU_PIXELS];
        inverse_dct(&forward_dct(&block), &mut back);
        for (a, b) in block.iter().zip(&back) {
            assert!(a.abs_diff(*b) <= 1);
        }
    }

    #[test]
    fn categories_and_magnitudes_invert() {
        for value in -1000..=1000 {
            let category = magnitude_category(value);
            assert_eq!(extend(magnitude_bits(value, category), category), value);
        }
    }

    #[test]
    fn quality_scales_the_table() {
        assert_eq!(quant_table(50)[0], 16);
        assert!(quant_table(90)[0] < quant_table(30)[0]);
        assert!(quant_table(100).iter().all(|&q| q == 1));
    }
}
