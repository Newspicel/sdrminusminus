use std::time::Duration;

use sdrmm_wire::cps::RadioIdent;

use crate::{CpsError, RadioSession, SerialLink};

pub const BLOCK: usize = 16;
const TIMEOUT: Duration = Duration::from_millis(2000);
const ACK: u8 = 0x06;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Open,
    Program,
    Closed,
}

pub struct AnytoneSession {
    link: Box<dyn SerialLink>,
    state: State,
    model_id: String,
    accepted: &'static [&'static str],
}

impl AnytoneSession {
    pub fn open(
        link: Box<dyn SerialLink>,
        model_id: impl Into<String>,
        accepted: &'static [&'static str],
    ) -> Result<Self, CpsError> {
        let mut session = Self {
            link,
            state: State::Open,
            model_id: model_id.into(),
            accepted,
        };
        session.link.discard_input()?;
        session.enter_program_mode()?;
        Ok(session)
    }

    fn enter_program_mode(&mut self) -> Result<(), CpsError> {
        if self.state == State::Program {
            return Ok(());
        }
        let mut ack = [0u8; 3];
        self.exchange(b"PROGRAM", &mut ack)?;
        if &ack != b"QX\x06" {
            return Err(CpsError::Protocol {
                step: "enter program mode",
                reason: format!(
                    "expected 515806, got {:02x}{:02x}{:02x}",
                    ack[0], ack[1], ack[2]
                ),
            });
        }
        self.state = State::Program;
        Ok(())
    }

    fn leave_program_mode(&mut self) -> Result<(), CpsError> {
        if self.state != State::Program {
            return Ok(());
        }
        let mut ack = [0u8; 1];
        self.exchange(b"END", &mut ack)?;
        self.state = State::Open;
        Ok(())
    }

    fn exchange(&mut self, request: &[u8], response: &mut [u8]) -> Result<(), CpsError> {
        self.link.send(request)?;
        self.link.receive(response, TIMEOUT)
    }
}

impl RadioSession for AnytoneSession {
    fn identify(&mut self) -> Result<RadioIdent, CpsError> {
        self.enter_program_mode()?;
        let mut response = [0u8; 16];
        self.exchange(&[0x02], &mut response)?;
        if response[0] != b'I' || response[15] != ACK {
            return Err(CpsError::Protocol {
                step: "identify",
                reason: "the radio did not answer with an identification frame".to_owned(),
            });
        }
        let reported_model = trim_ascii(&response[1..8]);
        let bands = response[8];
        let firmware = trim_ascii(&response[9..15]);
        if !self.accepted.is_empty()
            && !self
                .accepted
                .iter()
                .any(|accepted| accepted.eq_ignore_ascii_case(&reported_model))
        {
            return Err(CpsError::ModelMismatch {
                model: self.model_id.clone(),
                reported: reported_model,
            });
        }
        Ok(RadioIdent {
            reported_model,
            firmware: (!firmware.is_empty()).then_some(firmware),
            bands: Some(format!("{bands:#04x}")),
            model_id: Some(self.model_id.clone()),
        })
    }

    fn block_size(&self) -> u32 {
        BLOCK as u32
    }

    fn read(&mut self, addr: u32, buffer: &mut [u8]) -> Result<(), CpsError> {
        self.enter_program_mode()?;
        for (index, frame) in buffer.chunks_mut(BLOCK).enumerate() {
            let at = addr + (index * BLOCK) as u32;
            let mut request = [0u8; 6];
            request[0] = b'R';
            request[1..5].copy_from_slice(&at.to_be_bytes());
            request[5] = BLOCK as u8;
            let mut response = [0u8; 24];
            self.exchange(&request, &mut response)?;
            check_read(&response, at)?;
            let take = frame.len().min(BLOCK);
            frame[..take].copy_from_slice(&response[6..6 + take]);
        }
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), CpsError> {
        self.enter_program_mode()?;
        for (index, frame) in data.chunks(BLOCK).enumerate() {
            let at = addr + (index * BLOCK) as u32;
            let mut request = [0u8; 24];
            request[0] = b'W';
            request[1..5].copy_from_slice(&at.to_be_bytes());
            request[5] = BLOCK as u8;
            request[6..6 + frame.len()].copy_from_slice(frame);
            request[22] = checksum(&request[1..22]);
            request[23] = ACK;
            let mut ack = [0u8; 1];
            self.exchange(&request, &mut ack)?;
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
        if self.state == State::Closed {
            return Ok(());
        }
        let result = self.leave_program_mode();
        self.state = State::Closed;
        result
    }
}

impl Drop for AnytoneSession {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte))
}

fn check_read(response: &[u8; 24], addr: u32) -> Result<(), CpsError> {
    if response[0] != b'W' {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("expected command 'W', got {:#04x}", response[0]),
        });
    }
    let echoed = u32::from_be_bytes([response[1], response[2], response[3], response[4]]);
    if echoed != addr {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("expected address {addr:#010x}, got {echoed:#010x}"),
        });
    }
    if usize::from(response[5]) != BLOCK {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("expected a {BLOCK} byte block, got {}", response[5]),
        });
    }
    let sum = checksum(&response[1..22]);
    if sum != response[22] {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("checksum {sum:#04x} does not match {:#04x}", response[22]),
        });
    }
    if response[23] != ACK {
        return Err(CpsError::Protocol {
            step: "read",
            reason: format!("expected ACK, got {:#04x}", response[23]),
        });
    }
    Ok(())
}

fn trim_ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| char::from(*byte))
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serial::fixture::ScriptedLink;

    fn read_response(addr: u32, fill: u8) -> Vec<u8> {
        let mut frame = vec![0u8; 24];
        frame[0] = b'W';
        frame[1..5].copy_from_slice(&addr.to_be_bytes());
        frame[5] = BLOCK as u8;
        frame[6..22].fill(fill);
        frame[22] = checksum(&frame[1..22]);
        frame[23] = ACK;
        frame
    }

    fn ident_frame(model: &str, firmware: &str) -> Vec<u8> {
        let mut frame = vec![0u8; 16];
        frame[0] = b'I';
        frame[1..1 + model.len()].copy_from_slice(model.as_bytes());
        frame[8] = 0x03;
        frame[9..9 + firmware.len()].copy_from_slice(firmware.as_bytes());
        frame[15] = ACK;
        frame
    }

    #[test]
    fn opening_enters_program_mode_and_identifies_the_radio() {
        let link = ScriptedLink::new(vec![b"QX\x06".to_vec(), ident_frame("D890UV", "V105")]);
        let mut session =
            AnytoneSession::open(Box::new(link), "anytone-d890uv", &["D890UV", "890UV"])
                .expect("program mode");
        let ident = session.identify().expect("identify");
        assert_eq!(ident.reported_model, "D890UV");
        assert_eq!(ident.firmware.as_deref(), Some("V105"));
        assert_eq!(ident.model_id.as_deref(), Some("anytone-d890uv"));
    }

    #[test]
    fn a_foreign_radio_is_refused_instead_of_being_read() {
        let link = ScriptedLink::new(vec![b"QX\x06".to_vec(), ident_frame("D878UV", "V100")]);
        let mut session = AnytoneSession::open(Box::new(link), "anytone-d890uv", &["D890UV"])
            .expect("program mode");
        let error = session.identify().expect_err("wrong radio");
        assert!(matches!(error, CpsError::ModelMismatch { .. }), "{error}");
    }

    #[test]
    fn reads_are_verified_against_the_echoed_address_and_checksum() {
        let mut link = ScriptedLink::new(vec![b"QX\x06".to_vec()]);
        link.push_response(read_response(0x1000_0000, 0xa5));
        link.push_response(read_response(0x1000_0010, 0x5a));
        let mut session =
            AnytoneSession::open(Box::new(link), "anytone-d890uv", &[]).expect("open");
        let mut buffer = [0u8; 32];
        session.read(0x1000_0000, &mut buffer).expect("read");
        assert_eq!(&buffer[..16], [0xa5; 16]);
        assert_eq!(&buffer[16..], [0x5a; 16]);
    }

    #[test]
    fn a_mismatched_read_echo_surfaces_rather_than_being_stored() {
        let mut link = ScriptedLink::new(vec![b"QX\x06".to_vec()]);
        link.push_response(read_response(0x2000, 0xa5));
        let mut session =
            AnytoneSession::open(Box::new(link), "anytone-d890uv", &[]).expect("open");
        let mut buffer = [0u8; 16];
        let error = session
            .read(0x1000, &mut buffer)
            .expect_err("wrong address");
        assert!(
            matches!(error, CpsError::Protocol { step: "read", .. }),
            "{error}"
        );
    }

    #[test]
    fn a_write_frame_carries_the_documented_header_and_checksum() {
        let mut link = ScriptedLink::new(vec![b"QX\x06".to_vec()]);
        link.push_response(vec![ACK]);
        link.push_response(vec![ACK]);
        let mut session =
            AnytoneSession::open(Box::new(link), "anytone-d890uv", &[]).expect("open");
        session.write(0x0102_0304, &[0x11; 16]).expect("write");
        session.finish().expect("finish");
    }
}
