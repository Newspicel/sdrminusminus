use std::{convert::Infallible, sync::Arc};

use axum::Router;
use bytes::Bytes;
use futures::{Sink, SinkExt as _, Stream, StreamExt as _};
use http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Empty, StreamBody, combinators::BoxBody};
use hyper::{
    body::{Frame as BodyFrame, Incoming},
    client::conn::http1::SendRequest,
};
use hyper_util::{rt::TokioIo, service::TowerToHyperService};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        self, Message, Utf8Bytes,
        handshake::client::generate_key,
        protocol::{CloseFrame, Role, frame::coding::CloseCode},
    },
};

use crate::{
    Relayed,
    frame::{Frame, FrameError, RequestHead, ResponseHead},
    window::Window,
};

pub(crate) const CHUNK: usize = 64 * 1024;
const DUPLEX: usize = 256 * 1024;
const UPLOAD_QUEUE: usize = 4;
const LOCAL_HOST: &str = "tunnel";
const NO_STATUS: u16 = 1005;
const ABNORMAL: u16 = 1006;
const TLS_FAILURE: u16 = 1015;

const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
];
const CREDENTIALS: &[&str] = &["authorization", "cookie"];
const HANDSHAKE: &[&str] = &[
    "sec-websocket-key",
    "sec-websocket-version",
    "sec-websocket-extensions",
    "sec-websocket-accept",
];

type LocalBody = BoxBody<Bytes, Infallible>;

#[derive(Debug)]
pub(crate) enum Inbound {
    Body(Bytes),
    End,
    Text(Bytes),
    Binary(Bytes),
    Close(u16, String),
}

#[derive(Debug, thiserror::Error)]
enum StreamError {
    #[error("tunnel closed")]
    Closed,
    #[error("request head: {0}")]
    Head(String),
    #[error("local server: {0}")]
    Local(String),
    #[error("relay sent {0} on this stream")]
    Unexpected(&'static str),
    #[error(transparent)]
    Frame(#[from] FrameError),
}

#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub(crate) stream: u32,
    pub(crate) out: mpsc::Sender<Message>,
    pub(crate) window: Arc<Window>,
}

impl Link {
    async fn send(&self, frame: Frame) -> Result<(), StreamError> {
        let bytes = frame.encode()?;
        self.out
            .send(Message::Binary(bytes))
            .await
            .map_err(|_| StreamError::Closed)
    }

    async fn send_counted(&self, frame: Frame) -> Result<(), StreamError> {
        self.window.spend(frame.credited_len()).await;
        self.send(frame).await
    }

    async fn credit(&self, bytes: usize) -> Result<(), StreamError> {
        self.send(Frame::Credit {
            stream: self.stream,
            bytes: u32::try_from(bytes).unwrap_or(u32::MAX),
        })
        .await
    }

    async fn send_chunks(&self, mut data: Bytes) -> Result<(), StreamError> {
        while !data.is_empty() {
            let chunk = data.split_to(data.len().min(CHUNK));
            self.send_counted(Frame::Body {
                stream: self.stream,
                data: chunk,
            })
            .await?;
        }
        Ok(())
    }
}

pub(crate) async fn run(
    head: RequestHead,
    inbound: mpsc::Receiver<Inbound>,
    link: Link,
    router: Router,
) {
    let stream = link.stream;
    let Err(error) = exchange(head, inbound, &link, router).await else {
        return;
    };
    tracing::debug!(stream, %error, "relayed stream failed");
    let reset = Frame::Reset {
        stream,
        reason: error.to_string(),
    };
    if link.send(reset).await.is_err() {
        tracing::debug!(stream, "tunnel closed before the stream reset was sent");
    }
}

async fn exchange(
    head: RequestHead,
    inbound: mpsc::Receiver<Inbound>,
    link: &Link,
    router: Router,
) -> Result<(), StreamError> {
    let relayed = Relayed {
        user: head.user.clone(),
    };
    let sender = connect(router, relayed).await?;
    if head.websocket {
        websocket(&head, inbound, link, sender).await
    } else {
        http(&head, inbound, link, sender).await
    }
}

async fn connect(router: Router, relayed: Relayed) -> Result<SendRequest<LocalBody>, StreamError> {
    let (client, server) = tokio::io::duplex(DUPLEX);
    let service = TowerToHyperService::new(tower::util::MapRequest::new(
        router,
        move |mut request: Request<Incoming>| {
            request.extensions_mut().insert(relayed.clone());
            request
        },
    ));
    tokio::spawn(async move {
        let served = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(server), service)
            .with_upgrades()
            .await;
        if let Err(error) = served {
            tracing::debug!(%error, "relayed connection ended");
        }
    });
    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(client))
        .await
        .map_err(local)?;
    tokio::spawn(async move {
        if let Err(error) = connection.with_upgrades().await {
            tracing::debug!(%error, "relayed client connection ended");
        }
    });
    Ok(sender)
}

async fn http(
    head: &RequestHead,
    inbound: mpsc::Receiver<Inbound>,
    link: &Link,
    mut sender: SendRequest<LocalBody>,
) -> Result<(), StreamError> {
    let (body_tx, body_rx) = mpsc::channel::<Bytes>(UPLOAD_QUEUE);
    let body = StreamBody::new(futures::stream::unfold(body_rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|data| (Ok::<_, Infallible>(BodyFrame::data(data)), rx))
    }));
    let request = local_request(head)?
        .body(BodyExt::boxed(body))
        .map_err(|error| StreamError::Head(error.to_string()))?;
    let upload = upload(inbound, body_tx, link);
    let download = async {
        let response = sender.send_request(request).await.map_err(local)?;
        relay_response(response, link).await
    };
    tokio::pin!(upload, download);
    tokio::select! {
        result = &mut download => return result,
        result = &mut upload => result?,
    }
    download.await
}

async fn upload(
    mut inbound: mpsc::Receiver<Inbound>,
    body: mpsc::Sender<Bytes>,
    link: &Link,
) -> Result<(), StreamError> {
    while let Some(item) = inbound.recv().await {
        match item {
            Inbound::Body(data) => {
                let len = data.len();
                if body.send(data).await.is_err() {
                    return discard_until_end(inbound).await;
                }
                link.credit(len).await?;
            }
            Inbound::End => return Ok(()),
            Inbound::Text(_) | Inbound::Binary(_) | Inbound::Close(..) => {
                return Err(StreamError::Unexpected("a WebSocket message"));
            }
        }
    }
    Ok(())
}

async fn discard_until_end(mut inbound: mpsc::Receiver<Inbound>) -> Result<(), StreamError> {
    while let Some(item) = inbound.recv().await {
        if matches!(item, Inbound::End) {
            break;
        }
    }
    Ok(())
}

async fn relay_response(response: Response<Incoming>, link: &Link) -> Result<(), StreamError> {
    let (parts, mut body) = response.into_parts();
    link.send(Frame::Response {
        stream: link.stream,
        head: response_head(parts.status, &parts.headers),
    })
    .await?;
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame.map_err(local)?.into_data() {
            link.send_chunks(data).await?;
        }
    }
    link.send(Frame::End {
        stream: link.stream,
    })
    .await
}

async fn websocket(
    head: &RequestHead,
    inbound: mpsc::Receiver<Inbound>,
    link: &Link,
    mut sender: SendRequest<LocalBody>,
) -> Result<(), StreamError> {
    let request = upgrade_request(head)?
        .body(BodyExt::boxed(Empty::new()))
        .map_err(|error| StreamError::Head(error.to_string()))?;
    let response = sender.send_request(request).await.map_err(local)?;
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return relay_response(response, link).await;
    }
    let accepted = response_head(response.status(), response.headers());
    let upgraded = hyper::upgrade::on(response).await.map_err(local)?;
    link.send(Frame::Response {
        stream: link.stream,
        head: accepted,
    })
    .await?;
    let socket = WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Client, None).await;
    let (sink, source) = socket.split();
    let to_relay = to_relay(source, link);
    let to_local = to_local(inbound, sink, link);
    tokio::pin!(to_relay, to_local);
    tokio::select! {
        result = &mut to_relay => result,
        result = &mut to_local => {
            let Some((code, reason)) = result? else {
                return to_relay.await;
            };
            if to_relay.await.is_err() {
                link.send(Frame::Close { stream: link.stream, code, reason }).await?;
            }
            Ok(())
        }
    }
}

async fn to_relay<S>(mut source: S, link: &Link) -> Result<(), StreamError>
where
    S: Stream<Item = Result<Message, tungstenite::Error>> + Unpin,
{
    let stream = link.stream;
    while let Some(message) = source.next().await {
        match message.map_err(local)? {
            Message::Text(text) => {
                link.send_counted(Frame::Text {
                    stream,
                    data: Bytes::from(text),
                })
                .await?;
            }
            Message::Binary(data) => link.send_counted(Frame::Binary { stream, data }).await?,
            Message::Close(frame) => {
                let (code, reason) = frame.map_or((NO_STATUS, String::new()), |frame| {
                    (u16::from(frame.code), frame.reason.to_string())
                });
                return link
                    .send(Frame::Close {
                        stream,
                        code,
                        reason,
                    })
                    .await;
            }
            Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
        }
    }
    link.send(Frame::Close {
        stream,
        code: ABNORMAL,
        reason: "local socket ended".to_string(),
    })
    .await
}

async fn to_local<S>(
    mut inbound: mpsc::Receiver<Inbound>,
    mut sink: S,
    link: &Link,
) -> Result<Option<(u16, String)>, StreamError>
where
    S: Sink<Message, Error = tungstenite::Error> + Unpin,
{
    while let Some(item) = inbound.recv().await {
        match item {
            Inbound::Text(data) => {
                let len = data.len();
                let text = Utf8Bytes::try_from(data)
                    .map_err(|_| StreamError::Unexpected("text that is not UTF-8"))?;
                sink.send(Message::Text(text)).await.map_err(local)?;
                link.credit(len).await?;
            }
            Inbound::Binary(data) => {
                let len = data.len();
                sink.send(Message::Binary(data)).await.map_err(local)?;
                link.credit(len).await?;
            }
            Inbound::Close(code, reason) => {
                sink.send(Message::Close(close_frame(code, reason.clone())))
                    .await
                    .map_err(local)?;
                return Ok(Some((code, reason)));
            }
            Inbound::Body(_) | Inbound::End => {
                return Err(StreamError::Unexpected("an HTTP body"));
            }
        }
    }
    Ok(None)
}

fn close_frame(code: u16, reason: String) -> Option<CloseFrame> {
    match code {
        NO_STATUS | ABNORMAL | TLS_FAILURE => None,
        code => Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        }),
    }
}

fn local_request(head: &RequestHead) -> Result<http::request::Builder, StreamError> {
    let method = Method::from_bytes(head.method.as_bytes())
        .map_err(|error| StreamError::Head(error.to_string()))?;
    let mut builder = Request::builder()
        .method(method)
        .uri(without_token(&head.uri));
    let mut has_host = false;
    for (name, value) in &head.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|error| StreamError::Head(error.to_string()))?;
        if is_dropped(&name) {
            continue;
        }
        has_host |= name == header::HOST;
        let value = HeaderValue::from_bytes(&latin1(value)?)
            .map_err(|error| StreamError::Head(error.to_string()))?;
        builder = builder.header(name, value);
    }
    if !has_host {
        builder = builder.header(header::HOST, LOCAL_HOST);
    }
    Ok(builder)
}

fn upgrade_request(head: &RequestHead) -> Result<http::request::Builder, StreamError> {
    Ok(local_request(head)?
        .method(Method::GET)
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, "websocket")
        .header(header::SEC_WEBSOCKET_VERSION, "13")
        .header(header::SEC_WEBSOCKET_KEY, generate_key()))
}

fn response_head(status: StatusCode, headers: &HeaderMap) -> ResponseHead {
    ResponseHead {
        status: status.as_u16(),
        headers: headers
            .iter()
            .filter(|(name, _)| {
                !HOP_BY_HOP.contains(&name.as_str()) && !HANDSHAKE.contains(&name.as_str())
            })
            .map(|(name, value)| (name.as_str().to_string(), from_latin1(value.as_bytes())))
            .collect(),
    }
}

fn is_dropped(name: &HeaderName) -> bool {
    let name = name.as_str();
    HOP_BY_HOP.contains(&name) || CREDENTIALS.contains(&name) || HANDSHAKE.contains(&name)
}

fn without_token(uri: &str) -> String {
    let Some((path, query)) = uri.split_once('?') else {
        return uri.to_string();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && pair.split('=').next() != Some("token"))
        .collect();
    if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    }
}

fn latin1(text: &str) -> Result<Vec<u8>, StreamError> {
    text.chars()
        .map(|c| {
            u8::try_from(u32::from(c))
                .map_err(|_| StreamError::Head(format!("header value holds {c:?}, beyond Latin-1")))
        })
        .collect()
}

fn from_latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

fn local(error: impl std::fmt::Display) -> StreamError {
    StreamError::Local(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_token_never_reaches_the_local_server() {
        assert_eq!(without_token("/api/ws?token=abc"), "/api/ws");
        assert_eq!(without_token("/a?x=1&token=abc&y=2"), "/a?x=1&y=2");
        assert_eq!(without_token("/a?tokens=1"), "/a?tokens=1");
        assert_eq!(without_token("/plain"), "/plain");
    }

    #[test]
    fn credentials_and_hop_by_hop_headers_are_dropped() {
        let head = RequestHead {
            method: "GET".to_string(),
            uri: "/".to_string(),
            headers: vec![
                ("Authorization".to_string(), "Bearer relay".to_string()),
                ("cookie".to_string(), "a=b".to_string()),
                ("connection".to_string(), "keep-alive".to_string()),
                ("accept".to_string(), "text/html".to_string()),
            ],
            websocket: false,
            user: "u".to_string(),
        };
        let request = local_request(&head)
            .and_then(|builder| {
                builder
                    .body(())
                    .map_err(|error| StreamError::Head(error.to_string()))
            })
            .expect("request");
        let names: Vec<&str> = request.headers().keys().map(HeaderName::as_str).collect();
        assert_eq!(names, ["accept", "host"]);
    }

    #[test]
    fn header_values_round_trip_as_latin1_byte_strings() {
        let raw = "attachment; filename=\"Grüße.wav\"".as_bytes();
        let text = from_latin1(raw);
        assert_eq!(latin1(&text).expect("latin1"), raw);
        assert!(latin1("\u{1F4FB}").is_err());
    }

    #[test]
    fn reserved_close_codes_are_never_sent() {
        assert!(close_frame(NO_STATUS, String::new()).is_none());
        assert!(close_frame(ABNORMAL, String::new()).is_none());
        let normal = close_frame(1000, "bye".to_string()).expect("normal close");
        assert_eq!(u16::from(normal.code), 1000);
    }
}
