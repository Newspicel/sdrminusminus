use std::{
    io::{ErrorKind, Read as _, Write as _},
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use rustls::{ClientConfig, ClientConnection};

use crate::{
    DeviceError, StopHandle, StreamFailure, lock,
    net::{
        CONNECT_TIMEOUT, Endpoint,
        tls::{self, Plain},
    },
};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const TLS_CHUNK: usize = 8 << 10;

#[derive(Debug, Eq, PartialEq)]
pub enum Read {
    Got(usize),
    Idle,
    Ended,
}

#[derive(Debug)]
pub struct Connection {
    socket: Arc<TcpStream>,
    tls: Option<Mutex<ClientConnection>>,
    timeout_us: AtomicU64,
    failure: Mutex<Option<StreamFailure>>,
}

impl Connection {
    pub fn new(socket: TcpStream) -> Self {
        let _ = socket.set_write_timeout(Some(WRITE_TIMEOUT));
        Self {
            socket: Arc::new(socket),
            tls: None,
            timeout_us: AtomicU64::new(0),
            failure: Mutex::new(None),
        }
    }

    pub fn dial(endpoint: &Endpoint) -> Result<Self, DeviceError> {
        let socket = endpoint.connect()?;
        if !endpoint.secure() {
            return Ok(Self::new(socket));
        }
        Self::secure(socket, endpoint.host(), tls::platform()?)
    }

    pub fn secure(
        socket: TcpStream,
        host: &str,
        config: Arc<ClientConfig>,
    ) -> Result<Self, DeviceError> {
        let _ = socket.set_read_timeout(Some(CONNECT_TIMEOUT));
        let session = tls::handshake(&socket, host, config)?;
        Ok(Self {
            tls: Some(Mutex::new(session)),
            ..Self::new(socket)
        })
    }

    pub fn stop_handle(&self) -> SocketStop {
        SocketStop {
            socket: self.socket.clone(),
        }
    }

    pub fn close(&self) {
        if let Some(session) = &self.tls {
            let mut session = lock(session);
            session.send_close_notify();
            let _ = tls::flush(&mut session, &self.socket);
        }
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    pub fn send(&self, frame: &[u8]) -> Result<(), DeviceError> {
        let sent = match &self.tls {
            Some(session) => tls::seal(&mut lock(session), &self.socket, frame),
            None => (&*self.socket).write_all(frame),
        };
        sent.map_err(|e| DeviceError::Io(format!("send: {e}")))
    }

    pub fn read(&self, buf: &mut [u8], timeout: Duration) -> Read {
        let wanted = u64::try_from(timeout.as_micros())
            .unwrap_or(u64::MAX)
            .max(1);
        if self.timeout_us.swap(wanted, Ordering::Relaxed) != wanted
            && let Err(e) = self
                .socket
                .set_read_timeout(Some(Duration::from_micros(wanted)))
        {
            return self.fail(format!("set read timeout: {e}"));
        }
        match &self.tls {
            Some(session) => self.read_secure(session, buf),
            None => self.read_raw(buf),
        }
    }

    fn read_secure(&self, session: &Mutex<ClientConnection>, buf: &mut [u8]) -> Read {
        loop {
            match tls::plaintext(&mut lock(session), buf) {
                Plain::Got(n) => return Read::Got(n),
                Plain::Closed => return self.fail("the server closed the connection".to_string()),
                Plain::Broken(reason) => return self.fail(reason),
                Plain::Empty => {}
            }
            let mut raw = [0u8; TLS_CHUNK];
            let n = match self.read_raw(&mut raw) {
                Read::Got(n) => n,
                other => return other,
            };
            if let Err(reason) = tls::absorb(&mut lock(session), &self.socket, &raw[..n]) {
                return self.fail(reason);
            }
        }
    }

    fn read_raw(&self, buf: &mut [u8]) -> Read {
        match (&*self.socket).read(buf) {
            Ok(0) => self.fail("the server closed the connection".to_string()),
            Ok(n) => Read::Got(n),
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                Read::Idle
            }
            Err(e) => self.fail(e.to_string()),
        }
    }

    pub fn fail(&self, reason: String) -> Read {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            *failure = Some(StreamFailure {
                reason,
                gone: false,
            });
        }
        Read::Ended
    }

    pub fn failure(&self) -> StreamFailure {
        lock(&self.failure).clone().unwrap_or(StreamFailure {
            reason: "the connection ended".to_string(),
            gone: false,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SocketStop {
    socket: Arc<TcpStream>,
}

impl StopHandle for SocketStop {
    fn stop(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use super::*;

    fn connected() -> (TcpStream, Connection) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let client = TcpStream::connect(addr).expect("connect");
        let (server, _) = listener.accept().expect("accept");
        (server, Connection::new(client))
    }

    fn tls_pair() -> (Arc<rustls::ServerConfig>, Arc<ClientConfig>) {
        let issued =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).expect("a cert");
        let cert = issued.cert.der().clone();
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(issued.signing_key.serialize_der().into());
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let server = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .expect("server config");
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert).expect("trusted");
        let client = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        (Arc::new(server), Arc::new(client))
    }

    #[test]
    fn a_tls_connection_carries_bytes_both_ways_and_idles_when_quiet() {
        let (server_config, client_config) = tls_pair();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("accept");
            let session = rustls::ServerConnection::new(server_config).expect("session");
            let mut stream = rustls::StreamOwned::new(session, socket);
            let mut ping = [0u8; 4];
            stream.read_exact(&mut ping).expect("ping");
            stream.write_all(&ping).expect("echo");
            stream.write_all(&vec![7u8; 40_000]).expect("bulk");
            stream.flush().expect("flush");
            let _ = released.recv();
            stream.conn.send_close_notify();
            let _ = stream.flush();
        });
        let socket = TcpStream::connect(addr).expect("connect");
        let conn = Connection::secure(socket, "localhost", client_config).expect("handshake");
        conn.send(b"ping").expect("send");
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while got.len() < 4 + 40_000 {
            match conn.read(&mut buf, Duration::from_secs(5)) {
                Read::Got(n) => got.extend_from_slice(&buf[..n]),
                other => panic!("stream stalled: {other:?}"),
            }
        }
        assert_eq!(&got[..4], b"ping");
        assert!(got[4..].iter().all(|b| *b == 7));
        assert_eq!(conn.read(&mut buf, Duration::from_millis(20)), Read::Idle);
        release.send(()).expect("released");
        assert_eq!(conn.read(&mut buf, Duration::from_secs(5)), Read::Ended);
        assert!(conn.failure().reason.contains("closed"));
    }

    #[test]
    fn a_certificate_nobody_trusts_is_refused() {
        let (server_config, _) = tls_pair();
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("accept");
            let session = rustls::ServerConnection::new(server_config).expect("session");
            let mut stream = rustls::StreamOwned::new(session, socket);
            let _ = stream.read(&mut [0u8; 1]);
        });
        let (_, untrusting) = tls_pair();
        let socket = TcpStream::connect(addr).expect("connect");
        assert!(Connection::secure(socket, "localhost", untrusting).is_err());
    }

    #[test]
    fn a_quiet_socket_is_idle_and_a_closed_one_ends() {
        let (mut server, conn) = connected();
        let mut buf = [0u8; 8];
        assert_eq!(
            conn.read(&mut buf, Duration::from_millis(20)),
            Read::Idle,
            "a server with nothing to say must not read as a failure"
        );
        server.write_all(b"abc").expect("write");
        assert_eq!(
            conn.read(&mut buf, Duration::from_millis(500)),
            Read::Got(3)
        );
        assert_eq!(&buf[..3], b"abc");
        drop(server);
        assert_eq!(conn.read(&mut buf, Duration::from_millis(500)), Read::Ended);
        assert!(conn.failure().reason.contains("closed"));
        assert!(!conn.failure().gone, "a remote can always be dialled again");
    }

    #[test]
    fn a_stop_handle_unblocks_a_parked_read() {
        let (_server, conn) = connected();
        let stop = conn.stop_handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            stop.stop();
        });
        let mut buf = [0u8; 8];
        assert_eq!(conn.read(&mut buf, Duration::from_secs(30)), Read::Ended);
    }

    #[test]
    fn the_first_failure_reason_is_the_one_reported() {
        let (server, conn) = connected();
        drop(server);
        let mut buf = [0u8; 8];
        assert_eq!(conn.read(&mut buf, Duration::from_millis(500)), Read::Ended);
        let first = conn.failure().reason;
        conn.close();
        assert_eq!(conn.read(&mut buf, Duration::from_millis(500)), Read::Ended);
        assert_eq!(conn.failure().reason, first);
    }
}
