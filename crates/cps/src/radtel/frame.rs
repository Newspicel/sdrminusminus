use crate::CpsError;

pub const ACK: u8 = 0x06;
pub const PAGE_BYTES: usize = 1024;
pub const PAGE_REPLY_BYTES: usize = PAGE_BYTES + 4;

const SESSION_PREFIX: [u8; 3] = [0x34, 0x52, 0x05];
const ENTER_CODE: u8 = 0x10;
const LEAVE_CODE: u8 = 0xee;
const READ_CODE: u8 = 0x52;
const WRITE_CODE: u8 = 0x09;

#[derive(Clone, Copy, Debug)]
pub enum Command<'a> {
    Enter,
    Leave,
    ReadPage {
        page: u16,
    },
    WritePage {
        segment: u8,
        page: u16,
        data: &'a [u8],
    },
}

pub fn sum8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

impl Command<'_> {
    pub fn to_bytes(self) -> Vec<u8> {
        let mut frame = self.body();
        frame.push(sum8(&frame));
        frame
    }

    fn body(self) -> Vec<u8> {
        match self {
            Self::Enter => session_body(ENTER_CODE),
            Self::Leave => session_body(LEAVE_CODE),
            Self::ReadPage { page } => {
                let [hi, lo] = page.to_be_bytes();
                vec![READ_CODE, hi, lo]
            }
            Self::WritePage {
                segment,
                page,
                data,
            } => write_body(segment, page, data),
        }
    }
}

fn session_body(code: u8) -> Vec<u8> {
    let mut body = SESSION_PREFIX.to_vec();
    body.push(code);
    body
}

fn write_body(segment: u8, page: u16, data: &[u8]) -> Vec<u8> {
    let [hi, lo] = page.to_be_bytes();
    let mut body = Vec::with_capacity(PAGE_BYTES + 5);
    body.extend_from_slice(&[WRITE_CODE, segment, hi, lo]);
    body.extend(data.iter().copied().take(PAGE_BYTES));
    body.resize(PAGE_BYTES + 4, 0);
    body
}

pub fn page_payload(reply: &[u8; PAGE_REPLY_BYTES]) -> Result<&[u8], CpsError> {
    let (framed, checksum) = reply.split_at(PAGE_REPLY_BYTES - 1);
    if framed[0] != READ_CODE {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("reply starts with {:#04x}", framed[0]),
        });
    }
    if sum8(framed) != checksum[0] {
        return Err(CpsError::Protocol {
            step: "read",
            reason: "page checksum mismatch".to_owned(),
        });
    }
    Ok(&framed[3..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_frames_carry_their_checksum() {
        assert_eq!(Command::Enter.to_bytes(), [0x34, 0x52, 0x05, 0x10, 0x9b]);
        assert_eq!(Command::Leave.to_bytes(), [0x34, 0x52, 0x05, 0xee, 0x79]);
        assert_eq!(
            Command::ReadPage { page: 0x0108 }.to_bytes(),
            [0x52, 0x01, 0x08, 0x5b]
        );
    }

    #[test]
    fn a_short_write_is_padded_to_a_full_page() {
        let frame = Command::WritePage {
            segment: 3,
            page: 1,
            data: &[0xaa],
        }
        .to_bytes();
        assert_eq!(frame.len(), PAGE_BYTES + 5);
        assert_eq!(&frame[..5], &[0x09, 0x03, 0x00, 0x01, 0xaa]);
        assert!(frame[5..PAGE_BYTES + 4].iter().all(|byte| *byte == 0));
        assert_eq!(frame[PAGE_BYTES + 4], 0xb7);
    }

    #[test]
    fn a_reply_with_a_foreign_opcode_is_refused() {
        let mut reply = [0u8; PAGE_REPLY_BYTES];
        reply[0] = 0x51;
        reply[PAGE_REPLY_BYTES - 1] = 0x51;
        assert!(matches!(
            page_payload(&reply),
            Err(CpsError::Protocol { step: "read", .. })
        ));
    }
}
