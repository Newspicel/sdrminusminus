use std::time::Duration;

use sdrmm_wire::cps::RadioIdent;

use crate::{CpsError, RadioSession, SerialLink};

pub const PAGE: usize = 0x400;
const TIMEOUT: Duration = Duration::from_millis(5000);
const ACK: u8 = 0x06;
const IDENT_PAGE: u32 = 0x0000_2000;
const IDENT_MAGIC: u16 = 0xabcd;
const IDENT_MAGIC_AT: usize = 12;

pub const SEGMENTS: [(u32, u32); 9] = [
    (0x0_2000, 0x0_0400),
    (0x0_4000, 0x0_c000),
    (0x1_c000, 0x0_0400),
    (0x1_e000, 0x2_0000),
    (0x5_e000, 0x3_4000),
    (0xc_6000, 0x0_5000),
    (0xd_0000, 0x0_3000),
    (0xd_6000, 0x0_1000),
    (0xf_0000, 0x0_1000),
];

#[must_use]
pub fn segment_of(address: u32) -> Option<(u8, u32)> {
    SEGMENTS
        .iter()
        .enumerate()
        .find(|(_, (start, len))| address >= *start && address < start + len)
        .and_then(|(index, (start, _))| {
            u8::try_from(index)
                .ok()
                .map(|index| (index, address - start))
        })
}

fn crc(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

pub struct Rt4DSession {
    link: Box<dyn SerialLink>,
    model_id: String,
    identified: bool,
    closed: bool,
}

impl Rt4DSession {
    pub fn open(link: Box<dyn SerialLink>, model_id: impl Into<String>) -> Result<Self, CpsError> {
        let mut session = Self {
            link,
            model_id: model_id.into(),
            identified: false,
            closed: false,
        };
        session.link.set_control_lines(true)?;
        session.link.discard_input()?;
        session.enter_program_mode()?;
        Ok(session)
    }

    fn enter_program_mode(&mut self) -> Result<(), CpsError> {
        if self.identified {
            return Ok(());
        }
        let mut request = [b'4', b'R', 0x05, 0x10, 0x00];
        request[4] = crc(&request[..4]);
        let mut ack = [0u8; 1];
        self.link.send(&request)?;
        self.link.receive(&mut ack, TIMEOUT)?;
        if ack[0] != ACK {
            return Err(CpsError::Protocol {
                step: "enter program mode",
                reason: format!("the radio answered {:#04x}", ack[0]),
            });
        }
        let mut page = [0u8; PAGE];
        self.read_page(IDENT_PAGE, &mut page)?;
        let magic = u16::from_le_bytes([page[IDENT_MAGIC_AT], page[IDENT_MAGIC_AT + 1]]);
        if magic != IDENT_MAGIC {
            return Err(CpsError::Protocol {
                step: "identify",
                reason: format!("unknown firmware signature {magic:#06x}"),
            });
        }
        self.identified = true;
        Ok(())
    }

    fn read_page(&mut self, address: u32, page: &mut [u8; PAGE]) -> Result<(), CpsError> {
        let index = u16::try_from(address >> 10).map_err(|_| CpsError::Protocol {
            step: "read",
            reason: format!("{address:#010x} is past the addressable pages"),
        })?;
        let mut request = [b'R', 0, 0, 0];
        request[1..3].copy_from_slice(&index.to_be_bytes());
        request[3] = crc(&request[..3]);
        let mut response = vec![0u8; PAGE + 4];
        self.link.send(&request)?;
        self.link.receive(&mut response, TIMEOUT)?;
        if response[0] != b'R' {
            return Err(CpsError::Protocol {
                step: "read",
                reason: format!("expected header 'R', got {:#04x}", response[0]),
            });
        }
        let sum = crc(&response[..PAGE + 3]);
        if sum != response[PAGE + 3] {
            return Err(CpsError::Protocol {
                step: "read",
                reason: format!(
                    "checksum {sum:#04x} does not match {:#04x}",
                    response[PAGE + 3]
                ),
            });
        }
        page.copy_from_slice(&response[3..PAGE + 3]);
        Ok(())
    }
}

impl RadioSession for Rt4DSession {
    fn identify(&mut self) -> Result<RadioIdent, CpsError> {
        self.enter_program_mode()?;
        Ok(RadioIdent {
            reported_model: "RT4D".to_owned(),
            firmware: None,
            bands: None,
            model_id: Some(self.model_id.clone()),
        })
    }

    fn block_size(&self) -> u32 {
        PAGE as u32
    }

    fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError> {
        self.enter_program_mode()?;
        for (index, chunk) in buffer.chunks_mut(PAGE).enumerate() {
            let mut page = [0u8; PAGE];
            self.read_page(addr + (index * PAGE) as u32, &mut page)?;
            let take = chunk.len().min(PAGE);
            chunk[..take].copy_from_slice(&page[..take]);
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), CpsError> {
        self.enter_program_mode()?;
        for (index, chunk) in data.chunks(PAGE).enumerate() {
            let at = addr + (index * PAGE) as u32;
            let Some((segment, offset)) = segment_of(at) else {
                return Err(CpsError::Protocol {
                    step: "write",
                    reason: format!("{at:#010x} is outside every writable segment"),
                });
            };
            let page = u16::try_from(offset >> 10).map_err(|_| CpsError::Protocol {
                step: "write",
                reason: format!("{at:#010x} is past the addressable pages"),
            })?;
            let mut request = vec![0u8; PAGE + 5];
            request[0] = 9;
            request[1] = segment;
            request[2..4].copy_from_slice(&page.to_be_bytes());
            request[4..4 + chunk.len()].copy_from_slice(chunk);
            request[PAGE + 4] = crc(&request[..PAGE + 4]);
            let mut ack = [0u8; 1];
            self.link.send(&request)?;
            self.link.receive(&mut ack, TIMEOUT)?;
            if ack[0] != ACK {
                return Err(CpsError::Protocol {
                    step: "write",
                    reason: format!("the radio answered {:#04x} at {at:#010x}", ack[0]),
                });
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CpsError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut request = [b'4', b'R', 0x05, 0xee, 0x00];
        request[4] = crc(&request[..4]);
        self.link.send(&request)?;
        let mut ack = [0u8; 1];
        let _ = self.link.receive(&mut ack, TIMEOUT);
        self.link.set_control_lines(false)
    }
}

impl Drop for Rt4DSession {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::fixture::ScriptedLink;

    fn read_frame(index: u16, fill: u8) -> Vec<u8> {
        let mut frame = vec![0u8; PAGE + 4];
        frame[0] = b'R';
        frame[1..3].copy_from_slice(&index.to_be_bytes());
        frame[3..PAGE + 3].fill(fill);
        frame[PAGE + 3] = crc(&frame[..PAGE + 3]);
        frame
    }

    fn ident_frame() -> Vec<u8> {
        let mut frame = read_frame((IDENT_PAGE >> 10) as u16, 0);
        frame[3 + IDENT_MAGIC_AT] = 0xcd;
        frame[3 + IDENT_MAGIC_AT + 1] = 0xab;
        frame[PAGE + 3] = crc(&frame[..PAGE + 3]);
        frame
    }

    #[test]
    fn the_segment_map_places_every_documented_bank() {
        assert_eq!(segment_of(0x0_4000), Some((1, 0)));
        assert_eq!(segment_of(0x0_4400), Some((1, 0x400)));
        assert_eq!(segment_of(0x5_e000), Some((4, 0)));
        assert_eq!(segment_of(0x0_3000), None);
    }

    #[test]
    fn opening_enters_program_mode_and_checks_the_firmware_signature() {
        let link = ScriptedLink::new(vec![vec![ACK], ident_frame()]);
        let mut session = Rt4DSession::open(Box::new(link), "radtel-rt4d").expect("open");
        let ident = session.identify().expect("identify");
        assert_eq!(ident.reported_model, "RT4D");
    }

    #[test]
    fn a_foreign_firmware_signature_is_refused() {
        let link = ScriptedLink::new(vec![vec![ACK], read_frame(8, 0)]);
        let Err(error) = Rt4DSession::open(Box::new(link), "radtel-rt4d") else {
            panic!("a foreign signature must not open a session");
        };
        assert!(
            matches!(
                error,
                CpsError::Protocol {
                    step: "identify",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_page_read_is_checked_against_its_own_checksum() {
        let mut link = ScriptedLink::new(vec![vec![ACK], ident_frame()]);
        link.push_response(read_frame(0x10, 0x77));
        let mut session = Rt4DSession::open(Box::new(link), "radtel-rt4d").expect("open");
        let mut buffer = [0u8; PAGE];
        session.read(0x0_4000, &mut buffer).expect("read");
        assert_eq!(buffer, [0x77; PAGE]);
    }

    #[test]
    fn a_write_outside_the_segment_map_is_refused_instead_of_being_sent() {
        let link = ScriptedLink::new(vec![vec![ACK], ident_frame()]);
        let mut session = Rt4DSession::open(Box::new(link), "radtel-rt4d").expect("open");
        let error = session
            .write(0x0_3000, &[0u8; PAGE])
            .expect_err("unmapped address");
        assert!(
            matches!(error, CpsError::Protocol { step: "write", .. }),
            "{error}"
        );
    }
}
