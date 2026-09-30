use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use futures::future::BoxFuture;
use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, server::TlsStream};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{handshake::derive_accept_key, protocol::Role},
};

pub type StubSocket = WebSocketStream<TlsStream<TcpStream>>;

const MAX_HEAD: usize = 64 * 1024;
const SUBPROTOCOL: &str = "sdrmm";

pub struct StubIdentity {
    pub cert: CertificateDer<'static>,
    pub key: PrivatePkcs8KeyDer<'static>,
    pub pin: String,
}

impl StubIdentity {
    pub fn generate() -> Self {
        let certified = rcgen::generate_simple_self_signed(vec![
            "localhost".to_owned(),
            "127.0.0.1".to_owned(),
        ])
        .expect("certificate");
        let cert = certified.cert.der().clone();
        let key = PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der());
        let pin = sdrmm_wire::phone::spki_pin(&cert).expect("pin");
        Self { cert, key, pin }
    }
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn server_config(identity: &StubIdentity) -> Arc<ServerConfig> {
    let config = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .expect("protocols")
        .with_no_client_auth()
        .with_single_cert(
            vec![identity.cert.clone()],
            PrivateKeyDer::Pkcs8(identity.key.clone_key()),
        )
        .expect("server certificate");
    Arc::new(config)
}

#[derive(Debug)]
struct FixedKey(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedKey {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

pub fn server_config_with_key(identity: &StubIdentity, key: &StubIdentity) -> Arc<ServerConfig> {
    let provider = provider();
    let signer = provider
        .key_provider
        .load_private_key(PrivateKeyDer::Pkcs8(key.key.clone_key()))
        .expect("signing key");
    let certified = CertifiedKey::new(vec![identity.cert.clone()], signer);
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocols")
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(FixedKey(Arc::new(certified))));
    Arc::new(config)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StubRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl StubRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    pub fn path(&self) -> &str {
        self.target
            .split_once('?')
            .map_or(self.target.as_str(), |(path, _)| path)
    }

    pub fn is_upgrade(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
    }

    fn content_length(&self) -> usize {
        self.header("content-length")
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StubResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
}

impl StubResponse {
    pub fn status(status: u16) -> Self {
        Self {
            status,
            ..Self::default()
        }
    }

    pub fn json(status: u16, value: &impl serde::Serialize) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), "application/json".to_owned())],
            body: serde_json::to_vec(value).expect("json"),
            delay: Duration::ZERO,
        }
    }

    pub fn error(status: u16, message: &str) -> Self {
        Self::json(status, &serde_json::json!({ "error": message }))
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

pub trait StubHandler: Send + Sync + 'static {
    fn http(&self, request: &StubRequest) -> StubResponse;

    fn upgrade(&self, _request: &StubRequest) -> Result<(), StubResponse> {
        Ok(())
    }

    fn socket(&self, _request: StubRequest, _socket: StubSocket) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

impl<F> StubHandler for F
where
    F: Fn(&StubRequest) -> StubResponse + Send + Sync + 'static,
{
    fn http(&self, request: &StubRequest) -> StubResponse {
        self(request)
    }
}

pub struct StubServer {
    pub addr: SocketAddr,
    pub pin: String,
    requests: Arc<Mutex<Vec<StubRequest>>>,
    handshakes: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl StubServer {
    pub async fn start(handler: impl StubHandler) -> Self {
        let identity = StubIdentity::generate();
        let config = server_config(&identity);
        Self::serve(identity.pin, config, Arc::new(handler)).await
    }

    pub async fn serve(
        pin: String,
        config: Arc<ServerConfig>,
        handler: Arc<dyn StubHandler>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let handshakes = Arc::new(AtomicUsize::new(0));
        let task = tokio::spawn(accept_loop(
            listener,
            TlsAcceptor::from(config),
            handler,
            requests.clone(),
            handshakes.clone(),
        ));
        Self {
            addr,
            pin,
            requests,
            handshakes,
            task,
        }
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.addr.port())
    }

    pub fn requests(&self) -> Vec<StubRequest> {
        lock(&self.requests).clone()
    }

    pub fn handshakes(&self) -> usize {
        self.handshakes.load(Ordering::SeqCst)
    }
}

impl Drop for StubServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    handler: Arc<dyn StubHandler>,
    requests: Arc<Mutex<Vec<StubRequest>>>,
    handshakes: Arc<AtomicUsize>,
) {
    while let Ok((tcp, _)) = listener.accept().await {
        let acceptor = acceptor.clone();
        let handler = handler.clone();
        let requests = requests.clone();
        let handshakes = handshakes.clone();
        tokio::spawn(async move {
            let Ok(Ok(stream)) =
                tokio::time::timeout(Duration::from_secs(5), acceptor.accept(tcp)).await
            else {
                return;
            };
            handshakes.fetch_add(1, Ordering::SeqCst);
            serve_one(stream, handler, requests).await;
        });
    }
}

async fn serve_one(
    mut stream: TlsStream<TcpStream>,
    handler: Arc<dyn StubHandler>,
    requests: Arc<Mutex<Vec<StubRequest>>>,
) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    lock(&requests).push(request.clone());
    if request.is_upgrade() {
        match handler.upgrade(&request) {
            Ok(()) => {
                if write_switch(&mut stream, &request).await.is_ok() {
                    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
                    handler.socket(request, socket).await;
                }
            }
            Err(response) => {
                let _ = write_response(&mut stream, &response).await;
            }
        }
        return;
    }
    let response = handler.http(&request);
    if !response.delay.is_zero() {
        tokio::time::sleep(response.delay).await;
    }
    let _ = write_response(&mut stream, &response).await;
}

async fn read_request(stream: &mut TlsStream<TcpStream>) -> Option<StubRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > MAX_HEAD {
            return None;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let mut request = parse_head(std::str::from_utf8(&buffer[..head_end]).ok()?)?;
    let mut body = buffer[head_end + 4..].to_vec();
    while body.len() < request.content_length() {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    request.body = body;
    Some(request)
}

fn parse_head(head: &str) -> Option<StubRequest> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
        .collect();
    Some(StubRequest {
        method,
        target,
        headers,
        body: Vec::new(),
    })
}

async fn write_switch(
    stream: &mut TlsStream<TcpStream>,
    request: &StubRequest,
) -> std::io::Result<()> {
    let key = request.header("sec-websocket-key").unwrap_or_default();
    let mut head = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n",
        derive_accept_key(key.as_bytes())
    );
    let offered = request
        .header("sec-websocket-protocol")
        .is_some_and(|list| list.split(',').any(|entry| entry.trim() == SUBPROTOCOL));
    if offered {
        head.push_str(&format!("Sec-WebSocket-Protocol: {SUBPROTOCOL}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.flush().await
}

async fn write_response(
    stream: &mut TlsStream<TcpStream>,
    response: &StubResponse,
) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} Stub\r\nContent-Length: {}\r\nConnection: close\r\n",
        response.status,
        response.body.len()
    );
    for (name, value) in &response.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(&response.body).await?;
    stream.flush().await?;
    stream.shutdown().await
}
