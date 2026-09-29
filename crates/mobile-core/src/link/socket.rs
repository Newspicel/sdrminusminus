use std::{
    collections::{BTreeSet, HashSet},
    sync::Arc,
    time::Duration,
};

use futures::{SinkExt, StreamExt, future::BoxFuture};
use rustls::{ClientConfig, pki_types::ServerName};
use sdrmm_wire::{
    about::API_PROTOCOL,
    frame::{self, FrameKind},
    rest::ApiError,
    ws::{ClientCommand, ServerEvent, StateScope, WS_CLOSE_REVOKED, WS_SUBPROTOCOL},
};
use serde::Deserialize;
use tokio::{net::TcpStream, sync::oneshot, time::Instant};
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{
        self, Message,
        client::IntoClientRequest,
        http::{
            HeaderValue, Request,
            header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL},
        },
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};

use super::{DialError, Dialed, Dialer, Inbound, Session, Subscriptions, Wires, candidates};
use crate::{
    events::CoreEvent,
    link::rest::RestClient,
    records::Notice,
    tls::{self, pin_mismatch_in},
    vault::ServerRecord,
};

pub(crate) const TCP_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const TLS_TIMEOUT: Duration = Duration::from_secs(4);
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const PING_EVERY: Duration = Duration::from_secs(15);
pub(crate) const DEAD_AFTER: Duration = Duration::from_secs(45);
pub(crate) const POSE_GAP: Duration = Duration::from_millis(crate::pose::PUBLISH_GAP_MS);
const MAX_MESSAGE: usize = 8 << 20;
const NORMAL_CLOSE: u16 = 1000;

type Socket = WebSocketStream<TlsStream<TcpStream>>;

pub(crate) async fn connect_tls(
    host: &str,
    config: Arc<ClientConfig>,
) -> Result<TlsStream<TcpStream>, DialError> {
    let tcp = tokio::time::timeout(TCP_TIMEOUT, TcpStream::connect(host))
        .await
        .map_err(|_| DialError::TimedOut)?
        .map_err(|error| DialError::Unreachable(error.to_string()))?;
    if let Err(error) = tcp.set_nodelay(true) {
        tracing::debug!(%error, "no TCP_NODELAY");
    }
    let name = ServerName::try_from(candidates::host_name(host).to_owned())
        .map_err(|_| DialError::Unreachable(format!("bad host {host}")))?;
    tokio::time::timeout(TLS_TIMEOUT, TlsConnector::from(config).connect(name, tcp))
        .await
        .map_err(|_| DialError::TimedOut)?
        .map_err(|error| match pin_mismatch_in(&error) {
            Some(seen) => DialError::KeyMismatch { seen },
            None => DialError::Unreachable(error.to_string()),
        })
}

pub(crate) fn upgrade_request(host: &str, token: &str) -> Result<Request<()>, DialError> {
    let mut request = format!("wss://{host}/api/ws")
        .into_client_request()
        .map_err(|error| DialError::Unreachable(error.to_string()))?;
    let mut bearer = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| DialError::Unreachable("bad token".to_owned()))?;
    bearer.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, bearer);
    request.headers_mut().insert(
        SEC_WEBSOCKET_PROTOCOL,
        HeaderValue::from_static(WS_SUBPROTOCOL),
    );
    Ok(request)
}

fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE))
}

fn upgrade_error(error: tungstenite::Error) -> DialError {
    match error {
        tungstenite::Error::Http(response) => {
            let status = response.status().as_u16();
            if status == 401 {
                return DialError::Revoked;
            }
            let message = response
                .body()
                .as_deref()
                .and_then(|body| serde_json::from_slice::<ApiError>(body).ok())
                .map_or_else(|| "upgrade refused".to_owned(), |error| error.error);
            DialError::Server { status, message }
        }
        other => DialError::Unreachable(other.to_string()),
    }
}

async fn wait_hello(socket: &mut Socket) -> Result<u32, DialError> {
    while let Some(message) = socket.next().await {
        match message.map_err(|error| DialError::Unreachable(error.to_string()))? {
            Message::Text(text) => {
                return match serde_json::from_str::<ServerEvent>(&text) {
                    Ok(ServerEvent::Hello { protocol, .. }) => Ok(protocol),
                    _ => Err(DialError::Server {
                        status: 101,
                        message: "no hello".to_owned(),
                    }),
                };
            }
            Message::Close(frame) => return Err(closed(frame.as_ref())),
            Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    Err(DialError::Closed("closed before hello".to_owned()))
}

fn closed(frame: Option<&CloseFrame>) -> DialError {
    match frame {
        Some(frame) if u16::from(frame.code) == WS_CLOSE_REVOKED => DialError::Revoked,
        Some(frame) => DialError::Closed(format!("{} {}", u16::from(frame.code), frame.reason)),
        None => DialError::Closed("closed".to_owned()),
    }
}

pub(crate) struct TlsDialer {
    wires: Wires,
}

impl TlsDialer {
    pub(crate) fn new(wires: Wires) -> Self {
        Self { wires }
    }
}

impl Dialer for TlsDialer {
    fn dial(
        &self,
        record: Arc<ServerRecord>,
        host: String,
    ) -> BoxFuture<'static, Result<Dialed, DialError>> {
        let wires = self.wires.clone();
        Box::pin(async move {
            let result = dial(&record, &host, &wires).await;
            result.map_err(|error| wires.net.explain(&host, error))
        })
    }
}

async fn dial(record: &ServerRecord, host: &str, wires: &Wires) -> Result<Dialed, DialError> {
    let tls =
        tls::pinned(&record.pin).map_err(|error| DialError::Unreachable(error.to_string()))?;
    let stream = connect_tls(host, tls.ws.clone()).await?;
    let rest = RestClient::new(host, &tls, Some(&record.token))
        .map_err(|error| DialError::Unreachable(error.to_string()))?;
    let about = rest.about().await.map_err(DialError::from_rest)?;
    if about.protocol != API_PROTOCOL {
        return Err(DialError::Protocol {
            server: about.protocol,
        });
    }
    let request = upgrade_request(host, &record.token)?;
    let (mut socket, _) = tokio::time::timeout(
        UPGRADE_TIMEOUT,
        client_async_with_config(request, stream, Some(ws_config())),
    )
    .await
    .map_err(|_| DialError::TimedOut)?
    .map_err(upgrade_error)?;
    let protocol = tokio::time::timeout(HELLO_TIMEOUT, wait_hello(&mut socket))
        .await
        .map_err(|_| DialError::TimedOut)??;
    if protocol != API_PROTOCOL {
        return Err(DialError::Protocol { server: protocol });
    }
    let (stop, stopped) = oneshot::channel();
    let session = Arc::new(Session::new(
        about.server_name,
        record.phone_id.clone(),
        host.to_owned(),
        Arc::new(rest),
    ));
    let connection = Connection::new(socket, stopped, wires.clone());
    Ok(Dialed {
        session,
        run: Box::pin(connection.run()),
        stop,
    })
}

#[derive(Deserialize)]
struct Tag {
    #[serde(rename = "type")]
    kind: String,
}

pub(crate) struct Connection<S> {
    socket: WebSocketStream<S>,
    stopped: oneshot::Receiver<()>,
    wires: Wires,
    sent: Subscriptions,
    refused: BTreeSet<String>,
    retry_refused: bool,
    unknown: HashSet<String>,
    odd_frames: HashSet<Option<u8>>,
    started: Instant,
    last_rx: Instant,
    last_pose: Option<Instant>,
    pose_waiting: bool,
    subs_open: bool,
    pose_open: bool,
}

impl<S> Connection<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send,
{
    pub(crate) fn new(
        socket: WebSocketStream<S>,
        stopped: oneshot::Receiver<()>,
        wires: Wires,
    ) -> Self {
        let now = Instant::now();
        Self {
            socket,
            stopped,
            wires,
            sent: Subscriptions::new(),
            refused: BTreeSet::new(),
            retry_refused: false,
            unknown: HashSet::new(),
            odd_frames: HashSet::new(),
            started: now,
            last_rx: now,
            last_pose: None,
            pose_waiting: true,
            subs_open: true,
            pose_open: true,
        }
    }

    pub(crate) async fn run(mut self) -> DialError {
        match self.pump().await {
            Ok(()) => {
                let frame = CloseFrame {
                    code: CloseCode::from(NORMAL_CLOSE),
                    reason: "bye".into(),
                };
                if let Err(error) = self.socket.close(Some(frame)).await {
                    tracing::debug!(%error, "close failed");
                }
                DialError::Closed("stopped".to_owned())
            }
            Err(error) => error,
        }
    }

    async fn pump(&mut self) -> Result<(), DialError> {
        self.sync_subscriptions().await?;
        self.flush_pose().await?;
        let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let pose_at = self.pose_ready_at();
            tokio::select! {
                message = self.socket.next() => {
                    self.last_rx = Instant::now();
                    self.receive(message)?;
                    self.resubscribe_refused().await?;
                }
                _ = &mut self.stopped => return Ok(()),
                changed = self.wires.subs.changed(), if self.subs_open => {
                    if changed.is_err() {
                        self.subs_open = false;
                    } else {
                        self.sync_subscriptions().await?;
                    }
                }
                changed = self.wires.pose.changed(), if self.pose_open => {
                    if changed.is_err() {
                        self.pose_open = false;
                    } else {
                        self.pose_waiting = true;
                        self.flush_pose().await?;
                    }
                }
                () = tokio::time::sleep_until(pose_at), if self.pose_waiting => {
                    self.flush_pose().await?;
                }
                _ = ping.tick() => self.ping().await?,
            }
        }
    }

    fn pose_ready_at(&self) -> Instant {
        self.last_pose
            .map_or_else(Instant::now, |last| last + POSE_GAP)
    }

    async fn send(&mut self, command: &ClientCommand) -> Result<(), DialError> {
        let text = serde_json::to_string(command)
            .map_err(|error| DialError::Closed(format!("encode failed: {error}")))?;
        self.socket
            .send(Message::text(text))
            .await
            .map_err(|error| DialError::Unreachable(error.to_string()))
    }

    async fn sync_subscriptions(&mut self) -> Result<(), DialError> {
        let wanted = self.wires.subs.borrow_and_update().clone();
        let gone: Vec<String> = self
            .sent
            .keys()
            .filter(|node| !wanted.contains_key(*node))
            .cloned()
            .collect();
        for node in gone {
            self.send(&ClientCommand::UnsubscribeSurface { node })
                .await?;
        }
        for (node, fit) in &wanted {
            if self.sent.get(node) != Some(fit) {
                let command = ClientCommand::SubscribeSurface {
                    node: node.clone(),
                    fit: *fit,
                };
                self.send(&command).await?;
            }
        }
        let sent = &self.sent;
        self.refused.retain(|node| {
            sent.get(node)
                .is_some_and(|fit| wanted.get(node) == Some(fit))
        });
        self.sent = wanted;
        Ok(())
    }

    async fn resubscribe_refused(&mut self) -> Result<(), DialError> {
        if !std::mem::take(&mut self.retry_refused) {
            return Ok(());
        }
        for node in std::mem::take(&mut self.refused) {
            if let Some(fit) = self.sent.get(&node).copied() {
                self.send(&ClientCommand::SubscribeSurface { node, fit })
                    .await?;
            }
        }
        Ok(())
    }

    fn watch_surfaces(&mut self, event: &ServerEvent) {
        match event {
            ServerEvent::SurfaceRefused { node, .. } if self.sent.contains_key(node) => {
                self.refused.insert(node.clone());
            }
            ServerEvent::SurfaceStreamStarted { node, .. } => {
                self.refused.remove(node);
            }
            ServerEvent::StateChanged {
                scope: StateScope::Workspaces | StateScope::Missions | StateScope::All,
            } => self.retry_refused |= !self.refused.is_empty(),
            _ => {}
        }
    }

    async fn flush_pose(&mut self) -> Result<(), DialError> {
        let now = Instant::now();
        if self.last_pose.is_some_and(|last| now < last + POSE_GAP) {
            return Ok(());
        }
        self.pose_waiting = false;
        let pose = self.wires.pose.borrow_and_update().clone();
        if let Some(pose) = pose {
            self.last_pose = Some(now);
            self.send(&pose.command()).await?;
        }
        Ok(())
    }

    async fn ping(&mut self) -> Result<(), DialError> {
        if self.last_rx.elapsed() > DEAD_AFTER {
            return Err(DialError::TimedOut);
        }
        let stamp = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.socket
            .send(Message::Ping(stamp.to_be_bytes().to_vec().into()))
            .await
            .map_err(|error| DialError::Unreachable(error.to_string()))
    }

    fn receive(
        &mut self,
        message: Option<Result<Message, tungstenite::Error>>,
    ) -> Result<(), DialError> {
        match message {
            None => Err(DialError::Closed("closed".to_owned())),
            Some(Err(error)) => Err(DialError::Unreachable(error.to_string())),
            Some(Ok(Message::Close(frame))) => Err(closed(frame.as_ref())),
            Some(Ok(Message::Text(text))) => {
                self.text(&text);
                Ok(())
            }
            Some(Ok(Message::Binary(bytes))) => {
                self.binary(bytes.to_vec());
                Ok(())
            }
            Some(Ok(Message::Pong(payload))) => {
                self.pong(&payload);
                Ok(())
            }
            Some(Ok(Message::Ping(_) | Message::Frame(_))) => Ok(()),
        }
    }

    fn text(&mut self, text: &str) {
        match serde_json::from_str::<ServerEvent>(text) {
            Ok(event) => {
                self.watch_surfaces(&event);
                self.wires.forward(Inbound::Event(Box::new(event)));
            }
            Err(error) => {
                let kind = serde_json::from_str::<Tag>(text)
                    .map_or_else(|_| "?".to_owned(), |tag| tag.kind);
                tracing::debug!(%kind, %error, "event not understood");
                if self.unknown.insert(kind.clone()) {
                    self.notice(format!("Unknown event {kind}"));
                }
            }
        }
    }

    fn binary(&mut self, bytes: Vec<u8>) {
        let kind = frame::peek_header(&bytes).ok().map(|header| header.kind);
        match kind {
            Some(FrameKind::RangeDoppler | FrameKind::FusionGrid) => {
                self.wires.forward(Inbound::Frame(bytes));
            }
            other => {
                let key = other.map(|kind| kind as u8);
                if self.odd_frames.insert(key) {
                    self.notice(match other {
                        Some(kind) => format!("Unexpected frame {kind:?}"),
                        None => "Bad frame".to_owned(),
                    });
                }
            }
        }
    }

    fn pong(&self, payload: &[u8]) {
        let Ok(stamp) = <[u8; 8]>::try_from(payload) else {
            return;
        };
        let sent = u64::from_be_bytes(stamp);
        let now = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::debug!(rtt_ms = now.saturating_sub(sent), "pong");
    }

    fn notice(&self, text: String) {
        self.wires.events.emit(CoreEvent::Notice {
            notice: Notice::warn(text),
        });
    }
}
