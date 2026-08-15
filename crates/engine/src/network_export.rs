use std::{
    io::Write,
    net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use num_complex::Complex;
use rtrb::{Consumer, Producer, RingBuffer};
use sdrmm_wire::{NetworkExportSettings, NetworkSampleFormat, NetworkTransport};

use crate::EngineError;

const NETWORK_RING_CAPACITY: usize = 1 << 20;
const UDP_PAYLOAD_BYTES: usize = 1_400;
const NETWORK_IO_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug)]
pub(crate) struct NetworkExportShared {
    bytes_per_sample: u64,
    bytes: AtomicU64,
    packets: AtomicU64,
    error: OnceLock<String>,
}

impl NetworkExportShared {
    fn new(format: NetworkSampleFormat) -> Self {
        Self {
            bytes_per_sample: format.bytes_per_sample() as u64,
            bytes: AtomicU64::new(0),
            packets: AtomicU64::new(0),
            error: OnceLock::new(),
        }
    }

    pub(crate) fn samples(&self) -> u64 {
        self.bytes() / self.bytes_per_sample
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub(crate) fn packets(&self) -> u64 {
        self.packets.load(Ordering::Relaxed)
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.error.get().cloned()
    }

    fn fail(&self, message: String) {
        let _ = self.error.set(message);
    }
}

pub(crate) struct NetworkExportTap {
    samples: Producer<Complex<f32>>,
    shared: Arc<NetworkExportShared>,
}

impl NetworkExportTap {
    #[must_use]
    pub(crate) fn push(&mut self, samples: &[Complex<f32>]) -> bool {
        if samples.is_empty() {
            return true;
        }
        match self.samples.push_entire_slice(samples) {
            Ok(()) => true,
            Err(_) if !self.samples.is_abandoned() => {
                self.shared
                    .fail("network export queue overflow — destination too slow?".to_owned());
                false
            }
            Err(_) => {
                self.shared.fail("network export writer stopped".to_owned());
                false
            }
        }
    }
}

enum Connection {
    Udp(UdpSocket),
    Tcp(TcpStream),
}

pub(crate) fn start(
    settings: &NetworkExportSettings,
) -> Result<(NetworkExportTap, Arc<NetworkExportShared>, JoinHandle<()>), EngineError> {
    let connection = connect(settings)?;
    let (producer, consumer) = RingBuffer::new(NETWORK_RING_CAPACITY);
    let shared = Arc::new(NetworkExportShared::new(settings.format));
    let tap = NetworkExportTap {
        samples: producer,
        shared: shared.clone(),
    };
    let worker_shared = shared.clone();
    let format = settings.format;
    let writer = std::thread::Builder::new()
        .name("sdrmm-network-export".to_owned())
        .spawn(move || write_loop(connection, format, consumer, &worker_shared))
        .map_err(|error| EngineError::NetworkExport(format!("spawn writer thread: {error}")))?;
    Ok((tap, shared, writer))
}

fn connect(settings: &NetworkExportSettings) -> Result<Connection, EngineError> {
    let target = settings
        .address
        .to_socket_addrs()
        .map_err(|error| {
            EngineError::NetworkExport(format!("resolve {}: {error}", settings.address))
        })?
        .next()
        .ok_or_else(|| {
            EngineError::NetworkExport(format!(
                "resolve {}: no destination address",
                settings.address
            ))
        })?;
    if target.port() == 0 {
        return Err(EngineError::NetworkExport(
            "destination port must be non-zero".to_owned(),
        ));
    }
    match settings.transport {
        NetworkTransport::Udp => {
            let bind = if target.is_ipv4() {
                SocketAddr::from(([0, 0, 0, 0], 0))
            } else {
                SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 0))
            };
            let socket = UdpSocket::bind(bind)
                .and_then(|socket| {
                    socket.connect(target)?;
                    socket.set_write_timeout(Some(NETWORK_IO_TIMEOUT))?;
                    Ok(socket)
                })
                .map_err(|error| {
                    EngineError::NetworkExport(format!(
                        "open UDP destination {}: {error}",
                        settings.address
                    ))
                })?;
            Ok(Connection::Udp(socket))
        }
        NetworkTransport::Tcp => TcpStream::connect_timeout(&target, NETWORK_IO_TIMEOUT)
            .and_then(|stream| {
                stream.set_write_timeout(Some(NETWORK_IO_TIMEOUT))?;
                Ok(stream)
            })
            .map(Connection::Tcp)
            .map_err(|error| {
                EngineError::NetworkExport(format!(
                    "connect TCP destination {}: {error}",
                    settings.address
                ))
            }),
    }
}

fn write_loop(
    mut connection: Connection,
    format: NetworkSampleFormat,
    mut samples: Consumer<Complex<f32>>,
    shared: &NetworkExportShared,
) {
    let mut encoded = Vec::new();
    loop {
        let available = samples.slots();
        if available == 0 {
            if samples.is_abandoned() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let Ok(chunk) = samples.read_chunk(available) else {
            continue;
        };
        let (first, second) = chunk.as_slices();
        for slice in [first, second] {
            if slice.is_empty() {
                continue;
            }
            encode(format, slice, &mut encoded);
            let result = match &mut connection {
                Connection::Udp(socket) => write_udp(socket, &encoded, format, shared),
                Connection::Tcp(stream) => write_tcp(stream, &encoded, shared),
            };
            if let Err(error) = result {
                shared.fail(format!("network export write failed: {error}"));
                return;
            }
        }
        chunk.commit_all();
    }
}

fn write_udp(
    socket: &UdpSocket,
    encoded: &[u8],
    format: NetworkSampleFormat,
    shared: &NetworkExportShared,
) -> std::io::Result<()> {
    let bytes_per_sample = format.bytes_per_sample();
    let payload = UDP_PAYLOAD_BYTES - UDP_PAYLOAD_BYTES % bytes_per_sample;
    for chunk in encoded.chunks(payload) {
        let sent = socket.send(chunk)?;
        if sent != chunk.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                format!("UDP sent {sent} of {} bytes", chunk.len()),
            ));
        }
        shared.bytes.fetch_add(sent as u64, Ordering::Relaxed);
        shared.packets.fetch_add(1, Ordering::Relaxed);
    }
    Ok(())
}

fn write_tcp(
    stream: &mut TcpStream,
    encoded: &[u8],
    shared: &NetworkExportShared,
) -> std::io::Result<()> {
    let mut written = 0;
    while written < encoded.len() {
        match stream.write(&encoded[written..]) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    format!("TCP accepted {written} of {} bytes", encoded.len()),
                ));
            }
            Ok(sent) => {
                written += sent;
                shared.bytes.fetch_add(sent as u64, Ordering::Relaxed);
                shared.packets.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn encode(format: NetworkSampleFormat, samples: &[Complex<f32>], output: &mut Vec<u8>) {
    output.clear();
    output.reserve(samples.len() * format.bytes_per_sample());
    match format {
        NetworkSampleFormat::Cf32Le => {
            for sample in samples {
                output.extend_from_slice(&sample.re.to_le_bytes());
                output.extend_from_slice(&sample.im.to_le_bytes());
            }
        }
        NetworkSampleFormat::Ci16Le => {
            for sample in samples {
                for component in [sample.re, sample.im] {
                    let value = (component.clamp(-1.0, 1.0) * 32_767.0).round() as i16;
                    output.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        NetworkSampleFormat::Cu8 => {
            for sample in samples {
                for component in [sample.re, sample.im] {
                    output.push((component.clamp(-1.0, 1.0) * 127.5 + 127.5).round() as u8);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTOR: [Complex<f32>; 3] = [
        Complex::new(-1.0, 1.0),
        Complex::new(0.0, -0.5),
        Complex::new(1.5, -2.0),
    ];

    #[test]
    fn cf32_le_is_interleaved_ieee754() {
        let mut bytes = Vec::new();
        encode(NetworkSampleFormat::Cf32Le, &VECTOR, &mut bytes);
        let expected: Vec<u8> = VECTOR
            .iter()
            .flat_map(|sample| [sample.re.to_le_bytes(), sample.im.to_le_bytes()].concat())
            .collect();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn ci16_le_clamps_and_interleaves() {
        let mut bytes = Vec::new();
        encode(NetworkSampleFormat::Ci16Le, &VECTOR, &mut bytes);
        let expected: Vec<u8> = [-32_767i16, 32_767, 0, -16_384, 32_767, -32_767]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        assert_eq!(bytes, expected);
    }

    #[test]
    fn cu8_clamps_and_centers_between_127_and_128() {
        let mut bytes = Vec::new();
        encode(NetworkSampleFormat::Cu8, &VECTOR, &mut bytes);
        assert_eq!(bytes, [0, 255, 128, 64, 255, 0]);
    }

    #[test]
    fn zero_destination_port_is_rejected_before_start() {
        let settings = NetworkExportSettings {
            address: "127.0.0.1:0".to_owned(),
            ..NetworkExportSettings::default()
        };
        let Err(error) = connect(&settings) else {
            panic!("port zero must be rejected");
        };
        assert!(error.to_string().contains("non-zero"));
    }
}
