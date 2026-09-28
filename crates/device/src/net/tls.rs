use std::{
    io::{ErrorKind, Read as _, Write as _},
    net::TcpStream,
    sync::{Arc, OnceLock},
};

use rustls::{ClientConfig, ClientConnection, pki_types::ServerName};
use rustls_platform_verifier::BuilderVerifierExt;

use crate::DeviceError;

pub(crate) fn platform() -> Result<Arc<ClientConfig>, DeviceError> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .and_then(BuilderVerifierExt::with_platform_verifier)
            .map(|builder| Arc::new(builder.with_no_client_auth()))
            .map_err(|e| e.to_string())
        })
        .clone()
        .map_err(|e| DeviceError::Io(format!("TLS setup: {e}")))
}

pub(crate) fn handshake(
    socket: &TcpStream,
    host: &str,
    config: Arc<ClientConfig>,
) -> Result<ClientConnection, DeviceError> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|e| DeviceError::NotFound(format!("{host}: {e}")))?;
    let mut session = ClientConnection::new(config, name)
        .map_err(|e| DeviceError::Io(format!("TLS with {host}: {e}")))?;
    let mut io = socket;
    while session.is_handshaking() {
        session
            .complete_io(&mut io)
            .map_err(|e| DeviceError::Io(format!("TLS with {host}: {e}")))?;
    }
    Ok(session)
}

pub(crate) enum Plain {
    Got(usize),
    Empty,
    Closed,
    Broken(String),
}

pub(crate) fn plaintext(session: &mut ClientConnection, buf: &mut [u8]) -> Plain {
    match session.reader().read(buf) {
        Ok(0) => Plain::Closed,
        Ok(n) => Plain::Got(n),
        Err(e) if e.kind() == ErrorKind::WouldBlock => Plain::Empty,
        Err(e) => Plain::Broken(format!("TLS: {e}")),
    }
}

pub(crate) fn absorb(
    session: &mut ClientConnection,
    socket: &TcpStream,
    mut raw: &[u8],
) -> Result<(), String> {
    while !raw.is_empty() {
        session
            .read_tls(&mut raw)
            .map_err(|e| format!("TLS: {e}"))?;
        session
            .process_new_packets()
            .map_err(|e| format!("TLS: {e}"))?;
    }
    flush(session, socket).map_err(|e| format!("TLS: {e}"))
}

pub(crate) fn seal(
    session: &mut ClientConnection,
    socket: &TcpStream,
    frame: &[u8],
) -> std::io::Result<()> {
    session.writer().write_all(frame)?;
    flush(session, socket)
}

pub(crate) fn flush(session: &mut ClientConnection, socket: &TcpStream) -> std::io::Result<()> {
    let mut io = socket;
    while session.wants_write() {
        session.write_tls(&mut io)?;
    }
    Ok(())
}
