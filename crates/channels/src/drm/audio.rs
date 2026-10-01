use std::ops::Range;

use super::{
    aac::AudioConfig,
    bits::{BitReader, byte_bits, crc},
};

pub const TEXT_BYTES: usize = 4;
const MAX_FRAMES: usize = 11;
const MAX_BORDERS: usize = 15;
const MAX_CARRY: usize = 16_384;

#[must_use]
pub const fn aac_frames(plus: bool, rate_hz: u32) -> Option<usize> {
    match (plus, rate_hz) {
        (false, 12_000) | (true, 24_000) => Some(5),
        (false, 24_000) | (true, 48_000) => Some(10),
        _ => None,
    }
}

const fn header_bytes(frames: usize) -> usize {
    ((frames - 1) * 12).div_ceil(8)
}

pub fn aac_superframe(
    superframe: &[u8],
    frames: usize,
    out: &mut Vec<(u8, Range<usize>)>,
) -> Result<(), &'static str> {
    out.clear();
    let header = header_bytes(frames);
    let payload_start = header + frames;
    let payload = superframe
        .len()
        .checked_sub(payload_start)
        .ok_or("Audio super frame too short")?;
    let mut reader = BitReader::new(&superframe[..header]);
    let mut previous = 0usize;
    if frames + 1 > MAX_FRAMES {
        return Err("Too many audio frames");
    }
    let mut starts = [0usize; MAX_FRAMES];
    for slot in &mut starts[1..frames] {
        let raw = reader.read(12).ok_or("Audio super frame too short")? as usize;
        let mut border = previous - previous % 4096 + raw;
        if border < previous {
            border += 4096;
        }
        if border <= previous || border > payload {
            return Err("Invalid audio frame borders");
        }
        *slot = border;
        previous = border;
    }
    starts[frames] = payload;
    for (index, pair) in starts[..=frames].windows(2).enumerate() {
        out.push((
            superframe[header + index],
            payload_start + pair[0]..payload_start + pair[1],
        ));
    }
    Ok(())
}

#[derive(Default)]
pub struct XheAssembler {
    carry: Vec<u8>,
    synced: bool,
}

impl XheAssembler {
    pub fn reset(&mut self) {
        self.carry.clear();
        self.synced = false;
    }

    pub fn push(
        &mut self,
        superframe: &[u8],
        mut emit: impl FnMut(&[u8]),
    ) -> Result<u32, &'static str> {
        let invalid = "Invalid xHE-AAC super frame";
        let (&first, rest) = superframe.split_first().ok_or(invalid)?;
        let (&check, _) = rest.split_first().ok_or(invalid)?;
        let count = usize::from(first >> 4);
        if crc(0x1D, 8, byte_bits(&[first])) != u32::from(check) {
            self.reset();
            return Err("xHE-AAC header CRC failure");
        }
        let directory = 2 * count;
        let end = superframe.len().checked_sub(directory).ok_or(invalid)?;
        let payload = superframe.get(2..end).ok_or(invalid)?;
        let base = self.carry.len();
        let mut borders = [0usize; MAX_BORDERS];
        for (slot, entry) in superframe[end..]
            .as_chunks::<2>()
            .0
            .iter()
            .rev()
            .enumerate()
        {
            let index = usize::from(entry[0]) << 4 | usize::from(entry[1] >> 4);
            let position = match index {
                0xFFE => base.checked_sub(2),
                0xFFF => base.checked_sub(1),
                _ => Some(base + index),
            };
            match position {
                Some(position) if position <= base + payload.len() => borders[slot] = position,
                _ => return Err(invalid),
            }
        }
        let borders = &mut borders[..count];
        borders.sort_unstable();
        if self.carry.len() + payload.len() > MAX_CARRY {
            self.reset();
            return Err("xHE-AAC frame too long");
        }
        self.carry.extend_from_slice(payload);
        let mut start = 0;
        let mut errors = 0;
        for &mut border in borders {
            if self.synced && border > start {
                let frame = &self.carry[start..border];
                if frame.len() > 2 {
                    let (body, stored) = frame.split_at(frame.len() - 2);
                    let stored = u32::from(stored[0]) << 8 | u32::from(stored[1]);
                    if crc(0x1021, 16, byte_bits(body)) == stored {
                        emit(body);
                    } else {
                        errors += 1;
                    }
                }
            }
            self.synced = true;
            start = border;
        }
        if self.synced {
            self.carry.drain(..start.min(self.carry.len()));
        } else {
            let keep = self.carry.len().saturating_sub(2);
            self.carry.drain(..keep);
        }
        Ok(errors)
    }
}

pub struct Superframes {
    plus: bool,
    first: Vec<u8>,
    waiting: bool,
}

impl Superframes {
    #[must_use]
    pub fn new(plus: bool) -> Self {
        Self {
            plus,
            first: Vec::new(),
            waiting: false,
        }
    }

    pub fn reset(&mut self) {
        self.first.clear();
        self.waiting = false;
    }

    pub fn push(&mut self, logical: &[u8], starts: bool, out: &mut Vec<u8>) -> bool {
        if !self.plus {
            out.clear();
            out.extend_from_slice(logical);
            return true;
        }
        if starts {
            self.first.clear();
            self.first.extend_from_slice(logical);
            self.waiting = true;
            return false;
        }
        if !self.waiting {
            return false;
        }
        self.waiting = false;
        out.clear();
        out.extend_from_slice(&self.first);
        out.extend_from_slice(logical);
        true
    }
}

#[must_use]
pub fn split_text<'a>(
    logical: &'a [u8],
    config: &AudioConfig,
) -> (&'a [u8], Option<[u8; TEXT_BYTES]>) {
    if !config.text || logical.len() < TEXT_BYTES {
        return (logical, None);
    }
    let (audio, text) = logical.split_at(logical.len() - TEXT_BYTES);
    let mut piece = [0u8; TEXT_BYTES];
    piece.copy_from_slice(text);
    (audio, Some(piece))
}

#[cfg(any(test, feature = "synth"))]
pub fn build_aac_superframe(frames: &[Vec<u8>], length: usize) -> Option<Vec<u8>> {
    let count = frames.len();
    let header = header_bytes(count);
    let mut writer = super::bits::BitWriter::with_capacity(length);
    let mut border = 0usize;
    for frame in &frames[..count - 1] {
        border += frame.len() - 1;
        writer.put((border % 4096) as u32, 12);
    }
    writer.align();
    let mut out = writer.into_bytes();
    out.resize(header, 0);
    out.extend(frames.iter().map(|frame| frame[0]));
    for frame in frames {
        out.extend_from_slice(&frame[1..]);
    }
    if out.len() > length {
        return None;
    }
    out.resize(length, 0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_superframes_split_at_their_borders() {
        let frames: Vec<Vec<u8>> = (0..10u8)
            .map(|index| {
                let mut frame = vec![0xC0 | index];
                frame.extend(std::iter::repeat_n(index, 30 + usize::from(index)));
                frame
            })
            .collect();
        let superframe = build_aac_superframe(&frames, 500).expect("fits");
        let mut out = Vec::new();
        aac_superframe(&superframe, 10, &mut out).expect("valid");
        for (index, (check, range)) in out.iter().enumerate().take(9) {
            assert_eq!(*check, 0xC0 | index as u8);
            assert_eq!(&superframe[range.clone()], &frames[index][1..]);
        }
        assert!(superframe[out[9].1.clone()].starts_with(&frames[9][1..]));
    }

    #[test]
    fn broken_borders_are_rejected() {
        let mut superframe = vec![0u8; 200];
        superframe[0] = 0xFF;
        superframe[1] = 0xF0;
        let mut out = Vec::new();
        assert!(aac_superframe(&superframe, 5, &mut out).is_err());
    }

    fn xhe_superframe(payload: &[u8], borders: &[u16]) -> Vec<u8> {
        let count = borders.len() as u8;
        let first = count << 4 | 3;
        let mut out = vec![first, crc(0x1D, 8, byte_bits(&[first])) as u8];
        out.extend_from_slice(payload);
        for &border in borders.iter().rev() {
            out.push((border >> 4) as u8);
            out.push(((border & 0xF) as u8) << 4 | count);
        }
        out
    }

    fn usac_frame(body: &[u8]) -> Vec<u8> {
        let mut frame = body.to_vec();
        let check = crc(0x1021, 16, byte_bits(body)) as u16;
        frame.extend_from_slice(&check.to_be_bytes());
        frame
    }

    #[test]
    fn xhe_frames_span_superframes() {
        let one = usac_frame(&[1; 10]);
        let two = usac_frame(&[2; 30]);
        let three = usac_frame(&[3; 5]);
        let mut stream = vec![9, 9, 9];
        let first_border = stream.len() as u16;
        stream.extend_from_slice(&one);
        let second_border = stream.len() as u16;
        stream.extend_from_slice(&two);
        let third = stream.len();
        stream.extend_from_slice(&three);
        stream.extend_from_slice(&[7, 7]);
        let mut assembler = XheAssembler::default();
        let mut frames = Vec::new();
        let first = xhe_superframe(&stream[..30], &[first_border, second_border]);
        assert_eq!(
            assembler.push(&first, |frame| frames.push(frame.to_vec())),
            Ok(0)
        );
        assert_eq!(frames, vec![one[..10].to_vec()]);
        let second = xhe_superframe(
            &stream[30..],
            &[(third - 30) as u16, (stream.len() - 2 - 30) as u16],
        );
        frames.clear();
        assert_eq!(
            assembler.push(&second, |frame| frames.push(frame.to_vec())),
            Ok(0)
        );
        assert_eq!(frames, vec![two[..30].to_vec(), three[..5].to_vec()]);
    }
}
