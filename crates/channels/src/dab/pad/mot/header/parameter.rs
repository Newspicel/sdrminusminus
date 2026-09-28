const CONTENT_NAME: u8 = 0b00_1100;
const MIME_TYPE: u8 = 0b01_0000;
const COMPRESSION_TYPE: u8 = 0b01_0001;
const CA_INFO: u8 = 0b10_0011;
const PARAM_ID_MASK: u8 = 0x3f;
const EXTENSION_FLAG: u8 = 0x80;
const SHORT_LENGTH_MASK: u8 = 0x7f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parameter<'a> {
    ContentName(&'a [u8]),
    MimeType(&'a [u8]),
    CompressionType,
    CaInfo,
    Other,
}

impl<'a> Parameter<'a> {
    fn new(id: u8, data: &'a [u8]) -> Self {
        match id {
            CONTENT_NAME => Self::ContentName(data),
            MIME_TYPE => Self::MimeType(data),
            COMPRESSION_TYPE => Self::CompressionType,
            CA_INFO => Self::CaInfo,
            _ => Self::Other,
        }
    }
}

pub struct Parameters<'a> {
    rest: &'a [u8],
}

impl<'a> Parameters<'a> {
    pub fn new(extension: &'a [u8]) -> Self {
        Self { rest: extension }
    }

    fn read(&mut self) -> Result<Parameter<'a>, &'static str> {
        let Some((&first, rest)) = self.rest.split_first() else {
            return Err("Empty MOT header parameter");
        };
        let (length, rest) = match first >> 6 {
            0b00 => (0, rest),
            0b01 => (1, rest),
            0b10 => (4, rest),
            _ => data_field_length(rest)?,
        };
        let (data, rest) = rest
            .split_at_checked(length)
            .ok_or("Truncated MOT header parameter")?;
        self.rest = rest;
        Ok(Parameter::new(first & PARAM_ID_MASK, data))
    }
}

impl<'a> Iterator for Parameters<'a> {
    type Item = Result<Parameter<'a>, &'static str>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        let parameter = self.read();
        if parameter.is_err() {
            self.rest = &[];
        }
        Some(parameter)
    }
}

fn data_field_length(bytes: &[u8]) -> Result<(usize, &[u8]), &'static str> {
    match bytes {
        [short, rest @ ..] if short & EXTENSION_FLAG == 0 => Ok((usize::from(*short), rest)),
        [high, low, rest @ ..] => Ok((
            usize::from(u16::from_be_bytes([high & SHORT_LENGTH_MASK, *low])),
            rest,
        )),
        _ => Err("Truncated MOT DataFieldLength indicator"),
    }
}
