#![allow(clippy::expect_used)]
use std::{
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub const DEADLINE: Duration = Duration::from_secs(10);

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

    pub fn endpoint(&self) -> String {
        self.addr.to_string()
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}
