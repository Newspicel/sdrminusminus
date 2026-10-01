use std::{
    io::{ErrorKind, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};

const TICK: Duration = Duration::from_millis(10);

pub struct Feeder {
    port: u16,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    connected: AtomicBool,
    failed: AtomicBool,
    worst_lag_us: AtomicU64,
}

impl Feeder {
    pub fn start(raw: &Path, rate: f64) -> Result<Self> {
        let data = std::fs::read(raw).with_context(|| format!("read {}", raw.display()))?;
        let listener = TcpListener::bind("127.0.0.1:0").context("open the IQ feed")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared::default());
        let chunk = chunk_bytes(rate);
        let worker = Arc::clone(&shared);
        let thread = thread::spawn(move || serve(&listener, &data, chunk, &worker));
        Ok(Self {
            port,
            shared,
            thread: Some(thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }

    pub fn reset_lag(&self) {
        self.shared.worst_lag_us.store(0, Ordering::Release);
    }

    pub fn worst_lag(&self) -> Duration {
        Duration::from_micros(self.shared.worst_lag_us.load(Ordering::Acquire))
    }
}

impl Drop for Feeder {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            eprintln!("the IQ feed thread panicked");
        }
    }
}

fn chunk_bytes(rate: f64) -> usize {
    (rate * TICK.as_secs_f64()).round() as usize * 8
}

fn serve(listener: &TcpListener, data: &[u8], chunk: usize, shared: &Shared) {
    let Some(stream) = accept(listener, shared) else {
        return;
    };
    shared.connected.store(true, Ordering::Release);
    if let Err(err) = stream_paced(stream, data, chunk, shared)
        && !shared.stop.load(Ordering::Acquire)
    {
        shared.failed.store(true, Ordering::Release);
        eprintln!("the IQ feed stopped: {err}");
    }
}

fn accept(listener: &TcpListener, shared: &Shared) -> Option<TcpStream> {
    while !shared.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(err) if err.kind() == ErrorKind::WouldBlock => thread::sleep(TICK),
            Err(err) => {
                eprintln!("the IQ feed could not accept: {err}");
                return None;
            }
        }
    }
    None
}

fn stream_paced(mut stream: TcpStream, data: &[u8], chunk: usize, shared: &Shared) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let start = Instant::now();
    let mut offset = 0;
    let mut ticks: u32 = 0;
    while !shared.stop.load(Ordering::Acquire) {
        let deadline = start + TICK * ticks;
        if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        let (piece, next) = slice(data, offset, chunk);
        stream.write_all(piece)?;
        offset = next;
        if piece.len() == chunk {
            ticks += 1;
            let lag = Instant::now().saturating_duration_since(deadline);
            shared
                .worst_lag_us
                .fetch_max(lag.as_micros() as u64, Ordering::AcqRel);
        }
    }
    Ok(())
}

fn slice(data: &[u8], offset: usize, chunk: usize) -> (&[u8], usize) {
    let end = (offset + chunk).min(data.len());
    let next = if end == data.len() { 0 } else { end };
    (&data[offset..end], next)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn a_chunk_is_ten_milliseconds_of_complex_floats() {
        assert_eq!(chunk_bytes(10_000_000.0), 800_000);
    }

    #[test]
    fn slices_wrap_to_the_start() {
        let data = [1, 2, 3, 4, 5];
        assert_eq!(slice(&data, 0, 2), (&data[..2], 2));
        assert_eq!(slice(&data, 4, 2), (&data[4..], 0));
    }

    #[test]
    fn a_client_receives_the_recording_in_a_loop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iq");
        std::fs::write(&path, (0..16u8).collect::<Vec<_>>()).unwrap();
        let feeder = Feeder::start(&path, 100.0).unwrap();
        let mut client = TcpStream::connect(("127.0.0.1", feeder.port())).unwrap();
        let mut received = [0u8; 24];
        client.read_exact(&mut received).unwrap();
        assert!(feeder.connected());
        assert_eq!(&received[..16], (0..16u8).collect::<Vec<_>>().as_slice());
        assert_eq!(&received[16..], &[0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
