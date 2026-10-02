use std::ops::Range;

use super::{
    huffman::{self, ESCAPE_VALUE, NOISE, Prefix, ZERO},
    tables::{SWB_LONG_16, SWB_LONG_24, SWB_LONG_48, SWB_SHORT_16, SWB_SHORT_24, SWB_SHORT_48},
};
use crate::drm::bits::{BitReader, BitWriter};

pub const EIGHT_SHORT: u8 = 2;
pub const MAX_CODEWORD_BITS: u8 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IcsInfo {
    pub sequence: u8,
    pub shape: bool,
    pub max_sfb: u8,
    pub grouping: u8,
}

impl IcsInfo {
    #[must_use]
    pub const fn short(self) -> bool {
        self.sequence == EIGHT_SHORT
    }

    #[must_use]
    pub fn groups(self) -> Vec<usize> {
        if !self.short() {
            return vec![1];
        }
        let mut groups = vec![1];
        for bit in (0..7).rev() {
            match groups.last_mut() {
                Some(length) if self.grouping >> bit & 1 == 1 => *length += 1,
                _ => groups.push(1),
            }
        }
        groups
    }

    pub fn read_header(reader: &mut BitReader<'_>) -> Option<Self> {
        let sequence = reader.read(2)? as u8;
        let shape = reader.flag()?;
        let (max_sfb, grouping) = if sequence == EIGHT_SHORT {
            (reader.read(4)? as u8, reader.read(7)? as u8)
        } else {
            (reader.read(6)? as u8, 0)
        };
        Some(Self {
            sequence,
            shape,
            max_sfb,
            grouping,
        })
    }

    pub fn write_header(self, writer: &mut BitWriter) {
        writer.put(u32::from(self.sequence), 2);
        writer.bit(self.shape);
        if self.short() {
            writer.put(u32::from(self.max_sfb), 4);
            writer.put(u32::from(self.grouping), 7);
        } else {
            writer.put(u32::from(self.max_sfb), 6);
        }
    }
}

#[must_use]
pub fn band_offsets(rate_hz: u32, short: bool) -> Option<&'static [u16]> {
    Some(match (rate_hz, short) {
        (48_000, false) => SWB_LONG_48,
        (48_000, true) => SWB_SHORT_48,
        (24_000, false) => SWB_LONG_24,
        (24_000, true) => SWB_SHORT_24,
        (12_000 | 16_000, false) => SWB_LONG_16,
        (12_000 | 16_000, true) => SWB_SHORT_16,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub group: usize,
    pub codebook: u8,
    pub start: u8,
    pub end: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Codeword {
    pub window: u8,
    pub line: u16,
    pub codebook: u8,
    pub bits: u64,
    pub length: u8,
}

impl Codeword {
    pub fn write(&self, writer: &mut BitWriter) {
        for shift in (0..self.length).rev() {
            writer.bit(self.bits >> shift & 1 == 1);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Body,
    Sign,
    EscapePrefix,
    EscapeWord,
    Done,
}

#[derive(Clone, Copy, Debug)]
pub struct Partial {
    codebook: u8,
    stage: Stage,
    prefix: Prefix,
    values: [i32; 4],
    signs: u8,
    escape: usize,
    ones: u8,
    word: u8,
    bits: u64,
    length: u8,
}

impl Partial {
    #[must_use]
    pub fn new(codebook: u8) -> Self {
        Self {
            codebook,
            stage: Stage::Body,
            prefix: Prefix::default(),
            values: [0; 4],
            signs: 0,
            escape: 0,
            ones: 0,
            word: 0,
            bits: 0,
            length: 0,
        }
    }

    #[must_use]
    pub const fn done(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }

    pub fn push(&mut self, bit: bool) -> Option<bool> {
        if self.length >= MAX_CODEWORD_BITS || self.done() {
            return None;
        }
        self.bits = self.bits << 1 | u64::from(bit);
        self.length += 1;
        match self.stage {
            Stage::Body => self.body(bit)?,
            Stage::Sign => {
                self.signs -= 1;
                if self.signs == 0 {
                    self.after_signs();
                }
            }
            Stage::EscapePrefix => {
                if bit {
                    self.ones += 1;
                    if self.ones > 8 {
                        return None;
                    }
                } else {
                    self.word = self.ones + 4;
                    self.stage = Stage::EscapeWord;
                }
            }
            Stage::EscapeWord => {
                self.word -= 1;
                if self.word == 0 {
                    self.escape += 1;
                    self.next_escape();
                }
            }
            Stage::Done => return None,
        }
        Some(self.done())
    }

    fn body(&mut self, bit: bool) -> Option<()> {
        self.prefix.push(bit)?;
        let Some(index) = self.prefix.spectral(self.codebook) else {
            return (!self.prefix.exhausted()).then_some(());
        };
        self.values = huffman::values(self.codebook, index);
        self.signs = if huffman::signed(self.codebook) {
            0
        } else {
            self.values.iter().filter(|&&value| value != 0).count() as u8
        };
        if self.signs == 0 {
            self.after_signs();
        } else {
            self.stage = Stage::Sign;
        }
        Some(())
    }

    fn after_signs(&mut self) {
        self.escape = 0;
        self.next_escape();
    }

    fn next_escape(&mut self) {
        let escapes = huffman::base(self.codebook) == huffman::ESCAPE;
        while self.escape < 2 && escapes {
            if self.values[self.escape] == ESCAPE_VALUE {
                self.ones = 0;
                self.stage = Stage::EscapePrefix;
                return;
            }
            self.escape += 1;
        }
        self.stage = Stage::Done;
    }

    #[must_use]
    pub fn finish(&self, window: u8, line: u16) -> Codeword {
        Codeword {
            window,
            line,
            codebook: self.codebook,
            bits: self.bits,
            length: self.length,
        }
    }
}

pub fn read_codeword(reader: &mut BitReader<'_>, codebook: u8) -> Option<Partial> {
    let mut partial = Partial::new(codebook);
    while !partial.push(reader.bit()?)? {}
    Some(partial)
}

pub fn read_sections(
    reader: &mut BitReader<'_>,
    info: IcsInfo,
    resilient: bool,
) -> Option<Vec<Section>> {
    let length_bits = if info.short() { 3 } else { 5 };
    let escape = (1 << length_bits) - 1;
    let mut sections = Vec::new();
    for group in 0..info.groups().len() {
        let mut sfb = 0u8;
        while sfb < info.max_sfb {
            let codebook = reader.read(if resilient { 5 } else { 4 })? as u8;
            if codebook == 12 || (!resilient && codebook > 15) {
                return None;
            }
            let length = if resilient && (codebook == huffman::ESCAPE || codebook >= 16) {
                1
            } else {
                let mut length = 0u32;
                loop {
                    let increment = reader.read(length_bits)?;
                    length += increment;
                    if increment != escape {
                        break;
                    }
                }
                length
            };
            let end = u32::from(sfb) + length;
            if length == 0 || end > u32::from(info.max_sfb) {
                return None;
            }
            sections.push(Section {
                group,
                codebook,
                start: sfb,
                end: end as u8,
            });
            sfb = end as u8;
        }
    }
    Some(sections)
}

pub fn write_sections(
    writer: &mut BitWriter,
    info: IcsInfo,
    sections: &[Section],
    resilient: bool,
) {
    let length_bits = if info.short() { 3 } else { 5 };
    let escape = (1u32 << length_bits) - 1;
    for section in sections {
        let codebook = if resilient {
            section.codebook
        } else {
            huffman::base(section.codebook)
        };
        if resilient && (codebook == huffman::ESCAPE || codebook >= 16) {
            for _ in section.start..section.end {
                writer.put(u32::from(codebook), 5);
            }
            continue;
        }
        writer.put(u32::from(codebook), if resilient { 5 } else { 4 });
        let mut length = u32::from(section.end - section.start);
        while length >= escape {
            writer.put(escape, length_bits);
            length -= escape;
        }
        writer.put(length, length_bits);
    }
}

#[must_use]
pub fn codebook_at(sections: &[Section], group: usize, sfb: u8) -> u8 {
    sections
        .iter()
        .find(|section| section.group == group && (section.start..section.end).contains(&sfb))
        .map_or(ZERO, |section| section.codebook)
}

pub fn skip_scalefactors(
    reader: &mut BitReader<'_>,
    info: IcsInfo,
    sections: &[Section],
) -> Option<Range<usize>> {
    let start = reader.position();
    let mut noise_started = false;
    for group in 0..info.groups().len() {
        for sfb in 0..info.max_sfb {
            match codebook_at(sections, group, sfb) {
                ZERO => {}
                NOISE if !noise_started => {
                    noise_started = true;
                    reader.skip(9)?;
                }
                _ => {
                    huffman::read_scalefactor(reader)?;
                }
            }
        }
    }
    Some(start..reader.position())
}

pub fn skip_tns(reader: &mut BitReader<'_>, info: IcsInfo) -> Option<Range<usize>> {
    let start = reader.position();
    let short = info.short();
    let windows = if short { 8 } else { 1 };
    for _ in 0..windows {
        let filters = reader.read(if short { 1 } else { 2 })?;
        if filters == 0 {
            continue;
        }
        let resolution = reader.read(1)?;
        for _ in 0..filters {
            reader.skip(if short { 4 } else { 6 })?;
            let order = reader.read(if short { 3 } else { 5 })?;
            if order == 0 {
                continue;
            }
            reader.skip(1)?;
            let compress = reader.read(1)?;
            let bits = 3 + resolution - compress;
            reader.skip((order * bits) as usize)?;
        }
    }
    Some(start..reader.position())
}

pub fn standard_positions(
    info: IcsInfo,
    sections: &[Section],
    offsets: &[u16],
) -> Vec<(u8, u8, u16)> {
    let groups = info.groups();
    let mut starts = Vec::with_capacity(groups.len());
    let mut window = 0usize;
    for &length in &groups {
        starts.push(window);
        window += length;
    }
    let mut positions = Vec::new();
    for section in sections {
        if !huffman::spectral(section.codebook) {
            continue;
        }
        let dimension = huffman::dimension(section.codebook);
        for sfb in section.start..section.end {
            let lines = offsets[usize::from(sfb)]..offsets[usize::from(sfb) + 1];
            for w in 0..groups[section.group] {
                let window = (starts[section.group] + w) as u8;
                for line in lines.clone().step_by(dimension) {
                    positions.push((section.codebook, window, line));
                }
            }
        }
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_bits_split_the_eight_short_windows() {
        let info = IcsInfo {
            sequence: EIGHT_SHORT,
            shape: false,
            max_sfb: 10,
            grouping: 0b110_1011,
        };
        assert_eq!(info.groups(), vec![3, 2, 3]);
    }

    #[test]
    fn escape_codewords_collect_their_sign_and_escape_bits() {
        let index = 16 * 17 + 3;
        let (code, length) = huffman::code(11, index);
        let mut writer = BitWriter::default();
        writer.put(code, length);
        writer.put(0b10, 2);
        writer.put(0b10, 2);
        writer.put(0b10101, 5);
        writer.align();
        let bytes = writer.into_bytes();
        let mut reader = BitReader::new(&bytes);
        let partial = read_codeword(&mut reader, 11).expect("a complete codeword");
        let codeword = partial.finish(0, 0);
        assert_eq!(u32::from(codeword.length), length + 9);
        assert_eq!(reader.position(), (length + 9) as usize);
    }
}
