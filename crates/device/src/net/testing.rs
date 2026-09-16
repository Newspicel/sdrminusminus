#![allow(clippy::expect_used)]
use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::net::websocket::handshake;

/// How long a test waits for a backend to reach the state it is asserting on.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// Waits for `check` to produce something, or fails the test by name.
///
/// A backend answers on threads of its own, so the moment a result appears is not the moment the
/// call that started it returned.
///
/// # Panics
/// If `check` has produced nothing within [`DEADLINE`].
pub fn eventually<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        if let Some(got) = check() {
            return got;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

/// A server on the loopback interface that answers every connection with the scripted handler,
/// so a network backend is exercised without a radio or a host program behind it.
pub struct FakeServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
}

impl FakeServer {
    /// Starts one, handing each connection to `handle` along with the number of connections that
    /// came before it.
    ///
    /// # Panics
    /// If the loopback interface cannot be bound.
    pub fn spawn(handle: impl Fn(TcpStream, usize) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("a bound address");
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = connections.clone();
        std::thread::spawn(move || {
            let handle = Arc::new(handle);
            for stream in listener.incoming().flatten() {
                let nth = counted.fetch_add(1, Ordering::SeqCst);
                let handle = handle.clone();
                std::thread::spawn(move || handle(stream, nth));
            }
        });
        Self { addr, connections }
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        self.addr.to_string()
    }

    /// How many clients have connected since it started.
    #[must_use]
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Starts one that answers the WebSocket upgrade first, so the script speaks messages rather
    /// than frames.
    ///
    /// # Panics
    /// If the loopback interface cannot be bound.
    pub fn spawn_websocket(handle: impl Fn(WebSocketPeer, usize) + Send + Sync + 'static) -> Self {
        Self::spawn(move |stream, nth| {
            if let Some(peer) = WebSocketPeer::upgrade(stream) {
                handle(peer, nth);
            }
        })
    }
}

/// The server half of one upgraded WebSocket, framing what a backend sends and what it is sent.
pub struct WebSocketPeer {
    writer: Mutex<TcpStream>,
    messages: mpsc::Receiver<String>,
}

impl WebSocketPeer {
    fn upgrade(mut stream: TcpStream) -> Option<Self> {
        let mut request = Vec::new();
        let mut buf = [0u8; 512];
        while find(&request, handshake::TERMINATOR).is_none() {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return None,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let key = String::from_utf8_lossy(&request)
            .split("\r\n")
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("sec-websocket-key")
                    .then(|| value.trim().to_string())
            })?;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                    handshake::accept(&key)
                )
                .as_bytes(),
            )
            .ok()?;
        let mut reader = stream.try_clone().ok()?;
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            while let Some((opcode, payload)) = read_frame(&mut reader) {
                if opcode == 0x8 {
                    break;
                }
                if opcode == 0x1
                    && tx
                        .send(String::from_utf8_lossy(&payload).into_owned())
                        .is_err()
                {
                    break;
                }
            }
        });
        Some(Self {
            writer: Mutex::new(stream),
            messages,
        })
    }

    pub fn send_text(&self, text: &str) -> std::io::Result<()> {
        self.send(0x1, text.as_bytes())
    }

    pub fn send_binary(&self, payload: &[u8]) -> std::io::Result<()> {
        self.send(0x2, payload)
    }

    /// Sends one message as several frames, for a backend that has to reassemble them.
    pub fn send_fragmented(&self, payload: &[u8], at: usize) -> std::io::Result<()> {
        let (head, tail) = payload.split_at(at.min(payload.len()));
        self.send_raw(0x2, head, false)?;
        self.send_raw(0x0, tail, true)
    }

    /// The next text message the backend sent, or nothing within `timeout`.
    #[must_use]
    pub fn next_message(&self, timeout: Duration) -> Option<String> {
        self.messages.recv_timeout(timeout).ok()
    }

    fn send(&self, opcode: u8, payload: &[u8]) -> std::io::Result<()> {
        self.send_raw(opcode, payload, true)
    }

    fn send_raw(&self, opcode: u8, payload: &[u8], fin: bool) -> std::io::Result<()> {
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
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        writer.write_all(&frame)
    }
}

fn read_frame(stream: &mut TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).ok()?;
    let len = match head[1] & 0x7F {
        126 => {
            let mut bytes = [0u8; 2];
            stream.read_exact(&mut bytes).ok()?;
            usize::from(u16::from_be_bytes(bytes))
        }
        127 => {
            let mut bytes = [0u8; 8];
            stream.read_exact(&mut bytes).ok()?;
            usize::try_from(u64::from_be_bytes(bytes)).ok()?
        }
        short => usize::from(short),
    };
    let mut mask = [0u8; 4];
    let masked = head[1] & 0x80 != 0;
    if masked {
        stream.read_exact(&mut mask).ok()?;
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    if masked {
        for (at, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[at % 4];
        }
    }
    Some((head[0] & 0x0F, payload))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
