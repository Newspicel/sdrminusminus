use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::atomic::Ordering,
};

use super::NetworkExportShared;

const MAX_CLIENTS: usize = 8;
const MAX_PENDING_BYTES: usize = 2 * 1024 * 1024;
const HEADER: [u8; 12] = [b'R', b'T', b'L', b'0', 0, 0, 0, 0, 0, 0, 0, 0];

pub(super) struct Server {
    listener: TcpListener,
    clients: Vec<Client>,
}

struct Client {
    stream: TcpStream,
    pending: Vec<u8>,
    written: usize,
    header_remaining: usize,
}

impl Server {
    pub fn bind(address: SocketAddr) -> io::Result<Self> {
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            clients: Vec::new(),
        })
    }

    pub fn poll(&mut self, shared: &NetworkExportShared) -> io::Result<()> {
        for _ in 0..MAX_CLIENTS {
            let (stream, peer) = match self.listener.accept() {
                Ok(client) => client,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if self.clients.len() == MAX_CLIENTS {
                shared.fail("rtl_tcp client limit reached".to_owned());
                continue;
            }
            stream.set_nonblocking(true)?;
            stream.set_nodelay(true)?;
            tracing::info!(%peer, "rtl_tcp client connected; tuning commands do not change the wired source");
            self.clients.push(Client {
                stream,
                pending: HEADER.to_vec(),
                written: 0,
                header_remaining: HEADER.len(),
            });
        }
        self.clients.retain_mut(|client| match client.poll(shared) {
            Ok(connected) => connected,
            Err(error) => {
                shared.fail(format!("rtl_tcp client disconnected: {error}"));
                false
            }
        });
        shared
            .clients
            .store(self.clients.len() as u32, Ordering::Relaxed);
        Ok(())
    }

    pub fn broadcast(&mut self, bytes: &[u8], shared: &NetworkExportShared) -> io::Result<()> {
        self.clients.retain_mut(|client| {
            if client.pending.len() - client.written + bytes.len() > MAX_PENDING_BYTES {
                shared.fail("rtl_tcp slow client disconnected: sample queue overflow".to_owned());
                return false;
            }
            client.pending.drain(..client.written);
            client.written = 0;
            client.pending.extend_from_slice(bytes);
            true
        });
        self.poll(shared)
    }
}

impl Client {
    fn poll(&mut self, shared: &NetworkExportShared) -> io::Result<bool> {
        let mut commands = [0u8; 1024];
        match self.stream.read(&mut commands) {
            Ok(0) => return Ok(false),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) => {}
            Err(error) => return Err(error),
        }
        while self.written < self.pending.len() {
            match self.stream.write(&self.pending[self.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(count) => {
                    self.written += count;
                    let header = count.min(self.header_remaining);
                    self.header_remaining -= header;
                    if count > header {
                        shared
                            .bytes
                            .fetch_add((count - header) as u64, Ordering::Relaxed);
                        shared.packets.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use sdrmm_wire::NetworkSampleFormat;

    use super::*;
    use crate::network_export::NetworkExportShared;

    fn pump(server: &mut Server, shared: &NetworkExportShared, clients: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            server.poll(shared).unwrap();
            if server.clients.len() == clients
                && server
                    .clients
                    .iter()
                    .all(|client| client.written == client.pending.len())
            {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "listener did not settle: {:?}",
                shared.error()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn clients_get_the_header_and_cu8_and_can_reconnect() {
        let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = server.listener.local_addr().unwrap();
        let shared = NetworkExportShared::new(NetworkSampleFormat::Cu8);
        server.broadcast(&[7, 8], &shared).unwrap();
        for _ in 0..2 {
            let mut client = TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            pump(&mut server, &shared, 1);
            let mut header = [0; 12];
            client.read_exact(&mut header).unwrap();
            assert_eq!(header, HEADER);
            client.write_all(&[1, 0, 0, 0, 0, 2, 0, 0, 0, 0]).unwrap();
            server.broadcast(&[0, 255, 128, 64], &shared).unwrap();
            pump(&mut server, &shared, 1);
            let mut iq = [0; 4];
            client.read_exact(&mut iq).unwrap();
            assert_eq!(iq, [0, 255, 128, 64]);
            assert_eq!(shared.clients(), 1);
            drop(client);
            pump(&mut server, &shared, 0);
            assert_eq!(shared.clients(), 0);
        }
        assert_eq!(shared.bytes(), 8);
        assert!(shared.error().is_none());
    }

    #[test]
    fn an_overflow_disconnects_only_the_slow_client_and_surfaces_loss() {
        let mut server = Server::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = server.listener.local_addr().unwrap();
        let _slow = TcpStream::connect(address).unwrap();
        let mut fast = TcpStream::connect(address).unwrap();
        fast.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let shared = NetworkExportShared::new(NetworkSampleFormat::Cu8);
        pump(&mut server, &shared, 2);
        server.clients[0].pending = vec![0; MAX_PENDING_BYTES];
        server.clients[0].written = 0;
        server.broadcast(&[42, 43], &shared).unwrap();
        pump(&mut server, &shared, 1);
        let mut bytes = [0; 14];
        fast.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes[..12], &HEADER);
        assert_eq!(&bytes[12..], &[42, 43]);
        assert_eq!(shared.clients(), 1);
        assert!(shared.error().unwrap().contains("overflow"));
    }
}
