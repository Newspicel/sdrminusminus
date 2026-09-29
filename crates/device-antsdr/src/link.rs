use std::{
    io::ErrorKind,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    time::Duration,
};

use sdrmm_device::DeviceError;
use socket2::{Domain, Protocol, Socket, Type};

pub(crate) const DEFAULT_PORT: u16 = 49200;
pub(crate) const MTU: usize = 1472;
const FIND_BELOW_CONTROL: u16 = 100;
const TX_ABOVE_CONTROL: u16 = 2;
const RX_ABOVE_CONTROL: u16 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ports {
    pub(crate) control: u16,
}

impl Ports {
    pub(crate) const fn find(self) -> u16 {
        self.control.wrapping_sub(FIND_BELOW_CONTROL)
    }

    pub(crate) const fn rx(self) -> u16 {
        self.control.wrapping_add(RX_ABOVE_CONTROL)
    }

    pub(crate) const fn tx(self, lane: usize) -> u16 {
        self.control
            .wrapping_add(TX_ABOVE_CONTROL)
            .wrapping_add(lane as u16)
    }
}

pub(crate) fn resolve(host: &str, port: u16) -> Result<SocketAddr, DeviceError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|e| DeviceError::NotFound(format!("{host}: {e}")))?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| DeviceError::NotFound(format!("{host}: no IPv4 address")))
}

pub(crate) fn connect(
    host: &str,
    port: u16,
    receive_buffer: usize,
) -> Result<UdpSocket, DeviceError> {
    let peer = resolve(host, port)?;
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(io)?;
    if let Err(e) = socket.set_recv_buffer_size(receive_buffer) {
        tracing::debug!("receive buffer of {receive_buffer} bytes refused: {e}");
    }
    let any: SocketAddr = ([0, 0, 0, 0], 0).into();
    socket.bind(&any.into()).map_err(io)?;
    socket.connect(&peer.into()).map_err(io)?;
    let socket: UdpSocket = socket.into();
    Ok(socket)
}

pub(crate) fn io(e: std::io::Error) -> DeviceError {
    DeviceError::Io(e.to_string())
}

pub(crate) enum Received {
    Got(usize),
    Quiet,
}

pub(crate) fn receive(
    socket: &UdpSocket,
    buf: &mut [u8],
    wait: Duration,
) -> Result<Received, DeviceError> {
    socket
        .set_read_timeout(Some(wait.max(Duration::from_millis(1))))
        .map_err(io)?;
    match socket.recv(buf) {
        Ok(n) => Ok(Received::Got(n)),
        Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            Ok(Received::Quiet)
        }
        Err(e) if e.kind() == ErrorKind::ConnectionRefused => Err(DeviceError::Disconnected(
            "the radio refused the connection".to_string(),
        )),
        Err(e) => Err(io(e)),
    }
}

pub(crate) fn flush(socket: &UdpSocket) {
    let mut buf = [0u8; MTU];
    while let Ok(Received::Got(_)) = receive(socket, &mut buf, Duration::from_millis(1)) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_port_sits_at_a_fixed_distance_from_the_control_port() {
        let ports = Ports {
            control: DEFAULT_PORT,
        };
        assert_eq!(ports.find(), 49100);
        assert_eq!(ports.tx(0), 49202);
        assert_eq!(ports.tx(1), 49203);
        assert_eq!(ports.rx(), 49204);
    }
}
