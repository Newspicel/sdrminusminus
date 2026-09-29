use std::{
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use sdrmm_device::DeviceError;

use crate::link::{self, Ports};

pub(crate) const WELL_KNOWN: [&str; 2] = ["192.168.1.10", "192.168.1.255"];
const HELLO_BYTES: usize = 56;
const SERIAL_AT: usize = 16;
const SERIAL_BYTES: usize = 32;
const BOARD_AT: usize = SERIAL_AT + SERIAL_BYTES;
const BOARD_BYTES: usize = 8;
const ASK: [u8; 4] = *b"1m9j";
const ANSWER: [u8; 4] = *b"1M0c";
pub(crate) const REPLY_TIMEOUT: Duration = Duration::from_millis(400);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) address: SocketAddr,
    pub(crate) serial: Option<String>,
    pub(crate) board: String,
}

impl Identity {
    pub(crate) fn label(&self) -> String {
        let model = if self.board.starts_with("E310") {
            "AntSDR E310"
        } else if self.board.starts_with("E200") {
            "AntSDR E200"
        } else {
            "AntSDR"
        };
        match &self.serial {
            Some(serial) => format!("{model} {}", tail(serial)),
            None => model.to_string(),
        }
    }
}

fn tail(serial: &str) -> &str {
    let start = serial.len().saturating_sub(6);
    serial.get(start..).unwrap_or(serial)
}

pub(crate) fn hello() -> [u8; HELLO_BYTES] {
    let mut packet = [0u8; HELLO_BYTES];
    for (slot, byte) in ASK.iter().enumerate() {
        packet[slot * 4 + 3] = *byte;
    }
    packet
}

pub(crate) fn parse(datagram: &[u8], address: SocketAddr) -> Option<Identity> {
    if datagram.len() < SERIAL_AT {
        return None;
    }
    let answered = ANSWER
        .iter()
        .enumerate()
        .all(|(slot, byte)| datagram[slot * 4..slot * 4 + 4] == [0, 0, 0, *byte]);
    if !answered {
        return None;
    }
    let serial = text(datagram, SERIAL_AT, SERIAL_BYTES).to_ascii_uppercase();
    let board = text(datagram, BOARD_AT, BOARD_BYTES);
    Some(Identity {
        address,
        serial: (!serial.is_empty()).then_some(serial),
        board: if board.starts_with('E') {
            board
        } else {
            "E200".to_string()
        },
    })
}

fn text(datagram: &[u8], at: usize, len: usize) -> String {
    let field = datagram
        .get(at..(at + len).min(datagram.len()))
        .unwrap_or(&[]);
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_string()
}

pub(crate) fn ask(hosts: &[String], ports: Ports, wait: Duration) -> Vec<Identity> {
    let socket = match open() {
        Ok(socket) => socket,
        Err(e) => {
            tracing::debug!("antsdr search socket: {e}");
            return Vec::new();
        }
    };
    for host in hosts {
        match link::resolve(host, ports.find()) {
            Ok(address) => {
                if let Err(e) = socket.send_to(&hello(), address) {
                    tracing::debug!(host, "antsdr hello: {e}");
                }
            }
            Err(e) => tracing::debug!(host, "antsdr hello: {e}"),
        }
    }
    collect(&socket, wait)
}

fn open() -> Result<UdpSocket, DeviceError> {
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(link::io)?;
    socket.set_broadcast(true).map_err(link::io)?;
    Ok(socket)
}

fn collect(socket: &UdpSocket, wait: Duration) -> Vec<Identity> {
    let deadline = Instant::now() + wait;
    let mut found: Vec<Identity> = Vec::new();
    let mut buf = [0u8; link::MTU];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || socket.set_read_timeout(Some(left)).is_err() {
            break;
        }
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            break;
        };
        if let Some(identity) = parse(&buf[..n], from)
            && !found.iter().any(|known| known.address.ip() == from.ip())
        {
            found.push(identity);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(serial: &str, board: &str) -> Vec<u8> {
        let mut packet = vec![0u8; HELLO_BYTES];
        for (slot, byte) in ANSWER.iter().enumerate() {
            packet[slot * 4 + 3] = *byte;
        }
        packet[SERIAL_AT..SERIAL_AT + serial.len()].copy_from_slice(serial.as_bytes());
        packet[BOARD_AT..BOARD_AT + board.len()].copy_from_slice(board.as_bytes());
        packet
    }

    fn at() -> SocketAddr {
        ([192, 168, 1, 10], 49100).into()
    }

    #[test]
    fn the_hello_asks_in_network_byte_order() {
        let packet = hello();
        assert_eq!(
            &packet[..16],
            &[0, 0, 0, b'1', 0, 0, 0, b'm', 0, 0, 0, b'9', 0, 0, 0, b'j']
        );
    }

    #[test]
    fn an_answer_names_the_board_and_its_serial() {
        let identity = parse(&answer("a1b2c3d4e5f6", "E310  v2"), at()).expect("answer");
        assert_eq!(identity.serial.as_deref(), Some("A1B2C3D4E5F6"));
        assert_eq!(identity.board, "E310  v2");
        assert_eq!(identity.label(), "AntSDR E310 D4E5F6");
    }

    #[test]
    fn a_board_without_a_version_is_an_e200() {
        let identity = parse(&answer("", ""), at()).expect("answer");
        assert_eq!(identity.board, "E200");
        assert_eq!(identity.serial, None);
        assert_eq!(identity.label(), "AntSDR E200");
    }

    #[test]
    fn anything_else_is_not_an_answer() {
        assert!(parse(&hello(), at()).is_none());
        assert!(parse(&[0; 8], at()).is_none());
    }

    #[test]
    fn a_board_that_answers_on_the_loopback_is_found() {
        let board = UdpSocket::bind("127.0.0.1:0").expect("bind");
        let find = board.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            if let Ok((_, from)) = board.recv_from(&mut buf) {
                let _ = board.send_to(&answer("cafe", "E310  v2"), from);
            }
        });
        let found = ask(
            &["127.0.0.1".to_string()],
            Ports {
                control: find.wrapping_add(100),
            },
            Duration::from_secs(2),
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].serial.as_deref(), Some("CAFE"));
    }
}
