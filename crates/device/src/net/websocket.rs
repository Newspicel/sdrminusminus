use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    DeviceError, StreamFailure, lock,
    net::{CONNECT_TIMEOUT, Connection, Endpoint, Read, SocketStop},
    pool::{Block, BlockPool},
};

pub(crate) mod frame;
pub(crate) mod handshake;

use frame::{Head, Opcode};

/// What a poll of the socket produced: one whole message, nothing yet, or the end of the stream.
#[derive(Debug)]
pub enum Incoming {
    Text(String),
    Binary(Block),
    Idle,
    Ended,
}

#[derive(Debug)]
enum Phase {
    Head {
        bytes: [u8; 2],
        got: usize,
    },
    Extra {
        head: Head,
        bytes: [u8; 12],
        got: usize,
        need: usize,
    },
    Payload {
        head: Head,
        mask: Option<[u8; 4]>,
        block: Block,
        got: usize,
    },
}

impl Phase {
    const fn head() -> Self {
        Self::Head {
            bytes: [0u8; 2],
            got: 0,
        }
    }
}

#[derive(Debug)]
struct Reader {
    phase: Phase,
    /// Bytes the upgrade response was read past; the first frames arrive in the same packet.
    pending: Vec<u8>,
    partial: Option<(Opcode, Vec<u8>)>,
}

/// An RFC 6455 client over one TCP connection, framed a step at a time so a capture thread polls
/// it the way it polls any other transport.
#[derive(Debug)]
pub struct WebSocket {
    connection: Arc<Connection>,
    pool: BlockPool,
    reader: Mutex<Reader>,
    sending: Mutex<()>,
}

impl WebSocket {
    /// Dials `endpoint` and completes the upgrade to `path`, or reports why it is not a WebSocket.
    pub fn connect(endpoint: &Endpoint, path: &str) -> Result<Self, DeviceError> {
        let connection = Connection::new(endpoint.connect()?);
        let key = handshake::nonce();
        connection.send(&handshake::request(&endpoint.to_string(), path, &key))?;
        let pending = Self::upgrade(&connection, &key)?;
        Ok(Self {
            connection: Arc::new(connection),
            pool: BlockPool::default(),
            reader: Mutex::new(Reader {
                phase: Phase::head(),
                pending,
                partial: None,
            }),
            sending: Mutex::new(()),
        })
    }

    fn upgrade(connection: &Connection, key: &str) -> Result<Vec<u8>, DeviceError> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let mut response = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            if let Some(at) = find(&response, handshake::TERMINATOR) {
                let body = response.split_off(at + handshake::TERMINATOR.len());
                handshake::verify(&response, key)?;
                return Ok(body);
            }
            if response.len() > handshake::MAX_RESPONSE {
                return Err(DeviceError::Io(
                    "the server's upgrade response never ended".to_string(),
                ));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(DeviceError::Io(
                    "timed out waiting for the WebSocket upgrade".to_string(),
                ));
            }
            match connection.read(&mut buf, left) {
                Read::Got(n) => response.extend_from_slice(&buf[..n]),
                Read::Idle => {}
                Read::Ended => {
                    return Err(DeviceError::Io(format!(
                        "the WebSocket upgrade: {}",
                        connection.failure().reason
                    )));
                }
            }
        }
    }

    pub fn send_text(&self, text: &str) -> Result<(), DeviceError> {
        self.send(Opcode::Text, text.as_bytes())
    }

    fn send(&self, opcode: Opcode, payload: &[u8]) -> Result<(), DeviceError> {
        let _sending = lock(&self.sending);
        self.connection.send(&frame::encode(opcode, payload))
    }

    #[must_use]
    pub fn stop_handle(&self) -> SocketStop {
        self.connection.stop_handle()
    }

    #[must_use]
    pub fn failure(&self) -> StreamFailure {
        self.connection.failure()
    }

    pub fn fail(&self, reason: String) {
        self.connection.fail(reason);
    }

    /// Says goodbye the way the protocol asks before dropping the socket underneath it.
    pub fn close(&self) {
        let _ = self.send(Opcode::Close, &1000u16.to_be_bytes());
        self.connection.close();
    }

    /// Advances the framer by one read, yielding a message only once it is whole.
    pub fn next(&self, timeout: Duration) -> Incoming {
        let mut reader = lock(&self.reader);
        self.advance(&mut reader, timeout)
    }

    fn advance(&self, reader: &mut Reader, timeout: Duration) -> Incoming {
        match std::mem::replace(&mut reader.phase, Phase::head()) {
            Phase::Head { mut bytes, mut got } => {
                match self.read(&mut reader.pending, &mut bytes[got..], timeout) {
                    Read::Got(n) => got += n,
                    Read::Idle => {
                        reader.phase = Phase::Head { bytes, got };
                        return Incoming::Idle;
                    }
                    Read::Ended => return Incoming::Ended,
                }
                if got < bytes.len() {
                    reader.phase = Phase::Head { bytes, got };
                    return Incoming::Idle;
                }
                let head = match Head::parse(bytes) {
                    Ok(head) => head,
                    Err(e) => return self.broken(e),
                };
                reader.phase = Phase::Extra {
                    head,
                    bytes: [0u8; 12],
                    got: 0,
                    need: head.extra(),
                };
                Incoming::Idle
            }
            Phase::Extra {
                head,
                mut bytes,
                mut got,
                need,
            } => {
                if got < need {
                    match self.read(&mut reader.pending, &mut bytes[got..need], timeout) {
                        Read::Got(n) => got += n,
                        Read::Idle => {
                            reader.phase = Phase::Extra {
                                head,
                                bytes,
                                got,
                                need,
                            };
                            return Incoming::Idle;
                        }
                        Read::Ended => return Incoming::Ended,
                    }
                    if got < need {
                        reader.phase = Phase::Extra {
                            head,
                            bytes,
                            got,
                            need,
                        };
                        return Incoming::Idle;
                    }
                }
                let (len, mask) = match head.payload(&bytes[..need]) {
                    Ok(payload) => payload,
                    Err(e) => return self.broken(e),
                };
                let block = self.pool.take(len);
                if len == 0 {
                    return self.whole(reader, head, mask, block);
                }
                reader.phase = Phase::Payload {
                    head,
                    mask,
                    block,
                    got: 0,
                };
                Incoming::Idle
            }
            Phase::Payload {
                head,
                mask,
                mut block,
                mut got,
            } => {
                match self.read(&mut reader.pending, &mut block.bytes_mut()[got..], timeout) {
                    Read::Got(n) => got += n,
                    Read::Idle => {
                        reader.phase = Phase::Payload {
                            head,
                            mask,
                            block,
                            got,
                        };
                        return Incoming::Idle;
                    }
                    Read::Ended => return Incoming::Ended,
                }
                if got < block.len() {
                    reader.phase = Phase::Payload {
                        head,
                        mask,
                        block,
                        got,
                    };
                    return Incoming::Idle;
                }
                self.whole(reader, head, mask, block)
            }
        }
    }

    fn read(&self, pending: &mut Vec<u8>, buf: &mut [u8], timeout: Duration) -> Read {
        if pending.is_empty() {
            return self.connection.read(buf, timeout);
        }
        let taken = buf.len().min(pending.len());
        buf[..taken].copy_from_slice(&pending[..taken]);
        pending.drain(..taken);
        Read::Got(taken)
    }

    fn broken(&self, reason: String) -> Incoming {
        self.connection.fail(reason);
        Incoming::Ended
    }

    fn whole(
        &self,
        reader: &mut Reader,
        head: Head,
        mask: Option<[u8; 4]>,
        mut block: Block,
    ) -> Incoming {
        if let Some(mask) = mask {
            frame::unmask(block.bytes_mut(), mask);
        }
        match head.opcode {
            Opcode::Ping => {
                let _ = self.send(Opcode::Pong, &block);
                Incoming::Idle
            }
            Opcode::Pong => Incoming::Idle,
            Opcode::Close => {
                let reason = frame::close_reason(&block);
                let _ = self.send(Opcode::Close, &block);
                self.broken(reason)
            }
            Opcode::Text | Opcode::Binary => {
                if reader.partial.is_some() {
                    return self.broken(
                        "a new WebSocket message started before the last one ended".to_string(),
                    );
                }
                if head.fin {
                    return message(head.opcode, block);
                }
                reader.partial = Some((head.opcode, block.to_vec()));
                Incoming::Idle
            }
            Opcode::Continuation => {
                let Some((opcode, mut bytes)) = reader.partial.take() else {
                    return self.broken(
                        "a WebSocket continuation without a message to continue".to_string(),
                    );
                };
                if bytes.len() + block.len() > frame::MAX_PAYLOAD {
                    return self.broken(format!(
                        "a fragmented WebSocket message exceeds the {} SDR-- will buffer",
                        frame::MAX_PAYLOAD
                    ));
                }
                bytes.extend_from_slice(&block);
                if !head.fin {
                    reader.partial = Some((opcode, bytes));
                    return Incoming::Idle;
                }
                let mut whole = self.pool.take(bytes.len());
                whole.bytes_mut().copy_from_slice(&bytes);
                message(opcode, whole)
            }
        }
    }
}

fn message(opcode: Opcode, block: Block) -> Incoming {
    if opcode == Opcode::Text {
        Incoming::Text(String::from_utf8_lossy(&block).into_owned())
    } else {
        Incoming::Binary(block)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpStream,
    };

    use super::*;
    use crate::net::testing::FakeServer;

    const POLL: Duration = Duration::from_millis(200);

    fn server_frame(opcode: u8, payload: &[u8], fin: bool) -> Vec<u8> {
        let mut frame = vec![if fin { 0x80 | opcode } else { opcode }];
        match payload.len() {
            len if len < 126 => frame.push(len as u8),
            len if len <= usize::from(u16::MAX) => {
                frame.push(126);
                frame.extend_from_slice(&(len as u16).to_be_bytes());
            }
            len => {
                frame.push(127);
                frame.extend_from_slice(&(len as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(payload);
        frame
    }

    fn upgraded(script: impl Fn(&mut TcpStream) + Send + Sync + 'static) -> FakeServer {
        FakeServer::spawn(move |mut stream, _| {
            let mut request = Vec::new();
            let mut buf = [0u8; 256];
            while find(&request, handshake::TERMINATOR).is_none() {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let text = String::from_utf8_lossy(&request).to_string();
            let key = text
                .split("\r\n")
                .find_map(|line| line.strip_prefix("Sec-WebSocket-Key: "))
                .unwrap_or_default()
                .to_string();
            let response = format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                handshake::accept(&key)
            );
            if stream.write_all(response.as_bytes()).is_err() {
                return;
            }
            script(&mut stream);
        })
    }

    fn connect(server: &FakeServer) -> WebSocket {
        let endpoint = Endpoint::parse(&server.endpoint(), 80).expect("a loopback endpoint");
        WebSocket::connect(&endpoint, "/").expect("the upgrade")
    }

    fn drain(socket: &WebSocket, wanted: usize) -> Vec<Incoming> {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = Vec::new();
        while got.len() < wanted && Instant::now() < deadline {
            match socket.next(POLL) {
                Incoming::Idle => {}
                Incoming::Ended => {
                    got.push(Incoming::Ended);
                    break;
                }
                message => got.push(message),
            }
        }
        got
    }

    #[test]
    fn text_and_binary_messages_come_back_whole() {
        let server = upgraded(|stream| {
            let _ = stream.write_all(&server_frame(0x1, b"{\"a\":1}", true));
            let _ = stream.write_all(&server_frame(0x2, &[1, 2, 3, 4], true));
            let _ = stream.write_all(&server_frame(0x2, &vec![7u8; 70_000], true));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        let got = drain(&socket, 3);
        assert!(matches!(&got[0], Incoming::Text(text) if text == "{\"a\":1}"));
        assert!(matches!(&got[1], Incoming::Binary(block) if **block == [1, 2, 3, 4]));
        assert!(
            matches!(&got[2], Incoming::Binary(block) if block.len() == 70_000),
            "a 64-bit length is framed like any other"
        );
    }

    #[test]
    fn a_message_split_across_frames_and_reads_is_reassembled() {
        let server = upgraded(|stream| {
            let _ = stream.write_all(&server_frame(0x2, &[1, 2], false));
            std::thread::sleep(Duration::from_millis(5));
            let _ = stream.write_all(&server_frame(0x0, &[3, 4], false));
            std::thread::sleep(Duration::from_millis(5));
            let _ = stream.write_all(&server_frame(0x0, &[5], true));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        let got = drain(&socket, 1);
        assert!(matches!(&got[0], Incoming::Binary(block) if **block == [1, 2, 3, 4, 5]));
    }

    #[test]
    fn a_ping_is_answered_and_never_reaches_the_caller() {
        let server = upgraded(|stream| {
            let _ = stream.write_all(&server_frame(0x9, b"beat", true));
            let mut pong = [0u8; 10];
            let echoed = stream.read_exact(&mut pong).is_ok();
            let payload = if echoed {
                let mask = [pong[2], pong[3], pong[4], pong[5]];
                let mut body = pong[6..].to_vec();
                frame::unmask(&mut body, mask);
                body
            } else {
                Vec::new()
            };
            let _ = stream.write_all(&server_frame(0x1, &payload, true));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        let got = drain(&socket, 1);
        assert!(
            matches!(&got[0], Incoming::Text(text) if text == "beat"),
            "the server saw the pong SDR-- sent back: {got:?}"
        );
    }

    #[test]
    fn a_close_frame_ends_the_stream_with_the_reason_it_carried() {
        let server = upgraded(|stream| {
            let mut payload = 1001u16.to_be_bytes().to_vec();
            payload.extend_from_slice(b"going away");
            let _ = stream.write_all(&server_frame(0x8, &payload, true));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        assert!(matches!(drain(&socket, 1)[0], Incoming::Ended));
        assert!(socket.failure().reason.contains("going away"));
    }

    #[test]
    fn a_frame_this_cannot_read_ends_the_stream_instead_of_desynchronising() {
        let server = upgraded(|stream| {
            let _ = stream.write_all(&server_frame(0x3, b"?", true));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        assert!(matches!(drain(&socket, 1)[0], Incoming::Ended));
        assert!(socket.failure().reason.contains("opcode"));
    }

    #[test]
    fn a_server_that_does_not_upgrade_is_refused_with_what_it_said() {
        let server = FakeServer::spawn(|mut stream, _| {
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
        });
        let endpoint = Endpoint::parse(&server.endpoint(), 80).expect("a loopback endpoint");
        let error = WebSocket::connect(&endpoint, "/").expect_err("not a WebSocket server");
        assert!(error.to_string().contains("403"), "{error}");
    }

    #[test]
    fn what_a_client_sends_is_masked_and_arrives_as_text() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let server = upgraded(move |stream| {
            let mut head = [0u8; 2];
            if stream.read_exact(&mut head).is_err() {
                return;
            }
            let mut mask = [0u8; 4];
            if stream.read_exact(&mut mask).is_err() {
                return;
            }
            let mut body = vec![0u8; usize::from(head[1] & 0x7F)];
            if stream.read_exact(&mut body).is_err() {
                return;
            }
            frame::unmask(&mut body, mask);
            lock(&recorder).push((
                head[0],
                head[1] & 0x80 != 0,
                String::from_utf8_lossy(&body).to_string(),
            ));
            std::thread::sleep(Duration::from_secs(5));
        });
        let socket = connect(&server);
        socket.send_text("hello").expect("sends");
        crate::net::testing::eventually("the server to see the message", || {
            lock(&seen).first().cloned()
        });
        let (head, masked, text) = lock(&seen).remove(0);
        assert_eq!(head, 0x81, "a final text frame");
        assert!(masked, "a client frame is always masked");
        assert_eq!(text, "hello");
    }
}
