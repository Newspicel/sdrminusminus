use std::time::Duration;

use sdrmm_device::{
    DeviceError, StreamFailure,
    net::{CONNECT_TIMEOUT, Connection, Endpoint, Read},
};

use crate::iio::link::{Stopper, Transport};

#[derive(Debug)]
pub(crate) struct NetTransport {
    connection: Connection,
}

impl NetTransport {
    pub(crate) fn connect(endpoint: &Endpoint) -> Result<Self, DeviceError> {
        Self::connect_within(endpoint, CONNECT_TIMEOUT)
    }

    pub(crate) fn connect_within(
        endpoint: &Endpoint,
        timeout: Duration,
    ) -> Result<Self, DeviceError> {
        Ok(Self {
            connection: Connection::new(endpoint.connect_within(timeout)?),
        })
    }
}

impl Transport for NetTransport {
    fn send(&self, bytes: &[u8]) -> Result<(), DeviceError> {
        self.connection.send(bytes)
    }

    fn read(&self, buf: &mut [u8], _wanted: usize, timeout: Duration) -> Read {
        self.connection.read(buf, timeout)
    }

    fn fail(&self, reason: String) {
        self.connection.fail(reason);
    }

    fn failure(&self) -> StreamFailure {
        self.connection.failure()
    }

    fn close(&self) {
        self.connection.close();
    }

    fn stopper(&self) -> Stopper {
        Stopper::socket(self.connection.stop_handle())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read as _, Write as _},
        net::TcpListener,
    };

    use super::*;

    #[test]
    fn a_transport_carries_commands_and_answers_over_the_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut got = [0u8; 9];
            stream.read_exact(&mut got).expect("command");
            assert_eq!(&got, b"VERSION\r\n");
            stream.write_all(b"1.1.deadbee\n").expect("answer");
        });
        let endpoint = Endpoint::parse(&format!("127.0.0.1:{port}"), 30_431).expect("endpoint");
        let transport = NetTransport::connect(&endpoint).expect("connect");
        transport.send(b"VERSION\r\n").expect("send");
        let mut buf = [0u8; 32];
        let Read::Got(n) = transport.read(&mut buf, 1, Duration::from_secs(5)) else {
            panic!("the server answered");
        };
        assert_eq!(&buf[..n], b"1.1.deadbee\n");
        server.join().expect("server");
    }

    #[test]
    fn an_endpoint_nothing_listens_on_is_reported_rather_than_retried() {
        let endpoint = Endpoint::parse("127.0.0.1:1", 30_431).expect("endpoint");
        assert!(NetTransport::connect(&endpoint).is_err());
    }
}
