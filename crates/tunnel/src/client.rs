use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Duration,
};

use axum::Router;
use bytes::Bytes;
use futures::{SinkExt as _, StreamExt as _};
use rustls::{ClientConfig, pki_types::ServerName};
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    sync::{mpsc, watch},
    task::{AbortHandle, JoinError, JoinHandle, JoinSet},
    time::Instant,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, ClientRequestBuilder, Message, http::Uri, protocol::CloseFrame},
};

use crate::{
    frame::{CONTROL_STREAM, Frame, FrameError, MAX_HEALTH, PROTOCOL, RequestHead, WINDOW},
    identity::{DeviceKey, IdentityError},
    stream::{self, Inbound, Link},
    window::Window,
};

pub const PING: &str = "ping";
pub const PONG: &str = "pong";

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const KEEPALIVE: Duration = Duration::from_secs(25);
const HEALTH_SPACING: Duration = Duration::from_secs(60);
const HEALTH_REFRESH: Duration = Duration::from_secs(270);
const SILENCE: Duration = Duration::from_secs(70);
const STABLE: Duration = Duration::from_secs(60);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);
const OUTBOX: usize = 256;
const INBOUND: usize = 256;
const REJECTED: std::ops::Range<u16> = 4000..4100;

#[derive(Clone, Debug)]
pub struct Config {
    pub url: url::Url,
    pub key: Arc<DeviceKey>,
    pub health: watch::Receiver<Option<Bytes>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Online,
    Retrying { error: String, delay: Duration },
    Rejected { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("relay address: {0}")]
    Address(String),
    #[error("connect: {0}")]
    Connect(String),
    #[error("TLS: {0}")]
    Tls(String),
    #[error("relay socket: {0}")]
    Socket(String),
    #[error("relay rejected this device: {0}")]
    Rejected(String),
    #[error("relay broke the protocol: {0}")]
    Protocol(String),
    #[error("timed out: {0}")]
    Timeout(&'static str),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
}

#[derive(Debug)]
pub struct Tunnel {
    status: watch::Receiver<Status>,
    task: JoinHandle<()>,
}

impl Tunnel {
    pub fn spawn(config: Config, router: Router) -> Self {
        let (status_tx, status) = watch::channel(Status::Connecting);
        let task = tokio::spawn(run(config, router, status_tx));
        Self { status, task }
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(config: Config, router: Router, status: watch::Sender<Status>) {
    let mut attempt = 0u32;
    loop {
        status.send_replace(Status::Connecting);
        let started = Instant::now();
        let error = match session(&config, &router, &status).await {
            Err(TunnelError::Rejected(reason)) => {
                tracing::warn!(%reason, "relay rejected this device; remote access stopped");
                status.send_replace(Status::Rejected { reason });
                return;
            }
            Err(error) => error.to_string(),
            Ok(()) => "relay closed the connection".to_string(),
        };
        if started.elapsed() >= STABLE {
            attempt = 0;
        }
        let delay = retry_delay(attempt);
        attempt = attempt.saturating_add(1);
        tracing::info!(%error, ?delay, "remote access offline");
        status.send_replace(Status::Retrying { error, delay });
        tokio::time::sleep(delay).await;
    }
}

fn retry_delay(attempt: u32) -> Duration {
    let ceiling = FIRST_RETRY
        .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
        .min(LAST_RETRY);
    let mut noise = [0u8; 4];
    if aws_lc_rs::rand::fill(&mut noise).is_err() {
        return ceiling;
    }
    let fraction = f64::from(u32::from_be_bytes(noise)) / f64::from(u32::MAX);
    ceiling.mul_f64(0.5 + fraction / 2.0)
}

async fn session(
    config: &Config,
    router: &Router,
    status: &watch::Sender<Status>,
) -> Result<(), TunnelError> {
    let mut socket = connect(&config.url).await?;
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut socket, &config.key))
        .await
        .map_err(|_| TunnelError::Timeout("relay handshake"))??;
    status.send_replace(Status::Online);
    tracing::info!(relay = %config.url, "remote access online");
    serve(
        socket,
        router.clone(),
        Heartbeat::new(config.health.clone()),
    )
    .await
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Socket = WebSocketStream<Box<dyn Io>>;

async fn connect(url: &url::Url) -> Result<Socket, TunnelError> {
    let host = url
        .host_str()
        .ok_or_else(|| TunnelError::Address(format!("{url} has no host")))?;
    let port = url
        .port_or_known_default()
        .ok_or_else(|| TunnelError::Address(format!("{url} has no port")))?;
    let tcp = TcpStream::connect((host, port))
        .await
        .map_err(|error| TunnelError::Connect(format!("{host}:{port}: {error}")))?;
    tcp.set_nodelay(true)
        .map_err(|error| TunnelError::Connect(error.to_string()))?;
    let io: Box<dyn Io> = match url.scheme() {
        "wss" => Box::new(tls(host, tcp).await?),
        "ws" => Box::new(tcp),
        other => return Err(TunnelError::Address(format!("unsupported scheme {other}"))),
    };
    let uri: Uri = url
        .as_str()
        .parse()
        .map_err(|error| TunnelError::Address(format!("{url}: {error}")))?;
    let request = ClientRequestBuilder::new(uri).with_sub_protocol(PROTOCOL);
    let (socket, _) = tokio_tungstenite::client_async(request, io)
        .await
        .map_err(upgrade_error)?;
    Ok(socket)
}

fn upgrade_error(error: tungstenite::Error) -> TunnelError {
    match error {
        tungstenite::Error::Http(response)
            if matches!(response.status().as_u16(), 401 | 403 | 404 | 410) =>
        {
            TunnelError::Rejected(format!("HTTP {}", response.status()))
        }
        other => TunnelError::Socket(other.to_string()),
    }
}

async fn tls(
    host: &str,
    tcp: TcpStream,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, TunnelError> {
    let name = ServerName::try_from(host.to_string())
        .map_err(|error| TunnelError::Address(format!("{host}: {error}")))?;
    tokio_rustls::TlsConnector::from(platform_tls()?)
        .connect(name, tcp)
        .await
        .map_err(|error| TunnelError::Tls(error.to_string()))
}

fn platform_tls() -> Result<Arc<ClientConfig>, TunnelError> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .and_then(BuilderVerifierExt::with_platform_verifier)
            .map(|builder| Arc::new(builder.with_no_client_auth()))
            .map_err(|error| error.to_string())
        })
        .clone()
        .map_err(TunnelError::Tls)
}

async fn handshake(socket: &mut Socket, key: &DeviceKey) -> Result<(), TunnelError> {
    let Frame::Challenge { nonce } = next_frame(socket).await? else {
        return Err(TunnelError::Protocol("expected a challenge".to_string()));
    };
    let proof = Frame::Proof {
        signature: key.prove(&nonce)?,
    };
    socket
        .send(Message::Binary(proof.encode()?))
        .await
        .map_err(|error| TunnelError::Socket(error.to_string()))?;
    match next_frame(socket).await? {
        Frame::Ready => Ok(()),
        _ => Err(TunnelError::Protocol("expected ready".to_string())),
    }
}

async fn next_frame(socket: &mut Socket) -> Result<Frame, TunnelError> {
    loop {
        match socket.next().await {
            Some(Ok(Message::Binary(bytes))) => return Ok(Frame::decode(bytes)?),
            Some(Ok(Message::Close(frame))) => return Err(closed(frame)),
            Some(Ok(_)) => {}
            Some(Err(error)) => return Err(TunnelError::Socket(error.to_string())),
            None => return Err(TunnelError::Socket("closed during handshake".to_string())),
        }
    }
}

fn closed(frame: Option<CloseFrame>) -> TunnelError {
    match frame {
        Some(frame) if REJECTED.contains(&u16::from(frame.code)) => {
            TunnelError::Rejected(frame.reason.to_string())
        }
        Some(frame) => TunnelError::Socket(format!(
            "relay closed with {} {}",
            u16::from(frame.code),
            frame.reason
        )),
        None => TunnelError::Socket("relay closed".to_string()),
    }
}

struct AbortOnDrop(JoinHandle<Result<(), TunnelError>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn serve(
    socket: Socket,
    router: Router,
    mut heartbeat: Heartbeat,
) -> Result<(), TunnelError> {
    let (sink, mut source) = socket.split();
    let (out, outbox) = mpsc::channel(OUTBOX);
    let mut writer = AbortOnDrop(tokio::spawn(write(sink, outbox)));
    let mut streams = Streams::new(router, out.clone());
    let mut keepalive = tokio::time::interval_at(Instant::now() + KEEPALIVE, KEEPALIVE);
    let mut heard = Instant::now();
    heartbeat.beat(&out).await?;
    loop {
        tokio::select! {
            message = source.next() => {
                heard = Instant::now();
                match message {
                    Some(Ok(Message::Binary(bytes))) => streams.handle(Frame::decode(bytes)?).await?,
                    Some(Ok(Message::Text(text))) if text.as_str() == PONG => heartbeat.ponged(),
                    Some(Ok(Message::Close(frame))) => {
                        return match closed(frame) {
                            TunnelError::Rejected(reason) => Err(TunnelError::Rejected(reason)),
                            _ => Ok(()),
                        };
                    }
                    Some(Ok(_)) => {}
                    Some(Err(error)) => return Err(TunnelError::Socket(error.to_string())),
                    None => return Ok(()),
                }
            }
            Some(joined) = streams.tasks.join_next() => streams.finish(joined),
            _ = keepalive.tick() => {
                if heard.elapsed() > SILENCE {
                    return Err(TunnelError::Timeout("relay went silent"));
                }
                push(&out, Message::text(PING)).await?;
                heartbeat.pinged();
                heartbeat.beat(&out).await?;
            }
            written = &mut writer.0 => {
                return match written {
                    Ok(result) => result,
                    Err(error) => Err(TunnelError::Socket(error.to_string())),
                };
            }
        }
    }
}

async fn push(out: &mpsc::Sender<Message>, message: Message) -> Result<(), TunnelError> {
    out.send(message)
        .await
        .map_err(|_| TunnelError::Socket("writer stopped".to_string()))
}

struct Heartbeat {
    site: watch::Receiver<Option<Bytes>>,
    sent: Option<Instant>,
    sent_rtt: bool,
    ping: Option<Instant>,
    rtt: Option<Duration>,
    oversized: bool,
}

impl Heartbeat {
    fn new(site: watch::Receiver<Option<Bytes>>) -> Self {
        Self {
            site,
            sent: None,
            sent_rtt: false,
            ping: None,
            rtt: None,
            oversized: false,
        }
    }

    fn pinged(&mut self) {
        self.ping = Some(Instant::now());
    }

    fn ponged(&mut self) {
        if let Some(ping) = self.ping.take() {
            self.rtt = Some(ping.elapsed());
        }
    }

    fn due(&self) -> bool {
        let Some(sent) = self.sent else {
            return true;
        };
        let waited = sent.elapsed();
        let news = self.site.has_changed().unwrap_or(false) || self.rtt.is_some() != self.sent_rtt;
        waited >= HEALTH_REFRESH || (news && waited >= HEALTH_SPACING)
    }

    fn payload(&mut self) -> Option<Bytes> {
        if !self.due() {
            return None;
        }
        let site = self.site.borrow_and_update().clone()?;
        match envelope(self.rtt, &site) {
            Ok(payload) if payload.len() <= MAX_HEALTH => {
                self.oversized = false;
                Some(payload)
            }
            Ok(payload) => {
                if !std::mem::replace(&mut self.oversized, true) {
                    tracing::warn!(bytes = payload.len(), "health report too large; not sent");
                }
                None
            }
            Err(error) => {
                tracing::warn!(%error, "health report is not JSON; not sent");
                None
            }
        }
    }

    async fn beat(&mut self, out: &mpsc::Sender<Message>) -> Result<(), TunnelError> {
        let Some(data) = self.payload() else {
            return Ok(());
        };
        push(out, Message::Binary(Frame::Health { data }.encode()?)).await?;
        self.sent = Some(Instant::now());
        self.sent_rtt = self.rtt.is_some();
        Ok(())
    }
}

fn envelope(rtt: Option<Duration>, site: &[u8]) -> Result<Bytes, serde_json::Error> {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        rtt_ms: Option<u32>,
        site: &'a serde_json::value::RawValue,
    }
    let envelope = Envelope {
        rtt_ms: rtt.map(|rtt| u32::try_from(rtt.as_millis()).unwrap_or(u32::MAX)),
        site: serde_json::from_slice(site)?,
    };
    serde_json::to_vec(&envelope).map(Bytes::from)
}

async fn write<S>(mut sink: S, mut outbox: mpsc::Receiver<Message>) -> Result<(), TunnelError>
where
    S: futures::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    let failed = |error: tungstenite::Error| TunnelError::Socket(error.to_string());
    while let Some(message) = outbox.recv().await {
        sink.feed(message).await.map_err(failed)?;
        while let Ok(message) = outbox.try_recv() {
            sink.feed(message).await.map_err(failed)?;
        }
        sink.flush().await.map_err(failed)?;
    }
    Ok(())
}

struct Open {
    inbound: mpsc::Sender<Inbound>,
    window: Arc<Window>,
    task: AbortHandle,
}

struct Streams {
    router: Router,
    out: mpsc::Sender<Message>,
    open: HashMap<u32, Open>,
    tasks: JoinSet<u32>,
}

impl Streams {
    fn new(router: Router, out: mpsc::Sender<Message>) -> Self {
        Self {
            router,
            out,
            open: HashMap::new(),
            tasks: JoinSet::new(),
        }
    }

    async fn handle(&mut self, frame: Frame) -> Result<(), TunnelError> {
        match frame {
            Frame::Request { stream, head } => self.start(stream, head)?,
            Frame::Body { stream, data } => self.deliver(stream, Inbound::Body(data)).await,
            Frame::End { stream } => self.deliver(stream, Inbound::End).await,
            Frame::Text { stream, data } => self.deliver(stream, Inbound::Text(data)).await,
            Frame::Binary { stream, data } => self.deliver(stream, Inbound::Binary(data)).await,
            Frame::Close {
                stream,
                code,
                reason,
            } => self.deliver(stream, Inbound::Close(code, reason)).await,
            Frame::Credit { stream, bytes } => {
                if let Some(open) = self.open.get(&stream) {
                    open.window.grant(bytes);
                }
            }
            Frame::Reset { stream, reason } => {
                if let Some(open) = self.open.remove(&stream) {
                    open.task.abort();
                    tracing::debug!(stream, %reason, "relay reset a stream");
                }
            }
            Frame::Challenge { .. }
            | Frame::Proof { .. }
            | Frame::Ready
            | Frame::Response { .. }
            | Frame::Health { .. } => {
                return Err(TunnelError::Protocol(format!(
                    "unexpected frame on stream {}",
                    frame.stream()
                )));
            }
        }
        Ok(())
    }

    fn start(&mut self, stream: u32, head: RequestHead) -> Result<(), TunnelError> {
        if stream == CONTROL_STREAM || self.open.contains_key(&stream) {
            return Err(TunnelError::Protocol(format!(
                "stream {stream} opened twice"
            )));
        }
        let (inbound, receiver) = mpsc::channel(INBOUND);
        let window = Arc::new(Window::new(WINDOW));
        let link = Link {
            stream,
            out: self.out.clone(),
            window: window.clone(),
        };
        let router = self.router.clone();
        let task = self.tasks.spawn(async move {
            stream::run(head, receiver, link, router).await;
            stream
        });
        self.open.insert(
            stream,
            Open {
                inbound,
                window,
                task,
            },
        );
        Ok(())
    }

    async fn deliver(&mut self, stream: u32, item: Inbound) {
        let Some(open) = self.open.get(&stream) else {
            return;
        };
        if let Err(mpsc::error::TrySendError::Full(_)) = open.inbound.try_send(item) {
            tracing::warn!(stream, "relay overran the stream window; resetting it");
            if let Some(open) = self.open.remove(&stream) {
                open.task.abort();
            }
            let reset = Frame::Reset {
                stream,
                reason: "window overrun".to_string(),
            };
            if let Ok(bytes) = reset.encode()
                && self.out.send(Message::Binary(bytes)).await.is_err()
            {
                tracing::debug!(stream, "tunnel closed before the overrun reset was sent");
            }
        }
    }

    fn finish(&mut self, joined: Result<u32, JoinError>) {
        match joined {
            Ok(stream) => {
                self.open.remove(&stream);
            }
            Err(error) if error.is_panic() => {
                tracing::error!(%error, "relayed stream task panicked");
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_with_jitter_up_to_a_minute() {
        for attempt in 0..20 {
            let delay = retry_delay(attempt);
            let ceiling = FIRST_RETRY
                .saturating_mul(1u32.checked_shl(attempt).unwrap_or(u32::MAX))
                .min(LAST_RETRY);
            assert!(
                delay >= ceiling / 2 && delay <= ceiling,
                "{attempt}: {delay:?}"
            );
        }
        assert!(retry_delay(40) <= LAST_RETRY);
    }

    #[test]
    fn the_envelope_wraps_the_site_with_the_round_trip() {
        let site = br#"{"clients":1,"radios":[]}"#;
        assert_eq!(
            envelope(None, site).expect("envelope"),
            Bytes::from_static(br#"{"rtt_ms":null,"site":{"clients":1,"radios":[]}}"#)
        );
        assert_eq!(
            envelope(Some(Duration::from_micros(38_900)), site).expect("envelope"),
            Bytes::from_static(br#"{"rtt_ms":38,"site":{"clients":1,"radios":[]}}"#)
        );
        assert!(envelope(None, b"{").is_err());
    }

    async fn beat(heartbeat: &mut Heartbeat) -> Option<serde_json::Value> {
        let (out, mut sent) = mpsc::channel(1);
        heartbeat.beat(&out).await.expect("beat");
        let Ok(Message::Binary(bytes)) = sent.try_recv() else {
            return None;
        };
        let Frame::Health { data } = Frame::decode(bytes).expect("frame") else {
            panic!("expected health");
        };
        Some(serde_json::from_slice(&data).expect("json"))
    }

    #[tokio::test(start_paused = true)]
    async fn health_waits_for_a_site_then_paces_changes_and_refreshes() {
        let (site, receiver) = watch::channel(None);
        let mut heartbeat = Heartbeat::new(receiver);
        assert_eq!(beat(&mut heartbeat).await, None);
        site.send_replace(Some(Bytes::from_static(b"1")));
        assert_eq!(
            beat(&mut heartbeat).await,
            Some(serde_json::json!({"rtt_ms": null, "site": 1}))
        );
        site.send_replace(Some(Bytes::from_static(b"2")));
        tokio::time::advance(HEALTH_SPACING - Duration::from_secs(1)).await;
        assert_eq!(beat(&mut heartbeat).await, None);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            beat(&mut heartbeat).await,
            Some(serde_json::json!({"rtt_ms": null, "site": 2}))
        );
        tokio::time::advance(HEALTH_REFRESH - Duration::from_secs(1)).await;
        assert_eq!(beat(&mut heartbeat).await, None);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(beat(&mut heartbeat).await.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn the_first_round_trip_is_reported_within_a_minute() {
        let (_site, receiver) = watch::channel(Some(Bytes::from_static(b"{}")));
        let mut heartbeat = Heartbeat::new(receiver);
        assert!(beat(&mut heartbeat).await.is_some());
        heartbeat.pinged();
        tokio::time::advance(Duration::from_millis(38)).await;
        heartbeat.ponged();
        assert_eq!(beat(&mut heartbeat).await, None);
        tokio::time::advance(HEALTH_SPACING).await;
        assert_eq!(
            beat(&mut heartbeat).await,
            Some(serde_json::json!({"rtt_ms": 38, "site": {}}))
        );
        heartbeat.ponged();
        tokio::time::advance(HEALTH_SPACING).await;
        assert_eq!(beat(&mut heartbeat).await, None);
    }

    #[tokio::test(start_paused = true)]
    async fn an_oversized_report_is_not_sent() {
        let site = format!("\"{}\"", "x".repeat(MAX_HEALTH));
        let (_site, receiver) = watch::channel(Some(Bytes::from(site)));
        let mut heartbeat = Heartbeat::new(receiver);
        assert_eq!(beat(&mut heartbeat).await, None);
    }
}
