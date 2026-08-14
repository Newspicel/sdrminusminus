//! The transport both network backends stream over: one TCP connection, shared by the control
//! thread that writes commands into it and the capture thread that drains samples out of it.
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

/// How long a control write may block before the peer counts as wedged. A command is at most
/// eight bytes and cannot fill a socket buffer on its own, so reaching this means the far side has
/// stopped reading entirely.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// What one read off the socket produced.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Read {
    /// This many bytes landed at the front of the caller's buffer.
    Got(usize),
    /// The timeout expired with nothing to read. Not an error — it is how the capture supervisor
    /// gets a chance to look at its stop flag.
    Idle,
    /// The connection is over; [`Connection::failure`] says why.
    Ended,
}

/// A live connection to a remote receiver.
#[derive(Debug)]
pub(crate) struct Connection {
    socket: Arc<TcpStream>,
    /// The read timeout currently set on the socket, in microseconds. The supervisor passes the
    /// same poll interval every time, so caching it turns one `setsockopt` per block into one per
    /// connection.
    timeout_us: AtomicU64,
    failure: Mutex<Option<StreamFailure>>,
}

impl Connection {
    pub(crate) fn new(socket: TcpStream) -> Self {
        // A blocked control write must not park the control thread for the stack's own default.
        let _ = socket.set_write_timeout(Some(WRITE_TIMEOUT));
        Self {
            socket: Arc::new(socket),
            timeout_us: AtomicU64::new(0),
            failure: Mutex::new(None),
        }
    }

    /// A handle that ends this connection from another thread.
    pub(crate) fn stop_handle(&self) -> SocketStop {
        SocketStop {
            socket: self.socket.clone(),
        }
    }

    /// Close the connection. Idempotent, and safe from any thread: it is what unblocks a capture
    /// thread parked in `read`.
    pub(crate) fn close(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }

    /// Send a command. Every command in both protocols is a short fixed-size frame, so a partial
    /// write is a failure of the connection rather than something to resume.
    ///
    /// # Errors
    /// [`DeviceError::Io`] naming what the socket refused. The caller's setting has not been
    /// applied and must not be reported as if it had.
    pub(crate) fn send(&self, frame: &[u8]) -> Result<(), DeviceError> {
        (&*self.socket)
            .write_all(frame)
            .map_err(|e| DeviceError::Io(format!("send: {e}")))
    }

    /// Fill the front of `buf`, waiting at most `timeout`.
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
            // `WouldBlock` and `TimedOut` are the same event on different platforms; `Interrupted`
            // is a signal, and the supervisor's next pass simply asks again.
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

    /// End this connection with a reason, keeping the first one given: a shutdown from the stop
    /// handle makes every later read fail too, and "connection reset" would bury the reason that
    /// matters. Also how a backend reports a frame it cannot parse — the bytes are no longer where
    /// they are thought to be, and only a fresh connection can fix that.
    pub(crate) fn fail(&self, reason: String) -> Read {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            // Never fatal. `fatal` means re-arming in place cannot help because the device left
            // the bus — but a remote receiver is reached by dialling it again, which is exactly
            // what a tier-1 restart does here, and the server may simply have been restarted.
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

/// Ends a connection from a thread that is not the one draining it.
#[derive(Clone, Debug)]
pub(crate) struct SocketStop {
    socket: Arc<TcpStream>,
}

impl StopHandle for SocketStop {
    fn stop(&self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

/// Reusable capture buffers.
///
/// A capture stream hands out owned blocks — the supervisor's trait cannot lend from the stream,
/// because the block outlives the borrow — so without a pool every block on the sample path would
/// be a fresh zeroed allocation. Blocks return themselves here when they drop, so a steady stream
/// allocates once and then never again.
#[derive(Clone, Debug, Default)]
pub(crate) struct BlockPool {
    free: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl BlockPool {
    /// A block of exactly `len` bytes, whose contents are unspecified: every caller fills the
    /// prefix it later truncates to.
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

/// A capture block that goes back to its pool when the supervisor is done with it.
#[derive(Debug)]
pub(crate) struct Block {
    bytes: Vec<u8>,
    pool: BlockPool,
}

impl Block {
    /// Keep the first `len` bytes — what a short read leaves.
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
        // One capture thread holds at most a block at a time; anything beyond a handful means a
        // pool being used as a leak, and dropping the buffer is better than growing forever.
        if free.len() < 4 {
            free.push(std::mem::take(&mut self.bytes));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use super::*;

    /// A listener on a loopback port the OS picked, and the client end of one connection to it.
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

    /// The property the supervisor's stop depends on: a read parked in the kernel has to come back
    /// when another thread closes the socket.
    #[test]
    fn a_stop_handle_unblocks_a_parked_read() {
        let (_server, conn) = connected();
        let stop = conn.stop_handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            stop.stop();
        });
        let mut buf = [0u8; 8];
        // Far longer than the stop takes: if the shutdown did not reach the reader, this hangs.
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
