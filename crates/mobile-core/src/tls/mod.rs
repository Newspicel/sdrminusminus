use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use rustls::{ClientConfig, client::danger::ServerCertVerifier, crypto::CryptoProvider};

use crate::error::CoreError;

mod pinned;

pub(crate) use pinned::pin_mismatch_in;
use pinned::{PinnedVerifier, RecordingVerifier};

pub(crate) const REST_ALPN: [&[u8]; 2] = [b"h2", b"http/1.1"];
pub(crate) const WS_ALPN: [&[u8]; 1] = [b"http/1.1"];

#[derive(Clone, Debug, Default)]
pub(crate) struct Seen {
    pin: Arc<Mutex<Option<String>>>,
}

impl Seen {
    fn record(&self, pin: String) {
        *self.pin.lock().unwrap_or_else(PoisonError::into_inner) = Some(pin);
    }

    pub(crate) fn get(&self) -> Option<String> {
        self.pin
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

pub(crate) enum Trust {
    Pinned(Vec<String>),
    Recording(Seen),
}

#[derive(Clone, Debug)]
pub(crate) struct TlsConfigs {
    pub(crate) rest: Arc<ClientConfig>,
    pub(crate) ws: Arc<ClientConfig>,
}

pub(crate) fn provider() -> Arc<CryptoProvider> {
    static PROVIDER: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    PROVIDER
        .get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

pub(crate) fn configs(trust: Trust) -> Result<TlsConfigs, CoreError> {
    let provider = provider();
    let verifier: Arc<dyn ServerCertVerifier> = match trust {
        Trust::Pinned(pins) => Arc::new(PinnedVerifier::new(pins, provider.clone())),
        Trust::Recording(seen) => Arc::new(RecordingVerifier::new(seen, provider.clone())),
    };
    Ok(TlsConfigs {
        rest: config(&provider, &verifier, &REST_ALPN)?,
        ws: config(&provider, &verifier, &WS_ALPN)?,
    })
}

pub(crate) fn pinned(pin: &str) -> Result<TlsConfigs, CoreError> {
    configs(Trust::Pinned(vec![pin.to_owned()]))
}

fn config(
    provider: &Arc<CryptoProvider>,
    verifier: &Arc<dyn ServerCertVerifier>,
    alpn: &[&[u8]],
) -> Result<Arc<ClientConfig>, CoreError> {
    let mut config = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| CoreError::internal(format!("TLS setup failed: {error}")))?
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|protocol| protocol.to_vec()).collect();
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use std::io;

    use rustls::{ServerConfig, pki_types::ServerName};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    use super::*;
    use crate::stub_server::{StubIdentity, server_config, server_config_with_key};

    async fn handshake(
        server: Arc<ServerConfig>,
        client: Arc<ClientConfig>,
    ) -> (io::Result<()>, io::Result<()>) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let accept = async {
            let mut stream = TlsAcceptor::from(server).accept(server_io).await?;
            let mut byte = [0; 1];
            stream.read_exact(&mut byte).await?;
            stream.write_all(&byte).await?;
            stream.flush().await
        };
        let connect = async {
            let name = ServerName::try_from("localhost").map_err(io::Error::other)?;
            let mut stream = TlsConnector::from(client).connect(name, client_io).await?;
            stream.write_all(b"x").await?;
            stream.flush().await?;
            let mut byte = [0; 1];
            stream.read_exact(&mut byte).await?;
            Ok(())
        };
        tokio::join!(accept, connect)
    }

    #[test]
    fn the_provider_is_ring() {
        let ring = rustls::crypto::ring::default_provider();
        let ours = provider();
        assert_eq!(
            format!("{:?}", ours.cipher_suites),
            format!("{:?}", ring.cipher_suites)
        );
        assert!(Arc::ptr_eq(&ours, &provider()));
    }

    #[test]
    fn rest_offers_h2_and_http11_but_websocket_only_http11() {
        let configs = pinned(&StubIdentity::generate().pin).expect("configs");
        assert_eq!(
            configs.rest.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
        assert_eq!(configs.ws.alpn_protocols, vec![b"http/1.1".to_vec()]);
    }

    #[tokio::test]
    async fn a_handshake_with_the_pinned_key_succeeds() {
        let identity = StubIdentity::generate();
        let configs = pinned(&identity.pin).expect("configs");
        let (server, client) = handshake(server_config(&identity), configs.ws).await;
        server.expect("server side");
        client.expect("client side");
    }

    #[tokio::test]
    async fn a_pinned_certificate_served_with_another_key_fails_the_handshake() {
        let identity = StubIdentity::generate();
        let other = StubIdentity::generate();
        let configs = pinned(&identity.pin).expect("configs");
        let (_, client) = handshake(server_config_with_key(&identity, &other), configs.ws).await;
        let error = client.expect_err("the peer cannot prove the pinned key");
        assert!(pin_mismatch_in(&error).is_none());
    }

    #[tokio::test]
    async fn a_recording_handshake_keeps_the_key_it_saw() {
        let identity = StubIdentity::generate();
        let seen = Seen::default();
        let configs = configs(Trust::Recording(seen.clone())).expect("configs");
        let (server, client) = handshake(server_config(&identity), configs.ws).await;
        server.expect("server side");
        client.expect("client side");
        assert_eq!(seen.get(), Some(identity.pin));
    }

    #[tokio::test]
    async fn a_handshake_with_another_key_names_the_key_it_saw() {
        let identity = StubIdentity::generate();
        let pinned_key = StubIdentity::generate();
        let configs = pinned(&pinned_key.pin).expect("configs");
        let (_, client) = handshake(server_config(&identity), configs.ws).await;
        let error = client.expect_err("refused");
        assert_eq!(pin_mismatch_in(&error), Some(identity.pin));
    }
}
