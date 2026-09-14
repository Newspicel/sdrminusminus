use std::{
    io::{ErrorKind, Read as _, Write as _},
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::{DeviceError, StopHandle, StreamFailure, lock};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Eq, PartialEq)]
pub enum Read {
    Got(usize),
    Idle,
    Ended,
}

#[derive(Debug)]
pub struct Connection {
    socket: Arc<TcpStream>,
    timeout_us: AtomicU64,
    failure: Mutex<Option<StreamFailure>>,
}

impl Connection {
    pub fn new(socket: TcpStream) -> Self {
        let _ = socket.set_write_timeout(Some(WRITE_TIMEOUT));
        Self {
            socket: Arc::new(socket),
            timeout_us: AtomicU64::new(0),
            failure: Mutex::new(None),
        }
    }

    pub fn stop_handle(&self) -> SocketStop {
        SocketStop {
            socket: self.socket.clone(),
        }
    }

    pub fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    pub fn send(&self, frame: &[u8]) -> Result<(), DeviceError> {
        (&*self.socket)
            .write_all(frame)
            .map_err(|e| DeviceError::Io(format!("send: {e}")))
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
