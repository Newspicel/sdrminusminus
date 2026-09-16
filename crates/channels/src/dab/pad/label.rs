use super::crc_ok;

#[derive(Default)]
pub struct Label {
    pending: Vec<u8>,
    segments: [Option<Vec<u8>>; 8],
    toggle: Option<bool>,
    last: Option<usize>,
    charset: u8,
}

pub fn text(bytes: &[u8], charset: u8) -> Result<String, &'static str> {
    match charset {
        0 => Ok(crate::dab::fig::ebu_text(bytes)),
        6 if bytes.len().is_multiple_of(2) => {
            let words: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_be_bytes(*b))
                .collect();
            String::from_utf16(&words).map_err(|_| "Invalid DAB UCS-2 text")
        }
        15 => String::from_utf8(bytes.to_vec()).map_err(|_| "Invalid DAB UTF-8 text"),
        _ => Err("Unsupported DAB text character set"),
    }
}

impl Label {
    pub fn push(&mut self, start: bool, bytes: &[u8]) -> Result<Option<String>, &'static str> {
        if start {
            self.pending.clear();
        } else if self.pending.is_empty() {
            return Ok(None);
        }
        self.pending.extend(
            bytes
                .iter()
                .take(20usize.saturating_sub(self.pending.len())),
        );
        if self.pending.len() < 2 {
            return Ok(None);
        }
        let first = self.pending[0];
        let second = self.pending[1];
        let command = first & 16 != 0;
        let length = if command {
            match first & 15 {
                1 => 0,
                2 => usize::from(second & 15) + 1,
                _ => {
                    self.pending.clear();
                    return Ok(None);
                }
            }
        } else {
            usize::from(first & 15) + 1
        };
        if self.pending.len() < length + 4 {
            return Ok(None);
        }
        if !crc_ok(&self.pending[..length + 4]) {
            self.pending.clear();
            return Err("DAB dynamic-label CRC failure");
        }
        if command {
            self.pending.clear();
            if first & 15 == 1 {
                *self = Self::default();
                return Ok(Some(String::new()));
            }
            return Ok(None);
        }
        let toggle = first & 128 != 0;
        if self.toggle != Some(toggle) {
            self.segments.fill(None);
            self.last = None;
            self.toggle = Some(toggle);
        }
        let index = if first & 64 != 0 {
            self.charset = second >> 4;
            0
        } else {
            usize::from((second >> 4) & 7)
        };
        let changed = self.segments[index].as_deref() != Some(&self.pending[2..2 + length]);
        self.segments[index] = Some(self.pending[2..2 + length].to_vec());
        self.pending.clear();
        if first & 32 != 0 {
            self.last = Some(index);
        }
        let Some(last) = self.last else {
            return Ok(None);
        };
        if !changed || self.segments[..=last].iter().any(Option::is_none) {
            return Ok(None);
        }
        let mut complete = Vec::with_capacity(128);
        for segment in self.segments[..=last].iter().flatten() {
            complete.extend_from_slice(segment);
        }
        text(&complete, self.charset).map(Some)
    }
}
