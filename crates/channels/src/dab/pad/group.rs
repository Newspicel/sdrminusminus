use super::crc_ok;

const INDICATOR_LENGTH: usize = 4;
const GROUP_LENGTH_MASK: u16 = 0x3fff;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Indicator {
    #[default]
    Idle,
    Reading {
        bytes: [u8; INDICATOR_LENGTH],
        filled: usize,
    },
    Announced(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Group {
    #[default]
    Idle,
    Filling(usize),
}

#[derive(Default)]
pub struct Assembler {
    indicator: Indicator,
    group: Group,
    buffer: Vec<u8>,
}

impl Assembler {
    pub fn length_indicator(&mut self, start: bool, bytes: &[u8]) -> Result<(), &'static str> {
        let (mut received, mut filled) = match (start, self.indicator) {
            (true, _) => ([0; INDICATOR_LENGTH], 0),
            (false, Indicator::Reading { bytes, filled }) => (bytes, filled),
            (false, _) => return Ok(()),
        };
        for (slot, &byte) in received[filled..].iter_mut().zip(bytes) {
            *slot = byte;
            filled += 1;
        }
        self.indicator = Indicator::Idle;
        if filled < INDICATOR_LENGTH {
            self.indicator = Indicator::Reading {
                bytes: received,
                filled,
            };
            return Ok(());
        }
        if !crc_ok(&received) {
            return Err("X-PAD data group length indicator CRC failure");
        }
        let length = u16::from_be_bytes([received[0], received[1]]) & GROUP_LENGTH_MASK;
        self.indicator = Indicator::Announced(usize::from(length));
        Ok(())
    }

    pub fn interrupt(&mut self) {
        self.indicator = Indicator::Idle;
    }

    pub fn abandon(&mut self) -> bool {
        std::mem::take(&mut self.group) != Group::Idle
    }

    pub fn begin(&mut self) -> Result<(), &'static str> {
        let Indicator::Announced(length) = std::mem::take(&mut self.indicator) else {
            return Err("X-PAD MSC data group without length indicator");
        };
        if length == 0 {
            return Err("Empty X-PAD MSC data group");
        }
        self.buffer.clear();
        self.group = Group::Filling(length);
        Ok(())
    }

    pub fn extend(&mut self, bytes: &[u8]) -> Option<&[u8]> {
        let Group::Filling(length) = self.group else {
            return None;
        };
        let missing = length - self.buffer.len();
        self.buffer.extend(bytes.iter().take(missing));
        if self.buffer.len() < length {
            return None;
        }
        self.group = Group::Idle;
        Some(&self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::crc16_msb;

    use super::*;

    fn indicator(length: u16) -> Vec<u8> {
        let mut bytes = length.to_be_bytes().to_vec();
        bytes.extend((!crc16_msb(0x1021, 0xffff, &bytes)).to_be_bytes());
        bytes
    }

    #[test]
    fn indicator_split_across_subfields_announces_the_length() {
        let mut assembler = Assembler::default();
        let bytes = indicator(0xc005);
        assert_eq!(assembler.length_indicator(true, &bytes[..3]), Ok(()));
        assert_eq!(
            assembler.length_indicator(false, &[bytes[3], 0, 0, 0]),
            Ok(())
        );
        assert_eq!(assembler.indicator, Indicator::Announced(5));
    }

    #[test]
    fn damaged_indicator_is_reported() {
        let mut assembler = Assembler::default();
        let mut bytes = indicator(5);
        bytes[1] ^= 1;
        assert!(assembler.length_indicator(true, &bytes).is_err());
        assert!(assembler.begin().is_err());
    }

    #[test]
    fn group_completes_at_the_announced_length_and_drops_padding() {
        let mut assembler = Assembler::default();
        assembler.length_indicator(true, &indicator(5)).ok();
        assert_eq!(assembler.begin(), Ok(()));
        assert_eq!(assembler.extend(&[1, 2, 3]), None);
        assert_eq!(assembler.extend(&[4, 5, 0, 0]), Some(&[1, 2, 3, 4, 5][..]));
        assert_eq!(assembler.extend(&[6]), None);
    }

    #[test]
    fn announced_length_is_bound_to_the_next_subfield() {
        let mut assembler = Assembler::default();
        assembler.length_indicator(true, &indicator(5)).ok();
        assembler.interrupt();
        assert!(assembler.begin().is_err());
    }

    #[test]
    fn restart_reports_an_unfinished_group() {
        let mut assembler = Assembler::default();
        assembler.length_indicator(true, &indicator(5)).ok();
        assembler.begin().ok();
        assembler.extend(&[1]);
        assert!(assembler.abandon());
        assert!(!assembler.abandon());
    }
}
