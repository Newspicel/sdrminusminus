use std::{
    collections::VecDeque,
    net::UdpSocket,
    sync::Mutex,
    time::{Duration, Instant},
};

use sdrmm_device::{DeviceError, lock};

use crate::{
    chdr::{self, Header, Kind},
    link::{self, MTU, Received},
    regs::{Half, READBACK, Readback},
};

const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const WINDOW: usize = 3;
const PAYLOAD_BYTES: usize = 2 * chdr::WORD;
const RECEIVE_BUFFER: usize = 1 << 18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Local,
    Radio(usize),
}

impl Target {
    const ALL: [Self; 3] = [Self::Local, Self::Radio(0), Self::Radio(1)];

    pub(crate) const fn sid(self) -> u32 {
        match self {
            Self::Local => 0x40,
            Self::Radio(0) => 0x10,
            Self::Radio(_) => 0x20,
        }
    }

    const fn slot(self) -> usize {
        match self {
            Self::Local => 0,
            Self::Radio(0) => 1,
            Self::Radio(_) => 2,
        }
    }

    fn answering(sid: u32) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|target| chdr::flip(target.sid()) == sid)
    }
}

#[derive(Debug)]
pub(crate) struct Control {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    socket: UdpSocket,
    next: [u16; 3],
    waiting: [VecDeque<u16>; 3],
    buf: Vec<u8>,
}

impl Control {
    pub(crate) fn connect(host: &str, port: u16) -> Result<Self, DeviceError> {
        let socket = link::connect(host, port, RECEIVE_BUFFER)?;
        link::flush(&socket);
        Ok(Self::over(socket))
    }

    pub(crate) fn over(socket: UdpSocket) -> Self {
        Self {
            inner: Mutex::new(Inner {
                socket,
                next: [0; 3],
                waiting: Default::default(),
                buf: vec![0; MTU],
            }),
        }
    }

    pub(crate) fn poke(
        &self,
        target: Target,
        register: u32,
        value: u32,
    ) -> Result<(), DeviceError> {
        let mut inner = lock(&self.inner);
        inner.send(target, register, value)?;
        while inner.waiting[target.slot()].len() >= WINDOW {
            inner.acknowledge_oldest(target)?;
        }
        Ok(())
    }

    pub(crate) fn peek64(&self, target: Target, word: u32) -> Result<u64, DeviceError> {
        let mut inner = lock(&self.inner);
        inner.send(target, READBACK, word)?;
        let mut last = 0;
        while !inner.waiting[target.slot()].is_empty() {
            last = inner.acknowledge_oldest(target)?;
        }
        Ok(last)
    }

    pub(crate) fn peek32(&self, target: Target, readback: Readback) -> Result<u32, DeviceError> {
        let value = self.peek64(target, readback.word)?;
        Ok(match readback.half {
            Half::Low => value as u32,
            Half::High => (value >> 32) as u32,
        })
    }

    pub(crate) fn settle(&self) -> Result<(), DeviceError> {
        let mut inner = lock(&self.inner);
        for target in Target::ALL {
            while !inner.waiting[target.slot()].is_empty() {
                inner.acknowledge_oldest(target)?;
            }
        }
        Ok(())
    }
}

impl Inner {
    fn send(&mut self, target: Target, register: u32, value: u32) -> Result<(), DeviceError> {
        let slot = target.slot();
        let seq = self.next[slot];
        let mut packet = [0u8; 16];
        let header = Header::context(target.sid(), seq).write(PAYLOAD_BYTES, &mut packet)?;
        chdr::put(&mut packet[header..], 0, register);
        chdr::put(&mut packet[header..], 1, value);
        self.socket
            .send(&packet[..header + PAYLOAD_BYTES])
            .map_err(link::io)?;
        self.next[slot] = chdr::next_seq(seq);
        self.waiting[slot].push_back(seq);
        Ok(())
    }

    fn acknowledge_oldest(&mut self, target: Target) -> Result<u64, DeviceError> {
        let deadline = Instant::now() + ACK_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.waiting = Default::default();
                return Err(DeviceError::Io(
                    "the radio stopped answering control requests".to_string(),
                ));
            }
            let n = match link::receive(&self.socket, &mut self.buf, left)? {
                Received::Got(n) => n,
                Received::Quiet => continue,
            };
            if let Some((from, value)) = self.answer(n)?
                && from == target
            {
                return Ok(value);
            }
        }
    }

    fn answer(&mut self, n: usize) -> Result<Option<(Target, u64)>, DeviceError> {
        let packet = chdr::read(&self.buf[..n])?;
        let Some(from) = Target::answering(packet.header.sid) else {
            tracing::trace!(sid = packet.header.sid, "unrouted control packet");
            return Ok(None);
        };
        if packet.header.kind != Kind::Context || packet.payload.len() < PAYLOAD_BYTES {
            return Err(DeviceError::Io(format!(
                "control answer of {} bytes is not a response",
                packet.payload.len()
            )));
        }
        let Some(expected) = self.waiting[from.slot()].pop_front() else {
            tracing::debug!(seq = packet.header.seq, "unexpected control answer");
            return Ok(None);
        };
        if expected != packet.header.seq {
            self.waiting = Default::default();
            return Err(DeviceError::Io(format!(
                "control answer {} arrived where {expected} was due",
                packet.header.seq
            )));
        }
        let hi = u64::from(chdr::word(packet.payload, 0));
        let lo = u64::from(chdr::word(packet.payload, 1));
        Ok(Some((from, hi << 32 | lo)))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::{
        collections::HashMap,
        net::UdpSocket,
        sync::{Arc, Mutex},
        thread::JoinHandle,
    };

    use super::*;

    pub(crate) type Handler = dyn FnMut(Target, u32, u32) -> u64 + Send;

    pub(crate) struct Responder {
        pub(crate) socket: UdpSocket,
        pub(crate) writes: Arc<Mutex<Vec<(Target, u32, u32)>>>,
        _thread: JoinHandle<()>,
    }

    pub(crate) fn responder(mut handler: Box<Handler>) -> Responder {
        let device = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let host = UdpSocket::bind("127.0.0.1:0").expect("bind");
        host.connect(device.local_addr().expect("addr"))
            .expect("connect");
        let writes = Arc::new(Mutex::new(Vec::new()));
        let log = writes.clone();
        let thread = std::thread::spawn(move || {
            let mut buf = [0u8; MTU];
            let mut seen: HashMap<u32, u16> = HashMap::new();
            while let Ok((n, from)) = device.recv_from(&mut buf) {
                let Ok(packet) = chdr::read(&buf[..n]) else {
                    continue;
                };
                let Some(target) = Target::ALL
                    .into_iter()
                    .find(|t| t.sid() == packet.header.sid)
                else {
                    continue;
                };
                seen.insert(packet.header.sid, packet.header.seq);
                let register = chdr::word(packet.payload, 0);
                let value = chdr::word(packet.payload, 1);
                lock(&log).push((target, register, value));
                let answer = handler(target, register, value);
                let mut out = [0u8; 16];
                let header = Header::context(chdr::flip(target.sid()), packet.header.seq);
                let len = header.write(8, &mut out).expect("fits");
                chdr::put(&mut out[len..], 0, (answer >> 32) as u32);
                chdr::put(&mut out[len..], 1, answer as u32);
                if device.send_to(&out[..len + 8], from).is_err() {
                    break;
                }
            }
        });
        Responder {
            socket: host,
            writes,
            _thread: thread,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::responder, *};
    use crate::regs::STATUS_READBACK;

    #[test]
    fn a_readback_returns_the_word_the_radio_answered_with() {
        let echo = responder(Box::new(|_, register, value| {
            if register == READBACK && value == 2 {
                0x0000_0201_dead_beef
            } else {
                0
            }
        }));
        let control = Control::over(echo.socket);
        assert_eq!(
            control.peek64(Target::Local, 2).expect("answer"),
            0x0201_dead_beef
        );
        assert_eq!(
            control
                .peek32(Target::Local, STATUS_READBACK)
                .expect("answer"),
            0x0201
        );
    }

    #[test]
    fn writes_are_acknowledged_in_order_for_every_endpoint() {
        let echo = responder(Box::new(|_, _, _| 0));
        let writes = echo.writes.clone();
        let control = Control::over(echo.socket);
        for value in 0..10 {
            control
                .poke(Target::Radio(value % 2), 8, value as u32)
                .expect("ack");
        }
        control.settle().expect("settled");
        let seen = lock(&writes).clone();
        assert_eq!(seen.len(), 10);
        assert_eq!(seen[3], (Target::Radio(1), 8, 3));
    }

    #[test]
    fn a_silent_radio_is_an_error_not_a_hang() {
        let silent = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let host = UdpSocket::bind("127.0.0.1:0").expect("bind");
        host.connect(silent.local_addr().expect("addr"))
            .expect("connect");
        let control = Control::over(host);
        assert!(control.peek64(Target::Local, 0).is_err());
    }
}
