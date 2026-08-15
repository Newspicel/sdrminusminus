use std::{
    io::{ErrorKind, Read as _, Write as _},
    net::{Shutdown, TcpStream},
    ops::Deref,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use sdrmm_device::{DeviceError, StopHandle, StreamFailure, lock};

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Read {
    Got(usize),
    Idle,
    Ended,
}

#[derive(Debug)]
pub(crate) struct Connection {
    socket: Arc<TcpStream>,
    timeout_us: AtomicU64,
    failure: Mutex<Option<StreamFailure>>,
}

impl Connection {
    pub(crate) fn new(socket: TcpStream) -> Self {
        let _ = socket.set_write_timeout(Some(WRITE_TIMEOUT));
        Self {
            socket: Arc::new(socket),
            timeout_us: AtomicU64::new(0),
            failure: Mutex::new(None),
        }
    }

    pub(crate) fn stop_handle(&self) -> SocketStop {
        SocketStop {
            socket: self.socket.clone(),
        }
    }

    pub(crate) fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    pub(crate) fn send(&self, frame: &[u8]) -> Result<(), DeviceError> {
        (&*self.socket)
            .write_all(frame)
            .map_err(|e| DeviceError::Io(format!("send: {e}")))
    }

    pub(crate) fn read(&self, buf: &mut [u8], timeout: Duration) -> Read {
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

    pub(crate) fn fail(&self, reason: String) -> Read {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            *failure = Some(StreamFailure {
                reason,
                fatal: false,
            });
        }
        Read::Ended
    }

    pub(crate) fn failure(&self) -> StreamFailure {
        lock(&self.failure).clone().unwrap_or(StreamFailure {
            reason: "the connection ended".to_string(),
            fatal: false,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SocketStop {
    socket: Arc<TcpStream>,
}

impl StopHandle for SocketStop {
    fn stop(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct BlockPool {
    free: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl BlockPool {
    pub(crate) fn take(&self, len: usize) -> Block {
        let mut bytes = lock(&self.free).pop().unwrap_or_default();
        bytes.clear();
        bytes.resize(len, 0);
        Block {
            bytes,
            pool: self.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Block {
    bytes: Vec<u8>,
    pool: BlockPool,
}

impl Block {
    pub(crate) fn truncate(&mut self, len: usize) {
        self.bytes.truncate(len);
    }

    pub(crate) fn as_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
}

impl Deref for Block {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Block {
    fn drop(&mut self) {
        let mut free = lock(&self.pool.free);
        if free.len() < 4 {
            free.push(std::mem::take(&mut self.bytes));
        }
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
        assert!(
            !conn.failure().fatal,
            "a remote can always be dialled again"
        );
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

    #[test]
    fn blocks_come_back_to_the_pool_and_are_reused() {
        let pool = BlockPool::default();
        let address = {
            let mut block = pool.take(64);
            assert_eq!(block.len(), 64);
            block.truncate(8);
            assert_eq!(block.len(), 8);
            block.as_ptr()
        };
        let block = pool.take(64);
        assert_eq!(
            block.as_ptr(),
            address,
            "the capture path must not allocate"
        );
        assert_eq!(block.len(), 64, "a reused block is resized back to full");
    }
}
