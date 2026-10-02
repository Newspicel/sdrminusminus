use std::ops::Range;

use super::{
    AudioConfig, hcr,
    huffman::{self, ESCAPE, ESCAPE_VALUE, Prefix},
    syntax::{
        Codeword, IcsInfo, Section, band_offsets, read_codeword, read_sections, skip_scalefactors,
        skip_tns, standard_positions, write_sections,
    },
};
use crate::drm::bits::{BitReader, BitWriter, crc};

const LARGEST: [u32; 16] = [
    15, 31, 47, 63, 95, 127, 159, 191, 223, 255, 319, 383, 511, 767, 1023, 2047,
];
const LONGEST: [u8; 16] = [
    14, 17, 21, 21, 25, 25, 29, 29, 29, 29, 33, 33, 33, 37, 37, 41,
];

pub(super) struct Channel {
    tns: Option<Range<usize>>,
    gain: u8,
    sections: Vec<Section>,
    scalefactors: Range<usize>,
    pub(super) words: Vec<Codeword>,
}

fn read_info(reader: &mut BitReader<'_>) -> Option<IcsInfo> {
    if reader.flag()? {
        return None;
    }
    let info = IcsInfo::read_header(reader)?;
    if !info.short() && reader.flag()? {
        return None;
    }
    Some(info)
}

fn read_ics(
    reader: &mut BitReader<'_>,
    common: Option<IcsInfo>,
    rate_hz: u32,
) -> Option<(IcsInfo, Channel)> {
    let gain = reader.read(8)? as u8;
    let info = match common {
        Some(info) => info,
        None => read_info(reader)?,
    };
    let sections = read_sections(reader, info, false)?;
    let scalefactors = skip_scalefactors(reader, info, &sections)?;
    if reader.flag()? {
        return None;
    }
    let tns = if reader.flag()? {
        Some(skip_tns(reader, info)?)
    } else {
        None
    };
    if reader.flag()? {
        return None;
    }
    let offsets = band_offsets(rate_hz, info.short())?;
    let words = standard_positions(info, &sections, offsets)
        .into_iter()
        .map(|(codebook, window, line)| {
            read_codeword(reader, codebook).map(|partial| partial.finish(window, line))
        })
        .collect::<Option<Vec<_>>>()?;
    Some((
        info,
        Channel {
            tns,
            gain,
            sections,
            scalefactors,
            words,
        },
    ))
}

fn magnitude(word: &Codeword) -> Option<u32> {
    let bit = |index: u8| word.bits >> (word.length - 1 - index) & 1 == 1;
    let mut prefix = Prefix::default();
    let mut at = 0u8;
    let index = loop {
        prefix.push(bit(at))?;
        at += 1;
        if let Some(index) = prefix.spectral(word.codebook) {
            break index;
        }
    };
    let values = huffman::values(word.codebook, index);
    let mut largest = 0u32;
    at += values.iter().filter(|&&value| value != 0).count() as u8;
    for &value in &values[..2] {
        let mut value = value.unsigned_abs();
        if value == ESCAPE_VALUE as u32 {
            let mut ones = 0u8;
            while bit(at) {
                ones += 1;
                at += 1;
            }
            at += 1;
            let mut escaped = 0u32;
            for _ in 0..ones + 4 {
                escaped = escaped << 1 | u32::from(bit(at));
                at += 1;
            }
            value = (1 << (ones + 4)) + escaped;
        }
        largest = largest.max(value);
    }
    Some(largest)
}

fn virtual_codebook(words: &[&Codeword]) -> u8 {
    let largest = words
        .iter()
        .filter_map(|word| magnitude(word))
        .max()
        .unwrap_or(0);
    let longest = words.iter().map(|word| word.length).max().unwrap_or(0);
    (0..16)
        .find(|&index| largest <= LARGEST[index] && longest <= LONGEST[index])
        .map_or(ESCAPE, |index| 16 + index as u8)
}

fn resilient_sections(channel: &mut Channel, info: IcsInfo, offsets: &[u16]) {
    let groups = info.groups();
    let starts: Vec<u8> = groups
        .iter()
        .scan(0usize, |window, &length| {
            let start = *window;
            *window += length;
            Some(start as u8)
        })
        .collect();
    let mut sections = Vec::with_capacity(channel.sections.len());
    for section in &channel.sections {
        if section.codebook != ESCAPE {
            sections.push(*section);
            continue;
        }
        for sfb in section.start..section.end {
            let lines = offsets[usize::from(sfb)]..offsets[usize::from(sfb) + 1];
            let windows =
                starts[section.group]..starts[section.group] + groups[section.group] as u8;
            let members: Vec<&Codeword> = channel
                .words
                .iter()
                .filter(|word| windows.contains(&word.window) && lines.contains(&word.line))
                .collect();
            let codebook = virtual_codebook(&members);
            for word in &mut channel.words {
                if windows.contains(&word.window) && lines.contains(&word.line) {
                    word.codebook = codebook;
                }
            }
            sections.push(Section {
                group: section.group,
                codebook,
                start: sfb,
                end: sfb + 1,
            });
        }
    }
    channel.sections = sections;
}

pub(super) struct Element {
    pub(super) info: IcsInfo,
    pub(super) ms: (u32, Vec<bool>),
    pub(super) channels: Vec<Channel>,
    pub(super) sbr: Vec<bool>,
}

fn read_fill(reader: &mut BitReader<'_>, sbr: &mut Vec<bool>) -> Option<()> {
    let mut count = reader.read(4)? as usize;
    if count == 15 {
        count += reader.read(8)? as usize - 1;
    }
    let start = reader.position();
    if count > 0 && reader.read(4)? == super::SBR_EXTENSION {
        sbr.extend((reader.position()..start + count * 8).map(|at| reader.bit_at(at)));
    }
    reader.seek(start);
    reader.skip(count * 8)
}

fn read_data(reader: &mut BitReader<'_>) -> Option<()> {
    reader.skip(4)?;
    let align = reader.flag()?;
    let mut count = reader.read(8)? as usize;
    if count == 255 {
        count += reader.read(8)? as usize;
    }
    if align {
        let position = reader.position();
        reader.skip(position.next_multiple_of(8) - position)?;
    }
    reader.skip(count * 8)
}

pub(super) fn read_element(raw: &[u8], config: &AudioConfig) -> Option<Element> {
    let mut reader = BitReader::new(raw);
    let mut element: Option<Element> = None;
    let mut sbr = Vec::new();
    loop {
        match reader.read(3)? {
            0 => {
                reader.skip(4)?;
                let (info, channel) = read_ics(&mut reader, None, config.rate_hz)?;
                element = Some(Element {
                    info,
                    ms: (0, Vec::new()),
                    channels: vec![channel],
                    sbr: Vec::new(),
                });
            }
            1 => {
                reader.skip(4)?;
                if !reader.flag()? {
                    return None;
                }
                let info = read_info(&mut reader)?;
                let present = reader.read(2)?;
                let used = if present == 1 {
                    (0..info.groups().len() * usize::from(info.max_sfb))
                        .map(|_| reader.flag())
                        .collect::<Option<Vec<bool>>>()?
                } else {
                    Vec::new()
                };
                let (_, left) = read_ics(&mut reader, Some(info), config.rate_hz)?;
                let (_, right) = read_ics(&mut reader, Some(info), config.rate_hz)?;
                element = Some(Element {
                    info,
                    ms: (present, used),
                    channels: vec![left, right],
                    sbr: Vec::new(),
                });
            }
            4 => read_data(&mut reader)?,
            6 => read_fill(&mut reader, &mut sbr)?,
            7 => break,
            _ => return None,
        }
    }
    let mut element = element?;
    element.sbr = sbr;
    Some(element)
}

fn ordered(channel: &Channel, slots: &[hcr::Slot]) -> Option<Vec<Codeword>> {
    let mut words = channel.words.clone();
    words.sort_by_key(|word| (word.window, word.line));
    slots
        .iter()
        .map(|slot| {
            words
                .binary_search_by_key(&(slot.window, slot.line), |word| (word.window, word.line))
                .ok()
                .map(|index| words[index])
        })
        .collect()
}

#[cfg(test)]
pub fn drm_frame(raw: &[u8], config: &AudioConfig) -> Result<Vec<u8>, &'static str> {
    drm_frame_padded(raw, config, 0)
}

pub fn drm_frame_padded(
    raw: &[u8],
    config: &AudioConfig,
    minimum: usize,
) -> Result<Vec<u8>, &'static str> {
    let invalid = "Unsupported AAC access unit";
    let mut element = read_element(raw, config).ok_or(invalid)?;
    if element.channels.len() != if config.stereo() { 2 } else { 1 } {
        return Err("AAC channel layout does not match the DRM audio mode");
    }
    let offsets = band_offsets(config.rate_hz, element.info.short()).ok_or(invalid)?;
    let source = BitReader::new(raw);
    let mut writer = BitWriter::with_capacity(raw.len() + 64);
    writer.bit(false);
    element.info.write_header(&mut writer);
    if config.stereo() {
        writer.put(element.ms.0, 2);
        for &used in &element.ms.1 {
            writer.bit(used);
        }
    }
    let mut spectra = Vec::with_capacity(2);
    for channel in &mut element.channels {
        resilient_sections(channel, element.info, offsets);
        let slots = hcr::order(element.info, &channel.sections, offsets);
        let words = ordered(channel, &slots).ok_or(invalid)?;
        let (bits, longest) = hcr::encode(&words, &slots).ok_or(invalid)?;
        if bits.len() >= 1 << 14 {
            return Err(invalid);
        }
        writer.bit(channel.tns.is_some());
        writer.bit(false);
        writer.put(u32::from(channel.gain), 8);
        write_sections(&mut writer, element.info, &channel.sections, true);
        writer.copy(
            &source,
            channel.scalefactors.start,
            channel.scalefactors.end,
        );
        writer.put(bits.len() as u32, 14);
        writer.put(longest as u32, 6);
        spectra.push(bits);
    }
    for channel in &element.channels {
        if let Some(tns) = &channel.tns {
            writer.copy(&source, tns.start, tns.end);
        }
    }
    let protected = BitReader::new(writer.bytes());
    let check = crc(
        0x1D,
        8,
        (0..writer.bit_len()).map(|at| protected.bit_at(at)),
    );
    for bits in &spectra {
        for &bit in bits {
            writer.bit(bit);
        }
    }
    let mut tail = Vec::new();
    if config.sbr && !element.sbr.is_empty() {
        let sbr_check = crc(0x1D, 8, element.sbr.iter().copied());
        tail.extend((0..8).rev().map(|shift| sbr_check >> shift & 1 == 1));
        tail.extend_from_slice(&element.sbr);
    }
    let total = ((writer.bit_len() + tail.len()).div_ceil(8) * 8).max(8 * minimum);
    while writer.bit_len() < total {
        writer.bit(false);
    }
    let mut frame = Vec::with_capacity(total / 8 + 1);
    frame.push(check as u8);
    frame.extend_from_slice(writer.bytes());
    for (index, &bit) in tail.iter().enumerate() {
        let at = total - 1 - index;
        let byte = &mut frame[1 + at / 8];
        let mask = 1 << (7 - at % 8);
        if bit {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }
    Ok(frame)
}
