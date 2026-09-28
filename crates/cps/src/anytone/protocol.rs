use std::time::Duration;

use sdrmm_wire::cps::RadioIdent;

use crate::{CpsError, RadioSession, SerialLink};

pub const BLOCK: usize = 16;
const REPLY_WAIT: Duration = Duration::from_secs(2);
const ACK: u8 = 0x06;
const HELLO: &[u8] = b"PROGRAM";
const HELLO_REPLY: &[u8; 3] = b"QX\x06";
const QUERY_ID: u8 = 0x02;
const BYE: &[u8] = b"END";
const READ: u8 = b'R';
const DATA: u8 = b'W';
const DATA_FRAME: usize = BLOCK + 8;

pub struct AnytoneSession {
    link: Box<dyn SerialLink>,
    model_id: String,
    accepted: &'static [&'static str],
    programming: bool,
    finished: bool,
}

impl AnytoneSession {
    pub fn open(
        link: Box<dyn SerialLink>,
        model_id: impl Into<String>,
        accepted: &'static [&'static str],
    ) -> Result<Self, CpsError> {
        let mut session = Self {
            link,
            model_id: model_id.into(),
            accepted,
            programming: false,
            finished: false,
        };
        session.link.discard_input()?;
        session.ensure_programming()?;
        Ok(session)
    }

    fn ensure_programming(&mut self) -> Result<(), CpsError> {
        if self.programming {
            return Ok(());
        }
        let mut reply = [0u8; 3];
        self.transact(HELLO, &mut reply)?;
        if &reply != HELLO_REPLY {
            return Err(CpsError::Protocol {
                step: "enter program mode",
                reason: format!("radio replied {}", hex(&reply)),
            });
        }
        self.programming = true;
        Ok(())
    }

    fn accepts(&self, model: &str) -> bool {
        self.accepted.is_empty()
            || self
                .accepted
                .iter()
                .any(|name| name.eq_ignore_ascii_case(model))
    }

    fn transact(&mut self, request: &[u8], reply: &mut [u8]) -> Result<(), CpsError> {
        self.link.send(request)?;
        self.link.receive(reply, REPLY_WAIT)
    }

    fn fetch_block(&mut self, addr: u32) -> Result<[u8; DATA_FRAME], CpsError> {
        let mut request = [READ, 0, 0, 0, 0, BLOCK as u8];
        request[1..5].copy_from_slice(&addr.to_be_bytes());
        let mut reply = [0u8; DATA_FRAME];
        self.transact(&request, &mut reply)?;
        verify_data_frame(&reply, addr)?;
        Ok(reply)
    }

    fn store_block(&mut self, addr: u32, block: &[u8]) -> Result<(), CpsError> {
        let frame = data_frame(addr, block);
        let mut reply = [0u8; 1];
        self.transact(&frame, &mut reply)?;
        if reply[0] == ACK {
            return Ok(());
        }
        Err(CpsError::Protocol {
            step: "write",
            reason: format!("radio replied {:#04x} at {addr:#010x}", reply[0]),
        })
    }
}

impl RadioSession for AnytoneSession {
    fn identify(&mut self) -> Result<RadioIdent, CpsError> {
        self.ensure_programming()?;
        let mut reply = [0u8; 16];
        self.transact(&[QUERY_ID], &mut reply)?;
        let ident = Identity::parse(&reply)?;
        if !self.accepts(&ident.model) {
            return Err(CpsError::ModelMismatch {
                model: self.model_id.clone(),
                reported: ident.model,
            });
        }
        Ok(RadioIdent {
            reported_model: ident.model,
            firmware: ident.firmware,
            bands: Some(format!("{:#04x}", ident.bands)),
            model_id: Some(self.model_id.clone()),
        })
    }

    fn block_size(&self) -> u32 {
        BLOCK as u32
    }

    fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError> {
        self.ensure_programming()?;
        let mut at = addr;
        for target in buffer.chunks_mut(BLOCK) {
            let frame = self.fetch_block(at)?;
            target.copy_from_slice(&frame[6..6 + target.len()]);
            at += BLOCK as u32;
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), CpsError> {
        self.ensure_programming()?;
        let mut at = addr;
        for block in data.chunks(BLOCK) {
            self.store_block(at, block)?;
            at += BLOCK as u32;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CpsError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        if !self.programming {
            return Ok(());
        }
        self.programming = false;
        let mut reply = [0u8; 1];
        self.transact(BYE, &mut reply)
    }
}

impl Drop for AnytoneSession {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

struct Identity {
    model: String,
    bands: u8,
    firmware: Option<String>,
}

impl Identity {
    fn parse(reply: &[u8; 16]) -> Result<Self, CpsError> {
        if reply[0] != b'I' || reply[15] != ACK {
            return Err(CpsError::Protocol {
                step: "identify",
                reason: format!("no identification frame: {}", hex(reply)),
            });
        }
        let firmware = text(&reply[9..15]);
        Ok(Self {
            model: text(&reply[1..8]),
            bands: reply[8],
            firmware: (!firmware.is_empty()).then_some(firmware),
        })
    }
}

fn sum8(bytes: &[u8]) -> u8 {
    bytes.iter().copied().fold(0, u8::wrapping_add)
}

fn data_frame(addr: u32, block: &[u8]) -> [u8; DATA_FRAME] {
    let mut frame = [0u8; DATA_FRAME];
    frame[0] = DATA;
    frame[1..5].copy_from_slice(&addr.to_be_bytes());
    frame[5] = BLOCK as u8;
    frame[6..6 + block.len()].copy_from_slice(block);
    frame[DATA_FRAME - 2] = sum8(&frame[1..DATA_FRAME - 2]);
    frame[DATA_FRAME - 1] = ACK;
    frame
}

fn verify_data_frame(frame: &[u8; DATA_FRAME], addr: u32) -> Result<(), CpsError> {
    let fail = |reason: String| {
        Err(CpsError::Protocol {
            step: "read",
            reason,
        })
    };
    if frame[0] != DATA || frame[DATA_FRAME - 1] != ACK {
        return fail(format!(
            "malformed reply {:#04x}..{:#04x} at {addr:#010x}",
            frame[0],
            frame[DATA_FRAME - 1]
        ));
    }
    let echoed = u32::from_be_bytes([frame[1], frame[2], frame[3], frame[4]]);
    if echoed != addr || usize::from(frame[5]) != BLOCK {
        return fail(format!(
            "asked for {BLOCK} bytes at {addr:#010x}, got {} at {echoed:#010x}",
            frame[5]
        ));
    }
    let expected = sum8(&frame[1..DATA_FRAME - 2]);
    if frame[DATA_FRAME - 2] != expected {
        return fail(format!(
            "bad checksum at {addr:#010x}: {:#04x}, want {expected:#04x}",
            frame[DATA_FRAME - 2]
        ));
    }
    Ok(())
}

fn text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::fixture::ScriptedLink;

    fn block_reply(addr: u32, fill: u8) -> Vec<u8> {
        data_frame(addr, &[fill; BLOCK]).to_vec()
    }

    fn ident_reply(model: &str, firmware: &str) -> Vec<u8> {
        let mut reply = vec![0u8; 16];
        reply[0] = b'I';
        reply[1..1 + model.len()].copy_from_slice(model.as_bytes());
        reply[8] = 0x03;
        reply[9..9 + firmware.len()].copy_from_slice(firmware.as_bytes());
        reply[15] = ACK;
        reply
    }

    fn session(link: ScriptedLink, accepted: &'static [&'static str]) -> AnytoneSession {
        AnytoneSession::open(Box::new(link), "anytone-d890uv", accepted).expect("program mode")
    }

    #[test]
    fn opening_enters_program_mode_and_identifies_the_radio() {
        let link = ScriptedLink::new(vec![HELLO_REPLY.to_vec(), ident_reply("D890UV", "V105")]);
        let ident = session(link, &["D890UV", "890UV"])
            .identify()
            .expect("identify");
        assert_eq!(ident.reported_model, "D890UV");
        assert_eq!(ident.firmware.as_deref(), Some("V105"));
        assert_eq!(ident.bands.as_deref(), Some("0x03"));
        assert_eq!(ident.model_id.as_deref(), Some("anytone-d890uv"));
    }

    #[test]
    fn a_wrong_hello_reply_is_refused() {
        let link = ScriptedLink::new(vec![b"QX\x15".to_vec()]);
        let Err(error) = AnytoneSession::open(Box::new(link), "anytone-d890uv", &[]) else {
            panic!("a wrong hello must not open a session");
        };
        assert!(
            matches!(
                error,
                CpsError::Protocol {
                    step: "enter program mode",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_foreign_radio_is_refused_instead_of_being_read() {
        let link = ScriptedLink::new(vec![HELLO_REPLY.to_vec(), ident_reply("D878UV", "V100")]);
        let error = session(link, &["D890UV"])
            .identify()
            .expect_err("wrong radio");
        assert!(matches!(error, CpsError::ModelMismatch { .. }), "{error}");
    }

    #[test]
    fn reads_are_verified_against_the_echoed_address_and_checksum() {
        let mut link = ScriptedLink::new(vec![HELLO_REPLY.to_vec()]);
        link.push_response(block_reply(0x1000_0000, 0xa5));
        link.push_response(block_reply(0x1000_0010, 0x5a));
        let mut buffer = [0u8; 32];
        session(link, &[])
            .read(0x1000_0000, &mut buffer)
            .expect("read");
        assert_eq!(&buffer[..16], [0xa5; 16]);
        assert_eq!(&buffer[16..], [0x5a; 16]);
    }

    #[test]
    fn a_mismatched_read_echo_surfaces_rather_than_being_stored() {
        let mut link = ScriptedLink::new(vec![HELLO_REPLY.to_vec()]);
        link.push_response(block_reply(0x2000, 0xa5));
        let mut buffer = [0u8; 16];
        let error = session(link, &[])
            .read(0x1000, &mut buffer)
            .expect_err("wrong address");
        assert!(
            matches!(error, CpsError::Protocol { step: "read", .. }),
            "{error}"
        );
    }

    #[test]
    fn a_corrupted_read_checksum_surfaces() {
        let mut link = ScriptedLink::new(vec![HELLO_REPLY.to_vec()]);
        let mut reply = block_reply(0x1000, 0xa5);
        reply[DATA_FRAME - 2] ^= 0xff;
        link.push_response(reply);
        let mut buffer = [0u8; 16];
        let error = session(link, &[])
            .read(0x1000, &mut buffer)
            .expect_err("bad checksum");
        assert!(
            matches!(error, CpsError::Protocol { step: "read", .. }),
            "{error}"
        );
    }

    #[test]
    fn a_write_frame_carries_the_address_length_checksum_and_ack() {
        let frame = data_frame(0x0102_0304, &[0x11; BLOCK]);
        assert_eq!(&frame[..6], &[b'W', 0x01, 0x02, 0x03, 0x04, 0x10]);
        assert_eq!(&frame[6..22], &[0x11; BLOCK]);
        assert_eq!(frame[22], 0x2a);
        assert_eq!(frame[23], ACK);
    }

    #[test]
    fn writing_and_finishing_expect_an_ack_for_every_block() {
        let mut link = ScriptedLink::new(vec![HELLO_REPLY.to_vec()]);
        link.push_response(vec![ACK]);
        link.push_response(vec![ACK]);
        let mut session = session(link, &[]);
        session.write(0x0102_0304, &[0x11; 16]).expect("write");
        session.finish().expect("finish");
    }
}
