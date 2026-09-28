use std::time::Duration;

use sdrmm_wire::cps::RadioIdent;

use super::{
    frame::{ACK, Command, PAGE_BYTES, PAGE_REPLY_BYTES, page_payload},
    rt4d::MODEL_ID,
    segments::{Placement, place},
};
use crate::{CpsError, RadioSession, SerialLink, bits::get_u16_le};

const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
const IDENTITY_PAGE: u32 = 0x2000;
const SIGNATURE_AT: usize = 12;
const SIGNATURE: u16 = 0xabcd;
const REPORTED_MODEL: &str = "RT4D";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Programming,
    Closed,
}

pub struct Rt4DSession {
    link: Box<dyn SerialLink>,
    phase: Phase,
}

impl Rt4DSession {
    pub fn open(mut link: Box<dyn SerialLink>) -> Result<Self, CpsError> {
        link.set_control_lines(true)?;
        link.discard_input()?;
        Ok(Self {
            link,
            phase: Phase::Idle,
        })
    }

    fn ensure_programming(&mut self) -> Result<(), CpsError> {
        match self.phase {
            Phase::Programming => Ok(()),
            Phase::Closed => Err(CpsError::Protocol {
                step: "enter",
                reason: "the session is already closed".to_owned(),
            }),
            Phase::Idle => {
                self.send_acknowledged("enter", Command::Enter)?;
                self.phase = Phase::Programming;
                Ok(())
            }
        }
    }

    fn send_acknowledged(&mut self, step: &'static str, command: Command) -> Result<(), CpsError> {
        self.link.send(&command.to_bytes())?;
        let mut reply = [0u8; 1];
        self.link.receive(&mut reply, REPLY_TIMEOUT)?;
        if reply[0] == ACK {
            return Ok(());
        }
        Err(CpsError::Protocol {
            step,
            reason: format!("answered {:#04x} instead of ACK", reply[0]),
        })
    }

    fn read_page(&mut self, page: u16) -> Result<[u8; PAGE_BYTES], CpsError> {
        self.link.send(&Command::ReadPage { page }.to_bytes())?;
        let mut reply = [0u8; PAGE_REPLY_BYTES];
        self.link.receive(&mut reply, REPLY_TIMEOUT)?;
        let mut data = [0u8; PAGE_BYTES];
        data.copy_from_slice(page_payload(&reply)?);
        Ok(data)
    }

    fn write_page(&mut self, at: Placement, data: &[u8]) -> Result<(), CpsError> {
        self.send_acknowledged(
            "write",
            Command::WritePage {
                segment: at.segment,
                page: at.page,
                data,
            },
        )
    }

    fn leave(&mut self) {
        if self.phase == Phase::Programming && self.link.send(&Command::Leave.to_bytes()).is_ok() {
            let mut reply = [0u8; 1];
            let _ = self.link.receive(&mut reply, REPLY_TIMEOUT);
        }
        self.phase = Phase::Closed;
    }
}

fn page_of(addr: u32) -> Result<u16, CpsError> {
    u16::try_from(addr / PAGE_BYTES as u32).map_err(|_| CpsError::Protocol {
        step: "read",
        reason: format!("{addr:#x} is beyond the addressable memory"),
    })
}

fn placements(addr: u32, len: usize) -> Result<Vec<(Placement, usize)>, CpsError> {
    let mut plan = Vec::with_capacity(len.div_ceil(PAGE_BYTES));
    let mut offset = 0usize;
    while offset < len {
        let take = PAGE_BYTES.min(len - offset);
        let at = u32::try_from(offset)
            .ok()
            .and_then(|offset| addr.checked_add(offset))
            .and_then(|page_addr| place(page_addr, take))
            .ok_or_else(|| CpsError::Protocol {
                step: "write",
                reason: format!("{addr:#x} (+{offset:#x}) is outside every writable segment"),
            })?;
        plan.push((at, offset));
        offset += take;
    }
    Ok(plan)
}

impl RadioSession for Rt4DSession {
    fn identify(&mut self) -> Result<RadioIdent, CpsError> {
        self.ensure_programming()?;
        let page = self.read_page(page_of(IDENTITY_PAGE)?)?;
        let signature = get_u16_le(&page, SIGNATURE_AT);
        if signature != SIGNATURE {
            return Err(CpsError::Protocol {
                step: "identify",
                reason: format!("signature {signature:#06x} is not an RT-4D"),
            });
        }
        Ok(RadioIdent {
            reported_model: REPORTED_MODEL.to_owned(),
            firmware: None,
            bands: None,
            model_id: Some(MODEL_ID.to_owned()),
        })
    }

    fn block_size(&self) -> u32 {
        PAGE_BYTES as u32
    }

    fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError> {
        self.ensure_programming()?;
        let mut filled = 0usize;
        while filled < buffer.len() {
            let cursor = addr.saturating_add(u32::try_from(filled).unwrap_or(u32::MAX));
            let within = cursor as usize % PAGE_BYTES;
            let take = (PAGE_BYTES - within).min(buffer.len() - filled);
            let page = self.read_page(page_of(cursor)?)?;
            buffer[filled..filled + take].copy_from_slice(&page[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), CpsError> {
        let plan = placements(addr, data.len())?;
        self.ensure_programming()?;
        for (at, offset) in plan {
            let end = (offset + PAGE_BYTES).min(data.len());
            self.write_page(at, &data[offset..end])?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CpsError> {
        if self.phase == Phase::Closed {
            return Ok(());
        }
        self.leave();
        self.link.set_control_lines(false)
    }
}

impl Drop for Rt4DSession {
    fn drop(&mut self) {
        if self.phase != Phase::Closed {
            self.leave();
            let _ = self.link.set_control_lines(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        bits::set_u16_le,
        radtel::frame::{PAGE_BYTES, sum8},
        serial::fixture::ScriptedLink,
    };

    const ENTER: [u8; 5] = [0x34, 0x52, 0x05, 0x10, 0x9b];
    const LEAVE: [u8; 5] = [0x34, 0x52, 0x05, 0xee, 0x79];

    struct Tap {
        inner: ScriptedLink,
        sent: Arc<Mutex<Vec<u8>>>,
    }

    impl SerialLink for Tap {
        fn send(&mut self, data: &[u8]) -> Result<(), CpsError> {
            if let Ok(mut sent) = self.sent.lock() {
                sent.extend_from_slice(data);
            }
            self.inner.send(data)
        }

        fn receive(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<(), CpsError> {
            self.inner.receive(buffer, timeout)
        }

        fn discard_input(&mut self) -> Result<(), CpsError> {
            self.inner.discard_input()
        }

        fn set_control_lines(&mut self, asserted: bool) -> Result<(), CpsError> {
            self.inner.set_control_lines(asserted)
        }
    }

    fn session(responses: Vec<Vec<u8>>) -> (Rt4DSession, Arc<Mutex<Vec<u8>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let tap = Tap {
            inner: ScriptedLink::new(responses),
            sent: Arc::clone(&sent),
        };
        let session = Rt4DSession::open(Box::new(tap)).expect("open");
        (session, sent)
    }

    fn sent_bytes(sent: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
        sent.lock().expect("lock").clone()
    }

    fn page_reply(page: u16, data: &[u8; PAGE_BYTES]) -> Vec<u8> {
        let [hi, lo] = page.to_be_bytes();
        let mut reply = vec![0x52, hi, lo];
        reply.extend_from_slice(data);
        reply.push(sum8(&reply));
        reply
    }

    fn signed_page() -> [u8; PAGE_BYTES] {
        let mut data = [0xffu8; PAGE_BYTES];
        set_u16_le(&mut data, SIGNATURE_AT, SIGNATURE);
        data
    }

    #[test]
    fn identify_enters_programming_once_and_checks_the_signature() {
        let (mut radio, sent) = session(vec![
            vec![ACK],
            page_reply(8, &signed_page()),
            page_reply(8, &signed_page()),
        ]);
        let ident = radio.identify().expect("identify");
        assert_eq!(ident.reported_model, "RT4D");
        assert_eq!(ident.model_id.as_deref(), Some(MODEL_ID));
        radio.identify().expect("identify again");
        let mut expected = ENTER.to_vec();
        expected.extend_from_slice(&[0x52, 0x00, 0x08, 0x5a]);
        expected.extend_from_slice(&[0x52, 0x00, 0x08, 0x5a]);
        assert_eq!(sent_bytes(&sent), expected);
    }

    #[test]
    fn a_foreign_signature_is_refused() {
        let (mut radio, _) = session(vec![vec![ACK], page_reply(8, &[0u8; PAGE_BYTES])]);
        assert!(matches!(
            radio.identify(),
            Err(CpsError::Protocol {
                step: "identify",
                ..
            })
        ));
    }

    #[test]
    fn a_refused_enter_surfaces_as_an_error() {
        let (mut radio, _) = session(vec![vec![0x15]]);
        assert!(matches!(
            radio.identify(),
            Err(CpsError::Protocol { step: "enter", .. })
        ));
    }

    #[test]
    fn a_page_read_is_checked_and_sliced() {
        let mut data = [0u8; PAGE_BYTES];
        data[0x10..0x14].copy_from_slice(&[1, 2, 3, 4]);
        let mut corrupt = page_reply(0x10, &data);
        if let Some(last) = corrupt.last_mut() {
            *last = last.wrapping_add(1);
        }
        let (mut radio, _) = session(vec![vec![ACK], page_reply(0x10, &data), corrupt]);
        let mut buffer = [0u8; 4];
        radio.read(0x4010, &mut buffer).expect("read");
        assert_eq!(buffer, [1, 2, 3, 4]);
        assert!(matches!(
            radio.read(0x4010, &mut buffer),
            Err(CpsError::Protocol { step: "read", .. })
        ));
    }

    #[test]
    fn a_write_outside_the_segments_sends_nothing() {
        let (mut radio, sent) = session(Vec::new());
        assert!(matches!(
            radio.write(0x1000, &[0u8; 16]),
            Err(CpsError::Protocol { step: "write", .. })
        ));
        assert!(matches!(
            radio.write(0x1_c000, &[0u8; 2 * PAGE_BYTES]),
            Err(CpsError::Protocol { step: "write", .. })
        ));
        assert!(sent_bytes(&sent).is_empty());
    }

    #[test]
    fn a_write_frame_names_its_segment_and_page() {
        let (mut radio, sent) = session(vec![vec![ACK], vec![ACK]]);
        radio.write(0x4800, &[0x11, 0x22]).expect("write");
        let bytes = sent_bytes(&sent);
        let frame = &bytes[ENTER.len()..];
        assert_eq!(frame.len(), PAGE_BYTES + 5);
        assert_eq!(&frame[..6], &[0x09, 0x01, 0x00, 0x02, 0x11, 0x22]);
        assert!(frame[6..PAGE_BYTES + 4].iter().all(|byte| *byte == 0));
        assert_eq!(frame[PAGE_BYTES + 4], 0x3f);
    }

    #[test]
    fn finish_leaves_programming_once() {
        let (mut radio, sent) = session(vec![vec![ACK], page_reply(8, &signed_page())]);
        radio.identify().expect("identify");
        radio.finish().expect("finish");
        radio.finish().expect("finish twice");
        drop(radio);
        assert!(sent_bytes(&sent).ends_with(&LEAVE));
        assert_eq!(
            sent_bytes(&sent)
                .windows(LEAVE.len())
                .filter(|window| *window == LEAVE)
                .count(),
            1
        );
    }
}
