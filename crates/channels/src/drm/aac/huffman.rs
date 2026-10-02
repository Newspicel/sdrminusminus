use std::{collections::HashMap, sync::LazyLock};

use super::tables::{SCALEFACTOR_BITS, SCALEFACTOR_CODES, SPECTRAL_BITS, SPECTRAL_CODES};
use crate::drm::bits::BitReader;

pub const ZERO: u8 = 0;
pub const ESCAPE: u8 = 11;
pub const NOISE: u8 = 13;
pub const ESCAPE_VALUE: i32 = 16;

struct Book {
    lookup: HashMap<u32, u16>,
}

fn key(length: u32, code: u32) -> u32 {
    length << 24 | code
}

impl Book {
    fn new(codes: impl Iterator<Item = (u32, u8)>) -> Self {
        Self {
            lookup: codes
                .enumerate()
                .map(|(index, (code, bits))| (key(u32::from(bits), code), index as u16))
                .collect(),
        }
    }
}

static SPECTRAL: LazyLock<Vec<Book>> = LazyLock::new(|| {
    (0..11)
        .map(|book| {
            Book::new(
                SPECTRAL_CODES[book]
                    .iter()
                    .zip(SPECTRAL_BITS[book])
                    .map(|(&code, &bits)| (u32::from(code), bits)),
            )
        })
        .collect()
});

static SCALEFACTOR: LazyLock<Book> = LazyLock::new(|| {
    Book::new(
        SCALEFACTOR_CODES
            .iter()
            .zip(SCALEFACTOR_BITS)
            .map(|(&code, bits)| (code, bits)),
    )
});

#[must_use]
pub const fn spectral(codebook: u8) -> bool {
    matches!(codebook, 1..=11 | 16..=31)
}

#[must_use]
pub const fn base(codebook: u8) -> u8 {
    if codebook >= 16 { ESCAPE } else { codebook }
}

#[must_use]
pub const fn dimension(codebook: u8) -> usize {
    if base(codebook) < 5 { 4 } else { 2 }
}

#[must_use]
pub const fn signed(codebook: u8) -> bool {
    matches!(base(codebook), 1 | 2 | 5 | 6)
}

#[must_use]
pub fn values(codebook: u8, index: u16) -> [i32; 4] {
    let index = i32::from(index);
    match base(codebook) {
        1 | 2 => [
            index / 27 - 1,
            index / 9 % 3 - 1,
            index / 3 % 3 - 1,
            index % 3 - 1,
        ],
        3 | 4 => [index / 27, index / 9 % 3, index / 3 % 3, index % 3],
        5 | 6 => [index / 9 - 4, index % 9 - 4, 0, 0],
        7 | 8 => [index / 8, index % 8, 0, 0],
        9 | 10 => [index / 13, index % 13, 0, 0],
        _ => [index / 17, index % 17, 0, 0],
    }
}

#[cfg(test)]
#[must_use]
pub const fn zero_index(codebook: u8) -> u16 {
    if signed(codebook) { 40 } else { 0 }
}

#[cfg(test)]
#[must_use]
pub fn code(codebook: u8, index: u16) -> (u32, u32) {
    let book = usize::from(base(codebook) - 1);
    let index = usize::from(index);
    (
        u32::from(SPECTRAL_CODES[book][index]),
        u32::from(SPECTRAL_BITS[book][index]),
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Prefix {
    code: u32,
    length: u32,
}

impl Prefix {
    pub fn push(&mut self, bit: bool) -> Option<()> {
        if self.length >= 20 {
            return None;
        }
        self.code = self.code << 1 | u32::from(bit);
        self.length += 1;
        Some(())
    }

    #[must_use]
    pub fn spectral(&self, codebook: u8) -> Option<u16> {
        SPECTRAL
            .get(usize::from(base(codebook)).checked_sub(1)?)?
            .lookup
            .get(&key(self.length, self.code))
            .copied()
    }

    #[must_use]
    pub fn scalefactor(&self) -> Option<u16> {
        SCALEFACTOR
            .lookup
            .get(&key(self.length, self.code))
            .copied()
    }

    #[must_use]
    pub const fn exhausted(&self) -> bool {
        self.length >= 20
    }
}

pub fn read_scalefactor(reader: &mut BitReader<'_>) -> Option<i32> {
    let mut prefix = Prefix::default();
    loop {
        prefix.push(reader.bit()?)?;
        if let Some(index) = prefix.scalefactor() {
            return Some(i32::from(index) - 60);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_is_unique_and_prefix_free() {
        for book in 0..11u8 {
            let codes = SPECTRAL_CODES[usize::from(book)];
            let bits = SPECTRAL_BITS[usize::from(book)];
            for (a, (&code_a, &len_a)) in codes.iter().zip(bits).enumerate() {
                for (b, (&code_b, &len_b)) in codes.iter().zip(bits).enumerate() {
                    if a != b && len_a <= len_b {
                        assert_ne!(
                            u32::from(code_b) >> (len_b - len_a),
                            u32::from(code_a),
                            "book {} {a} {b}",
                            book + 1
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_zero_tuple_decodes_to_silence() {
        for codebook in 1..=11u8 {
            assert!(
                values(codebook, zero_index(codebook))
                    .iter()
                    .all(|&value| value == 0)
            );
        }
    }
}
