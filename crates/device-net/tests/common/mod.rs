#![allow(clippy::expect_used)]
//! A harness that cannot bind a loopback socket or clone it has nothing left to assert,
//! so its helpers panic. Clippy exempts `#[test]` functions from this by config, but not
//! the free functions and closures a fake server is built out of.
//! Shared scaffolding for the fake-server tests: a loopback listener whose connections a test
//! scripts, and the two waits every test here needs.
use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

/// How long a test will wait for something that should take milliseconds.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// Poll `check` until it holds, or fail naming what was being waited for.
pub fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {what}");
}

pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A server on a loopback port the OS picked, handing each accepted connection to `handle`.
///
/// The listener thread outlives the test process rather than being joined: a test that has
/// finished asserting has nothing to wait for, and a handler blocked writing to a client that has
/// gone would only make the run slower.
pub struct FakeServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
}

impl FakeServer {
    pub fn spawn(handle: impl Fn(TcpStream, usize) + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
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

    /// The `host:port` an operator would type to reach this server.
    pub fn endpoint(&self) -> String {
        self.addr.to_string()
    }

    /// How many connections have been accepted — one per open, and one more per reconnect.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}
