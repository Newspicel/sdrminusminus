use std::{
    io::{Read, Write},
    time::Duration,
};

use crate::CpsError;

pub trait SerialLink: Send {
    fn send(&mut self, data: &[u8]) -> Result<(), CpsError>;
    fn receive(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<(), CpsError>;
    fn discard_input(&mut self) -> Result<(), CpsError>;
    fn set_control_lines(&mut self, asserted: bool) -> Result<(), CpsError>;
}

pub trait SerialBackend: Send + Sync {
    fn ports(&self) -> Result<Vec<serialport::SerialPortInfo>, String>;
    fn open(&self, port: &str, baud: u32) -> Result<Box<dyn SerialLink>, String>;
}

pub struct SystemSerial;

const POLL: Duration = Duration::from_millis(50);

impl SerialBackend for SystemSerial {
    fn ports(&self) -> Result<Vec<serialport::SerialPortInfo>, String> {
        serialport::available_ports().map_err(|error| error.to_string())
    }

    fn open(&self, port: &str, baud: u32) -> Result<Box<dyn SerialLink>, String> {
        let handle = serialport::new(port, baud)
            .timeout(POLL)
            .flow_control(serialport::FlowControl::None)
            .open()
            .map_err(|error| error.to_string())?;
        Ok(Box::new(SystemLink { handle }))
    }
}

struct SystemLink {
    handle: Box<dyn serialport::SerialPort>,
}

impl SerialLink for SystemLink {
    fn send(&mut self, data: &[u8]) -> Result<(), CpsError> {
        self.handle
            .write_all(data)
            .and_then(|()| self.handle.flush())
            .map_err(|error| CpsError::Transport(error.to_string()))
    }

    fn receive(&mut self, buffer: &mut [u8], timeout: Duration) -> Result<(), CpsError> {
        let deadline = std::time::Instant::now() + timeout;
        let mut filled = 0;
        while filled < buffer.len() {
            if std::time::Instant::now() >= deadline {
                return Err(CpsError::Timeout {
                    wanted: buffer.len(),
                    got: filled,
                });
            }
            match self.handle.read(&mut buffer[filled..]) {
                Ok(0) => {}
                Ok(read) => filled += read,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => return Err(CpsError::Transport(error.to_string())),
            }
        }
        Ok(())
    }

    fn discard_input(&mut self) -> Result<(), CpsError> {
        self.handle
            .clear(serialport::ClearBuffer::Input)
            .map_err(|error| CpsError::Transport(error.to_string()))
    }

    fn set_control_lines(&mut self, asserted: bool) -> Result<(), CpsError> {
        self.handle
            .write_data_terminal_ready(asserted)
            .and_then(|()| self.handle.write_request_to_send(asserted))
            .map_err(|error| CpsError::Transport(error.to_string()))
    }
}

#[cfg(test)]
pub mod fixture {
    use std::{collections::VecDeque, time::Duration};

    use super::SerialLink;
    use crate::CpsError;

    #[derive(Default)]
    pub struct ScriptedLink {
        pub sent: Vec<u8>,
        responses: VecDeque<Vec<u8>>,
        pending: VecDeque<u8>,
    }

    impl ScriptedLink {
        #[must_use]
        pub fn new(responses: Vec<Vec<u8>>) -> Self {
            Self {
                sent: Vec::new(),
                responses: responses.into(),
                pending: VecDeque::new(),
            }
        }

        pub fn push_response(&mut self, response: Vec<u8>) {
            self.responses.push_back(response);
        }
    }

    impl SerialLink for ScriptedLink {
        fn send(&mut self, data: &[u8]) -> Result<(), CpsError> {
            self.sent.extend_from_slice(data);
            if let Some(response) = self.responses.pop_front() {
                self.pending.extend(response);
            }
            Ok(())
        }

        fn receive(&mut self, buffer: &mut [u8], _timeout: Duration) -> Result<(), CpsError> {
            for (index, slot) in buffer.iter_mut().enumerate() {
                let Some(byte) = self.pending.pop_front() else {
                    return Err(CpsError::Timeout {
                        wanted: buffer.len(),
                        got: index,
                    });
                };
                *slot = byte;
            }
            Ok(())
        }

        fn discard_input(&mut self) -> Result<(), CpsError> {
            self.pending.clear();
            Ok(())
        }

        fn set_control_lines(&mut self, _asserted: bool) -> Result<(), CpsError> {
            Ok(())
        }
    }
}
