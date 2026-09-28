use super::super::label::text;

mod content;
mod parameter;

#[cfg(test)]
mod tests;

pub use content::ContentType;

use parameter::{Parameter, Parameters};

const CORE_LENGTH: usize = 7;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Header {
    pub body_size: usize,
    pub content_type: ContentType,
    pub content_name: Option<String>,
    pub mime_type: Option<String>,
    pub compressed: bool,
    pub scrambled: bool,
}

impl Header {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        let Some((core, extension)) = bytes.split_first_chunk::<CORE_LENGTH>() else {
            return Err("Truncated MOT header core");
        };
        let mut word = [0; 8];
        word[1..].copy_from_slice(core);
        let core = u64::from_be_bytes(word);
        if field(core, 15, 13) != bytes.len() {
            return Err("MOT header size mismatch");
        }
        let mut header = Self {
            body_size: field(core, 28, 28),
            content_type: ContentType::new(field(core, 9, 6), field(core, 0, 9)),
            ..Self::default()
        };
        for parameter in Parameters::new(extension) {
            header.apply(parameter?)?;
        }
        Ok(header)
    }

    pub fn media_type(&self) -> &str {
        self.mime_type
            .as_deref()
            .unwrap_or_else(|| self.content_type.media_type())
    }

    fn apply(&mut self, parameter: Parameter) -> Result<(), &'static str> {
        match parameter {
            Parameter::ContentName(data) => self.content_name = Some(content_name(data)?),
            Parameter::MimeType(data) => self.mime_type = Some(mime_type(data)?),
            Parameter::CompressionType => self.compressed = true,
            Parameter::CaInfo => self.scrambled = true,
            Parameter::Other => {}
        }
        Ok(())
    }
}

fn field(word: u64, shift: u32, width: u32) -> usize {
    ((word >> shift) & ((1 << width) - 1)) as usize
}

fn content_name(data: &[u8]) -> Result<String, &'static str> {
    let Some((&indicator, name)) = data.split_first() else {
        return Err("Empty MOT ContentName");
    };
    text(name, indicator >> 4)
}

fn mime_type(data: &[u8]) -> Result<String, &'static str> {
    std::str::from_utf8(data)
        .ok()
        .filter(|mime| mime.is_ascii())
        .map(str::to_owned)
        .ok_or("Invalid MOT MimeType")
}
