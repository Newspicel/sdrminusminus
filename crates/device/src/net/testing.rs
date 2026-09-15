#![allow(clippy::expect_used)]
use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

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
}
