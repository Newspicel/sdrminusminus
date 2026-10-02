use super::{
    huffman,
    syntax::{Codeword, IcsInfo, Partial, Section, codebook_at},
};
use crate::drm::bits::BitReader;

const MAX_LENGTH: [u8; 32] = [
    0, 11, 9, 20, 16, 13, 11, 14, 12, 17, 14, 49, 0, 0, 0, 0, 14, 17, 21, 21, 25, 25, 29, 29, 29,
    29, 33, 33, 33, 37, 37, 41,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub codebook: u8,
    pub window: u8,
    pub line: u16,
}

const fn priority(codebook: u8) -> u8 {
    match codebook {
        11 => 22,
        16..=31 => codebook - 10,
        9 | 10 => 5,
        7 | 8 => 4,
        5 | 6 => 3,
        3 | 4 => 2,
        1 | 2 => 1,
        _ => 0,
    }
}

fn push_unit(slots: &mut Vec<Slot>, codebook: u8, window: u8, line: u16) {
    if huffman::dimension(codebook) == 4 {
        slots.push(Slot {
            codebook,
            window,
            line,
        });
    } else {
        for offset in [0, 2] {
            slots.push(Slot {
                codebook,
                window,
                line: line + offset,
            });
        }
    }
}

fn long_sections(sections: &[Section], offsets: &[u16]) -> Vec<(u8, Vec<Slot>)> {
    sections
        .iter()
        .map(|section| {
            let dimension = huffman::dimension(section.codebook);
            let lines = offsets[usize::from(section.start)]..offsets[usize::from(section.end)];
            let slots = lines
                .step_by(dimension)
                .map(|line| Slot {
                    codebook: section.codebook,
                    window: 0,
                    line,
                })
                .collect();
            (section.codebook, slots)
        })
        .collect()
}

fn short_sections(info: IcsInfo, sections: &[Section], offsets: &[u16]) -> Vec<(u8, Vec<Slot>)> {
    let groups = info.groups();
    let mut runs: Vec<(u8, Vec<Slot>)> = Vec::new();
    for band in 0..info.max_sfb {
        let low = offsets[usize::from(band)];
        let high = offsets[usize::from(band) + 1];
        for unit in (low..high).step_by(4) {
            let mut window = 0u8;
            for (group, &length) in groups.iter().enumerate() {
                let codebook = codebook_at(sections, group, band);
                for _ in 0..length {
                    match runs.last_mut() {
                        Some((current, slots)) if *current == codebook => {
                            push_unit(slots, codebook, window, unit);
                        }
                        _ => {
                            let mut slots = Vec::new();
                            push_unit(&mut slots, codebook, window, unit);
                            runs.push((codebook, slots));
                        }
                    }
                    window += 1;
                }
            }
        }
    }
    runs
}

#[must_use]
pub fn order(info: IcsInfo, sections: &[Section], offsets: &[u16]) -> Vec<Slot> {
    let mut runs = if info.short() {
        short_sections(info, sections, offsets)
    } else {
        long_sections(sections, offsets)
    };
    runs.retain(|(codebook, _)| priority(*codebook) > 0);
    runs.sort_by_key(|(codebook, _)| std::cmp::Reverse(priority(*codebook)));
    runs.into_iter().flat_map(|(_, slots)| slots).collect()
}

#[derive(Clone, Copy, Debug)]
struct Segment {
    left: usize,
    right: usize,
    remaining: usize,
}

impl Segment {
    fn take(&mut self, backwards: bool) -> usize {
        self.remaining -= 1;
        if backwards {
            let at = self.right;
            self.right = self.right.saturating_sub(1);
            at
        } else {
            let at = self.left;
            self.left += 1;
            at
        }
    }
}

fn segments(slots: &[Slot], length: usize, longest: usize) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut position = 0;
    for slot in slots {
        let width = usize::from(MAX_LENGTH[usize::from(slot.codebook)]).min(longest);
        if position + width <= length {
            segments.push(Segment {
                left: position,
                right: position + width - 1,
                remaining: width,
            });
            position += width;
        } else {
            if let Some(last) = segments.last_mut() {
                last.right = length - 1;
                last.remaining = length - last.left;
            }
            break;
        }
    }
    segments
}

pub fn decode(
    reader: &BitReader<'_>,
    start: usize,
    length: usize,
    longest: usize,
    slots: &[Slot],
) -> Option<Vec<Codeword>> {
    if slots.is_empty() {
        return Some(Vec::new());
    }
    if longest == 0 || reader.remaining() + reader.position() < start + length {
        return None;
    }
    let mut segments = segments(slots, length, longest);
    let count = segments.len();
    if count == 0 {
        return None;
    }
    let mut words = Vec::with_capacity(slots.len());
    for (slot, segment) in slots.iter().zip(&mut segments) {
        let mut partial = Partial::new(slot.codebook);
        loop {
            if segment.remaining == 0 {
                return None;
            }
            let at = segment.take(false);
            if partial.push(reader.bit_at(start + at))? {
                break;
            }
        }
        words.push(partial.finish(slot.window, slot.line));
    }
    let sets = (slots.len() - 1) / count + 1;
    let mut backwards = true;
    for set in 1..sets {
        let base = set * count;
        let members = &slots[base..slots.len().min(base + count)];
        let mut partials: Vec<Partial> = members
            .iter()
            .map(|slot| Partial::new(slot.codebook))
            .collect();
        for trial in 0..count {
            for (index, segment) in segments.iter_mut().enumerate() {
                let word = (index + count - trial) % count;
                let Some(partial) = partials.get_mut(word) else {
                    continue;
                };
                while !partial.done() && segment.remaining > 0 {
                    let at = segment.take(backwards);
                    partial.push(reader.bit_at(start + at))?;
                }
            }
        }
        for (slot, partial) in members.iter().zip(&partials) {
            if !partial.done() {
                return None;
            }
            words.push(partial.finish(slot.window, slot.line));
        }
        backwards = !backwards;
    }
    Some(words)
}

#[cfg(any(test, feature = "synth"))]
fn place(words: &[Codeword], length: usize, longest: usize, slots: &[Slot]) -> Option<Vec<bool>> {
    let mut bits = vec![false; length];
    let mut segments = segments(slots, length, longest);
    let count = segments.len();
    if count == 0 {
        return words.is_empty().then_some(bits);
    }
    let bit =
        |word: &Codeword, index: usize| word.bits >> (word.length as usize - 1 - index) & 1 == 1;
    for (word, segment) in words.iter().zip(&mut segments) {
        if usize::from(word.length) > segment.remaining {
            return None;
        }
        for index in 0..usize::from(word.length) {
            let at = segment.take(false);
            bits[at] = bit(word, index);
        }
    }
    let sets = (words.len() - 1) / count + 1;
    let mut backwards = true;
    for set in 1..sets {
        let base = set * count;
        let members = &words[base..words.len().min(base + count)];
        let mut written = vec![0usize; members.len()];
        for trial in 0..count {
            for (index, segment) in segments.iter_mut().enumerate() {
                let slot = (index + count - trial) % count;
                let Some(word) = members.get(slot) else {
                    continue;
                };
                while written[slot] < usize::from(word.length) && segment.remaining > 0 {
                    let at = segment.take(backwards);
                    bits[at] = bit(word, written[slot]);
                    written[slot] += 1;
                }
            }
        }
        if members
            .iter()
            .zip(&written)
            .any(|(word, &done)| done < usize::from(word.length))
        {
            return None;
        }
        backwards = !backwards;
    }
    Some(bits)
}

#[cfg(any(test, feature = "synth"))]
pub fn encode(words: &[Codeword], slots: &[Slot]) -> Option<(Vec<bool>, usize)> {
    let longest = words
        .iter()
        .map(|word| usize::from(word.length))
        .max()
        .unwrap_or(0);
    let total: usize = words.iter().map(|word| usize::from(word.length)).sum();
    (total..total + 4096)
        .find_map(|length| place(words, length, longest, slots).map(|bits| (bits, longest)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drm::{
        aac::syntax::{self, EIGHT_SHORT, band_offsets},
        bits::BitWriter,
    };

    fn words(slots: &[Slot], seed: u32) -> Vec<Codeword> {
        let mut state = seed | 1;
        slots
            .iter()
            .map(|slot| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let size = match huffman::base(slot.codebook) {
                    1..=4 => 81,
                    5 | 6 => 81,
                    7 | 8 => 64,
                    9 | 10 => 169,
                    _ => 289,
                };
                let index = (state % size) as u16;
                let (code, length) = huffman::code(slot.codebook, index);
                let mut writer = BitWriter::default();
                writer.put(code, length);
                let values = huffman::values(slot.codebook, index);
                if !huffman::signed(slot.codebook) {
                    for value in values {
                        if value != 0 {
                            writer.bit(state >> 7 & 1 == 1);
                        }
                    }
                }
                if huffman::base(slot.codebook) == huffman::ESCAPE {
                    for value in &values[..2] {
                        if *value == huffman::ESCAPE_VALUE {
                            writer.put(0b10, 2);
                            writer.put(0b10110, 5);
                        }
                    }
                }
                let length = writer.bit_len();
                writer.align();
                let mut reader = BitReader::new(writer.bytes());
                let mut bits = 0u64;
                for _ in 0..length {
                    bits = bits << 1 | u64::from(reader.bit().unwrap_or(false));
                }
                Codeword {
                    window: slot.window,
                    line: slot.line,
                    codebook: slot.codebook,
                    bits,
                    length: length as u8,
                }
            })
            .collect()
    }

    fn round_trip(info: IcsInfo, sections: &[Section], offsets: &[u16], seed: u32) {
        let slots = order(info, sections, offsets);
        let words = words(&slots, seed);
        let (bits, longest) = encode(&words, &slots).expect("the codewords fit");
        let mut writer = BitWriter::default();
        for &bit in &bits {
            writer.bit(bit);
        }
        writer.align();
        let reader = BitReader::new(writer.bytes());
        let decoded = decode(&reader, 0, bits.len(), longest, &slots).expect("decodable");
        assert_eq!(decoded, words);
    }

    #[test]
    fn long_windows_survive_reordering() {
        let offsets = band_offsets(24_000, false).expect("24 kHz bands");
        let info = IcsInfo {
            sequence: 0,
            shape: false,
            max_sfb: 30,
            grouping: 0,
        };
        let sections = [
            (0, 4, 1),
            (4, 9, 11),
            (9, 12, 0),
            (12, 20, 5),
            (20, 24, 24),
            (24, 30, 9),
        ]
        .map(|(start, end, codebook)| Section {
            group: 0,
            codebook,
            start,
            end,
        });
        for seed in 1..20 {
            round_trip(info, &sections, offsets, seed);
        }
    }

    #[test]
    fn grouped_short_windows_survive_reordering() {
        let offsets = band_offsets(48_000, true).expect("48 kHz bands");
        let info = IcsInfo {
            sequence: EIGHT_SHORT,
            shape: true,
            max_sfb: 12,
            grouping: 0b101_1001,
        };
        let groups = info.groups();
        let mut sections = Vec::new();
        for group in 0..groups.len() {
            for (start, end, codebook) in [(0, 3, 3), (3, 6, 7), (6, 9, 11), (9, 12, 2)] {
                sections.push(Section {
                    group,
                    codebook: (codebook + group as u8) % 12,
                    start,
                    end,
                });
            }
        }
        for seed in 1..20 {
            round_trip(info, &sections, offsets, seed);
        }
        let standard = syntax::standard_positions(info, &sections, offsets);
        let slots = order(info, &sections, offsets);
        assert_eq!(standard.len(), slots.len());
    }
}
