use super::bits::{byte_bits, crc};

const MARKER: [u8; 4] = [0xFF; 4];
const SEGMENTS: usize = 8;
const BODY: usize = 16;
const SEGMENT: usize = 2 + BODY + 2 + 4;
const CLEAR: u8 = 0b0001;

#[derive(Default)]
pub struct TextMessage {
    segment: [u8; SEGMENT],
    used: usize,
    collecting: bool,
    toggle: Option<bool>,
    parts: [[u8; BODY]; SEGMENTS],
    lengths: [Option<u8>; SEGMENTS],
    last: Option<usize>,
    pub message: Option<String>,
    pub segments_ok: u32,
    pub segments_bad: u32,
}

impl TextMessage {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn push(&mut self, piece: [u8; 4]) {
        if piece == MARKER {
            self.used = 0;
            self.collecting = true;
            return;
        }
        if !self.collecting || self.used + piece.len() > SEGMENT {
            self.collecting = false;
            return;
        }
        self.segment[self.used..self.used + piece.len()].copy_from_slice(&piece);
        self.used += piece.len();
        let [high, low] = [self.segment[0], self.segment[1]];
        let command = high & 0x10 != 0;
        let body = if command {
            if high & 0x0F != CLEAR {
                self.collecting = false;
                return;
            }
            0
        } else {
            usize::from(high & 0x0F) + 1
        };
        let total = 2 + body + 2;
        if self.used < total {
            return;
        }
        self.collecting = false;
        let (data, stored) = self.segment[..total].split_at(total - 2);
        let stored = u32::from(stored[0]) << 8 | u32::from(stored[1]);
        if crc(0x1021, 16, byte_bits(data)) != stored {
            self.segments_bad = self.segments_bad.saturating_add(1);
            return;
        }
        self.segments_ok = self.segments_ok.saturating_add(1);
        if command {
            self.message = None;
            return;
        }
        let mut part = [0u8; BODY];
        part[..body].copy_from_slice(&data[2..]);
        self.segment_complete(high, low, part, body);
    }

    fn segment_complete(&mut self, high: u8, low: u8, part: [u8; BODY], length: usize) {
        let toggle = high & 0x80 != 0;
        let first = high & 0x40 != 0;
        let last = high & 0x20 != 0;
        if self.toggle != Some(toggle) {
            self.toggle = Some(toggle);
            self.lengths = [None; SEGMENTS];
            self.last = None;
        }
        let index = if first {
            0
        } else {
            usize::from(low >> 4 & 0x07)
        };
        self.parts[index] = part;
        self.lengths[index] = Some(length as u8);
        if last {
            self.last = Some(index);
        }
        let Some(last) = self.last else {
            return;
        };
        let mut bytes = [0u8; SEGMENTS * BODY];
        let mut used = 0;
        for (part, length) in self.parts[..=last].iter().zip(&self.lengths[..=last]) {
            let Some(length) = length else {
                return;
            };
            let length = usize::from(*length);
            bytes[used..used + length].copy_from_slice(&part[..length]);
            used += length;
        }
        let text = String::from_utf8_lossy(&bytes[..used]);
        let text = text.trim_matches(char::is_control);
        if self.message.as_deref() != Some(text) {
            self.message = Some(text.to_owned());
        }
    }
}

#[cfg(any(test, feature = "synth"))]
#[must_use]
pub fn pieces(message: &str, toggle: bool) -> Vec<[u8; 4]> {
    let bytes = message.as_bytes();
    let chunks: Vec<&[u8]> = bytes.chunks(16).collect();
    let mut out = Vec::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let first = index == 0;
        let last = index + 1 == chunks.len();
        let high = u8::from(toggle) << 7
            | u8::from(first) << 6
            | u8::from(last) << 5
            | (chunk.len() as u8 - 1);
        let low = if first { 0xF0 } else { (index as u8) << 4 };
        let mut segment = vec![high, low];
        segment.extend_from_slice(chunk);
        let check = crc(0x1021, 16, byte_bits(&segment)) as u16;
        segment.extend_from_slice(&check.to_be_bytes());
        out.push(MARKER);
        for piece in segment.chunks(4) {
            let mut padded = [0u8; 4];
            padded[..piece.len()].copy_from_slice(piece);
            out.push(padded);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segmented_messages_reassemble() {
        let mut text = TextMessage::default();
        for piece in pieces("Rust on the short waves, live", false) {
            text.push(piece);
        }
        assert_eq!(
            text.message.as_deref(),
            Some("Rust on the short waves, live")
        );
        assert_eq!(text.segments_ok, 2);
    }

    #[test]
    fn damaged_segments_are_counted() {
        let mut text = TextMessage::default();
        let mut pieces = pieces("Hello", true);
        pieces[1][2] ^= 0x01;
        for piece in pieces {
            text.push(piece);
        }
        assert_eq!(text.message, None);
        assert_eq!(text.segments_bad, 1);
    }
}
