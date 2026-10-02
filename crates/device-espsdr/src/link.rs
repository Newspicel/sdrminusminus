use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceError, StopHandle};

const POLL: Duration = Duration::from_millis(20);
const MAX_LINE: usize = 512;
const CHUNK: usize = 4096;

pub(crate) trait Link: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), DeviceError>;
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, DeviceError>;
    fn set_baud(&mut self, baud: u32) -> Result<(), DeviceError>;
}

pub(crate) struct SystemLink {
    port: Box<dyn serialport::SerialPort>,
    name: String,
}

impl SystemLink {
    pub(crate) fn open(name: &str, baud: u32) -> Result<Self, DeviceError> {
        let mut port = serialport::new(name, baud)
            .timeout(POLL)
            .flow_control(serialport::FlowControl::None)
            .open()
            .map_err(|e| open_error(name, &e))?;
        port.write_data_terminal_ready(false)
            .and_then(|()| port.write_request_to_send(false))
            .map_err(|e| DeviceError::Io(format!("{name}: {e}")))?;
        Ok(Self {
            port,
            name: name.to_string(),
        })
    }

    fn io(&self, error: &std::io::Error) -> DeviceError {
        match error.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::BrokenPipe => {
                DeviceError::Disconnected(format!("{}: {error}", self.name))
            }
            _ => DeviceError::Io(format!("{}: {error}", self.name)),
        }
    }
}

fn open_error(name: &str, error: &serialport::Error) -> DeviceError {
    let message = format!("{name}: {error}");
    match error.kind() {
        serialport::ErrorKind::NoDevice => DeviceError::NotFound(message),
        serialport::ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => {
            DeviceError::PermissionDenied(message)
        }
        serialport::ErrorKind::Io(std::io::ErrorKind::ResourceBusy) => DeviceError::InUse(message),
        _ => DeviceError::Io(message),
    }
}

impl Link for SystemLink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.port
            .write_all(bytes)
            .and_then(|()| self.port.flush())
            .map_err(|e| self.io(&e))
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, DeviceError> {
        match self.port.read(buffer) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(0),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Ok(0),
            Err(e) => Err(self.io(&e)),
        }
    }

    fn set_baud(&mut self, baud: u32) -> Result<(), DeviceError> {
        self.port
            .set_baud_rate(baud)
            .map_err(|e| DeviceError::Unsupported(format!("{}: {baud} Bd: {e}", self.name)))
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Stop(Arc<AtomicBool>);

impl Stop {
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn clear(&self) {
        self.0.store(false, Ordering::Release);
    }
}

impl StopHandle for Stop {
    fn stop(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Wait {
    TimedOut,
    Stopped,
    Failed(DeviceError),
}

impl From<DeviceError> for Wait {
    fn from(error: DeviceError) -> Self {
        Self::Failed(error)
    }
}

pub(crate) struct Port {
    link: Box<dyn Link>,
    pending: Vec<u8>,
    chunk: Box<[u8; CHUNK]>,
    stop: Stop,
}

impl Port {
    pub(crate) fn new(link: Box<dyn Link>, stop: Stop) -> Self {
        Self {
            link,
            pending: Vec::with_capacity(CHUNK),
            chunk: Box::new([0; CHUNK]),
            stop,
        }
    }

    pub(crate) fn stop(&self) -> &Stop {
        &self.stop
    }

    pub(crate) fn send(&mut self, line: &str) -> Result<(), DeviceError> {
        let mut bytes = [0u8; MAX_LINE];
        let text = line.as_bytes();
        let len = text.len().min(MAX_LINE - 1);
        bytes[..len].copy_from_slice(&text[..len]);
        bytes[len] = b'\n';
        self.link.write(&bytes[..=len])
    }

    pub(crate) fn set_baud(&mut self, baud: u32) -> Result<(), DeviceError> {
        self.pending.clear();
        self.link.set_baud(baud)
    }

    fn fill(&mut self, deadline: Instant) -> Result<(), Wait> {
        if self.stop.is_set() {
            return Err(Wait::Stopped);
        }
        if Instant::now() >= deadline {
            return Err(Wait::TimedOut);
        }
        let n = self.link.read(&mut self.chunk[..])?;
        self.pending.extend_from_slice(&self.chunk[..n]);
        Ok(())
    }

    pub(crate) fn line(&mut self, deadline: Instant) -> Result<String, Wait> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = self.pending.drain(..=end).collect();
                let text = String::from_utf8_lossy(&raw[..end]);
                return Ok(text.trim_end_matches('\r').to_string());
            }
            if self.pending.len() > MAX_LINE {
                self.pending.clear();
            }
            self.fill(deadline)?;
        }
    }

    pub(crate) fn exact(&mut self, out: &mut [u8], deadline: Instant) -> Result<(), Wait> {
        let mut filled = self.pending.len().min(out.len());
        out[..filled].copy_from_slice(&self.pending[..filled]);
        self.pending.drain(..filled);
        while filled < out.len() {
            if self.stop.is_set() {
                return Err(Wait::Stopped);
            }
            if Instant::now() >= deadline {
                return Err(Wait::TimedOut);
            }
            filled += self.link.read(&mut out[filled..])?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    use sdrmm_device::{DeviceError, lock};

    use super::Link;

    pub(crate) type Responder = Box<dyn FnMut(&str, u32) -> Vec<u8> + Send>;

    #[derive(Default)]
    pub(crate) struct Wire {
        pub(crate) sent: Vec<String>,
        pub(crate) baud: u32,
        pub(crate) unplugged: bool,
        partial: Vec<u8>,
        incoming: VecDeque<u8>,
    }

    #[derive(Clone)]
    pub(crate) struct FakeLink {
        pub(crate) wire: Arc<Mutex<Wire>>,
        responder: Arc<Mutex<Responder>>,
    }

    impl FakeLink {
        pub(crate) fn new(responder: impl FnMut(&str, u32) -> Vec<u8> + Send + 'static) -> Self {
            Self {
                wire: Arc::new(Mutex::new(Wire::default())),
                responder: Arc::new(Mutex::new(Box::new(responder))),
            }
        }

        pub(crate) fn sent(&self) -> Vec<String> {
            lock(&self.wire).sent.clone()
        }

        pub(crate) fn inject(&self, bytes: &[u8]) {
            lock(&self.wire).incoming.extend(bytes);
        }
    }

    impl Link for FakeLink {
        fn write(&mut self, bytes: &[u8]) -> Result<(), DeviceError> {
            let mut wire = lock(&self.wire);
            if wire.unplugged {
                return Err(DeviceError::Disconnected("unplugged".into()));
            }
            wire.partial.extend_from_slice(bytes);
            while let Some(end) = wire.partial.iter().position(|b| *b == b'\n') {
                let raw: Vec<u8> = wire.partial.drain(..=end).collect();
                let line = String::from_utf8_lossy(&raw[..end]).to_string();
                if line.is_empty() {
                    continue;
                }
                let baud = wire.baud;
                let reply = (lock(&self.responder))(&line, baud);
                wire.sent.push(line);
                wire.incoming.extend(reply);
            }
            Ok(())
        }

        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, DeviceError> {
            let mut wire = lock(&self.wire);
            if wire.unplugged {
                return Err(DeviceError::Disconnected("unplugged".into()));
            }
            let n = buffer.len().min(wire.incoming.len());
            for slot in &mut buffer[..n] {
                *slot = wire.incoming.pop_front().unwrap_or_default();
            }
            drop(wire);
            if n == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(n)
        }

        fn set_baud(&mut self, baud: u32) -> Result<(), DeviceError> {
            lock(&self.wire).baud = baud;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{fake::FakeLink, *};

    fn port(link: &FakeLink) -> Port {
        Port::new(Box::new(link.clone()), Stop::default())
    }

    fn soon() -> Instant {
        Instant::now() + Duration::from_millis(200)
    }

    #[test]
    fn lines_split_on_newline_and_drop_carriage_returns() {
        let link = FakeLink::new(|_, _| Vec::new());
        link.inject(b"OK\r\nSYNC 7\n");
        let mut port = port(&link);
        assert_eq!(port.line(soon()).expect("line"), "OK");
        assert_eq!(port.line(soon()).expect("line"), "SYNC 7");
        assert!(matches!(port.line(soon()), Err(Wait::TimedOut)));
    }

    #[test]
    fn exact_reads_take_buffered_bytes_first() {
        let link = FakeLink::new(|_, _| Vec::new());
        link.inject(b"DATA 2 0 1\n\x01\x02\x03\x04rest");
        let mut port = port(&link);
        assert_eq!(port.line(soon()).expect("line"), "DATA 2 0 1");
        let mut out = [0u8; 4];
        port.exact(&mut out, soon()).expect("payload");
        assert_eq!(out, [1, 2, 3, 4]);
    }

    #[test]
    fn overlong_garbage_never_grows_without_bound() {
        let link = FakeLink::new(|_, _| Vec::new());
        link.inject(&[0x80; 3 * MAX_LINE]);
        link.inject(b"\nINFO\n");
        let mut port = port(&link);
        let mut lines = Vec::new();
        while let Ok(line) = port.line(soon()) {
            lines.push(line);
        }
        assert_eq!(lines.last().map(String::as_str), Some("INFO"));
    }

    #[test]
    fn a_stop_interrupts_a_wait() {
        let link = FakeLink::new(|_, _| Vec::new());
        let stop = Stop::default();
        let mut port = Port::new(Box::new(link), stop.clone());
        stop.stop();
        assert!(matches!(
            port.line(Instant::now() + Duration::from_secs(5)),
            Err(Wait::Stopped)
        ));
    }
}
