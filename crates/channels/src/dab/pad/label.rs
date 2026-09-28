use super::crc_ok;

mod charset;
mod message;

#[cfg(test)]
mod tests;

pub use charset::text;

use message::Message;

const PREFIX_LENGTH: usize = 2;
const CRC_LENGTH: usize = 2;
const CLEAR_DISPLAY: u8 = 0b0001;
const DL_PLUS: u8 = 0b0010;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    ClearDisplay,
    DlPlus { length: usize },
    Reserved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Characters { length: usize, field_2: u8 },
    Command(Command),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Prefix {
    toggle: bool,
    first: bool,
    last: bool,
    control: Control,
}

impl Prefix {
    fn read([high, low]: [u8; 2]) -> Self {
        let control = if high & 0x10 == 0 {
            Control::Characters {
                length: usize::from(high & 0x0f) + 1,
                field_2: low >> 4,
            }
        } else {
            Control::Command(match high & 0x0f {
                CLEAR_DISPLAY => Command::ClearDisplay,
                DL_PLUS => Command::DlPlus {
                    length: usize::from(low & 0x0f) + 1,
                },
                _ => Command::Reserved,
            })
        };
        Self {
            toggle: high & 0x80 != 0,
            first: high & 0x40 != 0,
            last: high & 0x20 != 0,
            control,
        }
    }

    fn data_length(self) -> Option<usize> {
        match self.control {
            Control::Characters { length, .. } | Control::Command(Command::DlPlus { length }) => {
                Some(length)
            }
            Control::Command(Command::ClearDisplay) => Some(0),
            Control::Command(Command::Reserved) => None,
        }
    }
}

#[derive(Default)]
pub struct Label {
    group: Vec<u8>,
    collecting: bool,
    message: Message,
    shown: Option<String>,
}

impl Label {
    pub fn push(&mut self, start: bool, bytes: &[u8]) -> Result<Option<String>, &'static str> {
        if start {
            self.group.clear();
            self.collecting = true;
        }
        if !self.collecting {
            return Ok(None);
        }
        self.group.extend_from_slice(bytes);
        let Some(&prefix) = self.group.first_chunk::<PREFIX_LENGTH>() else {
            return Ok(None);
        };
        let prefix = Prefix::read(prefix);
        let Some(data_length) = prefix.data_length() else {
            self.collecting = false;
            return Err("Reserved dynamic-label command");
        };
        let total = PREFIX_LENGTH + data_length + CRC_LENGTH;
        if self.group.len() < total {
            return Ok(None);
        }
        self.collecting = false;
        self.group.truncate(total);
        if !crc_ok(&self.group) {
            return Err("DAB dynamic-label CRC failure");
        }
        let data = std::mem::take(&mut self.group);
        let result = self.segment(prefix, &data[PREFIX_LENGTH..PREFIX_LENGTH + data_length]);
        self.group = data;
        result
    }

    fn segment(&mut self, prefix: Prefix, data: &[u8]) -> Result<Option<String>, &'static str> {
        match prefix.control {
            Control::Characters { field_2, .. } => {
                let Some((bytes, charset)) = self.message.insert(prefix, field_2, data)? else {
                    return Ok(None);
                };
                Ok(self.show(text(&bytes, charset)?))
            }
            Control::Command(Command::ClearDisplay) => Ok(self.show(String::new())),
            Control::Command(Command::DlPlus { .. } | Command::Reserved) => Ok(None),
        }
    }

    fn show(&mut self, text: String) -> Option<String> {
        if self.shown.as_ref() == Some(&text) {
            return None;
        }
        self.shown = Some(text.clone());
        Some(text)
    }
}
