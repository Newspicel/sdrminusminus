#![allow(clippy::expect_used, clippy::result_large_err)]
use std::{sync::Arc, time::Duration};

use axum::{
    Extension, Router,
    body::Bytes,
    extract::ws::{Message as LocalMessage, WebSocketUpgrade},
    http::{HeaderMap, Uri, header},
    routing::{get, post},
};
use futures::{SinkExt as _, StreamExt as _};
use sdrmm_tunnel::{
    Config, DeviceKey, PING, Relayed, Status, Tunnel,
    frame::{Frame, PROTOCOL, RequestHead, ResponseHead, WINDOW},
    identity::verify_proof,
};
use tokio::{net::TcpListener, sync::watch};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
        http::HeaderValue,
        protocol::{CloseFrame, frame::coding::CloseCode},
    },
};

const WAIT: Duration = Duration::from_secs(5);
const BIG: usize = 1 << 20;

struct Relay {
    listener: TcpListener,
    public_key: [u8; 32],
}

struct Session {
    socket: WebSocketStream<tokio::net::TcpStream>,
}

impl Relay {
    async fn start() -> (Self, Tunnel) {
        Self::reporting(None).await
    }

    async fn reporting(site: Option<Bytes>) -> (Self, Tunnel) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let (key, _) = DeviceKey::generate().expect("key");
        let public_key = key.public_key().expect("public key");
        let config = Config {
            url: format!("ws://{address}/v1/device/test")
                .parse()
                .expect("url"),
            key: Arc::new(key),
            health: watch::channel(site).1,
        };
        let tunnel = Tunnel::spawn(config, router());
        (
            Self {
                listener,
                public_key,
            },
            tunnel,
        )
    }

    async fn accept(&self) -> WebSocketStream<tokio::net::TcpStream> {
        let (tcp, _) = tokio::time::timeout(WAIT, self.listener.accept())
            .await
            .expect("device connects")
            .expect("accept");
        tokio_tungstenite::accept_hdr_async(tcp, |request: &Request, mut response: Response| {
            assert_eq!(request.uri().path(), "/v1/device/test");
            assert_eq!(
                request.headers().get("sec-websocket-protocol"),
                Some(&HeaderValue::from_static(PROTOCOL))
            );
            response
                .headers_mut()
                .insert("sec-websocket-protocol", HeaderValue::from_static(PROTOCOL));
            Ok(response)
        })
        .await
        .expect("upgrade")
    }

    async fn session(&self) -> Session {
        let mut session = Session {
            socket: self.accept().await,
        };
        let nonce = [9u8; 32];
        session.send(Frame::Challenge { nonce }).await;
        let Frame::Proof { signature } = session.next().await else {
            panic!("expected a proof");
        };
        assert!(verify_proof(&self.public_key, &nonce, &signature));
        session.send(Frame::Ready).await;
        session
    }
}

impl Session {
    async fn send(&mut self, frame: Frame) {
        self.socket
            .send(Message::Binary(frame.encode().expect("encode")))
            .await
            .expect("send");
    }

    async fn next(&mut self) -> Frame {
        loop {
            let message = tokio::time::timeout(WAIT, self.socket.next())
                .await
                .expect("frame in time")
                .expect("socket open")
                .expect("message");
            match message {
                Message::Binary(bytes) => return Frame::decode(bytes).expect("decode"),
                Message::Text(text) => assert_eq!(text.as_str(), PING),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    async fn next_data(&mut self) -> Frame {
        loop {
            match self.next().await {
                Frame::Credit { .. } => {}
                frame => return frame,
            }
        }
    }

    async fn request(&mut self, stream: u32, method: &str, uri: &str, body: &[Bytes]) {
        self.send(Frame::Request {
            stream,
            head: head(method, uri, false),
        })
        .await;
        for chunk in body {
            self.send(Frame::Body {
                stream,
                data: chunk.clone(),
            })
            .await;
        }
        self.send(Frame::End { stream }).await;
    }

    async fn response(&mut self, stream: u32) -> (ResponseHead, Vec<u8>) {
        let Frame::Response { stream: id, head } = self.next_data().await else {
            panic!("expected a response head");
        };
        assert_eq!(id, stream);
        let mut body = Vec::new();
        loop {
            match self.next_data().await {
                Frame::Body { data, .. } => {
                    body.extend_from_slice(&data);
                    self.send(Frame::Credit {
                        stream,
                        bytes: u32::try_from(data.len()).expect("chunk size"),
                    })
                    .await;
                }
                Frame::End { .. } => return (head, body),
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}

fn head(method: &str, uri: &str, websocket: bool) -> RequestHead {
    RequestHead {
        method: method.to_string(),
        uri: uri.to_string(),
        headers: vec![
            ("host".to_string(), "abc.sdrmm.link".to_string()),
            (
                "authorization".to_string(),
                "Bearer relay-token".to_string(),
            ),
        ],
        websocket,
        user: "user-1".to_string(),
    }
}

fn router() -> Router {
    Router::new()
        .route(
            "/whoami",
            get(
                |Extension(relayed): Extension<Relayed>, headers: HeaderMap, uri: Uri| async move {
                    format!(
                        "{} {} {} {}",
                        relayed.user,
                        headers.contains_key(header::AUTHORIZATION),
                        headers
                            .get(header::HOST)
                            .and_then(|host| host.to_str().ok())
                            .unwrap_or_default(),
                        uri
                    )
                },
            ),
        )
        .route("/echo", post(|body: Bytes| async move { body }))
        .route("/big", get(|| async { vec![7u8; BIG] }))
        .route(
            "/ws",
            get(|upgrade: WebSocketUpgrade| async move {
                upgrade.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if matches!(message, LocalMessage::Close(_)) {
                            break;
                        }
                        if socket.send(message).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
}

async fn wait_for(
    status: &mut watch::Receiver<Status>,
    wanted: impl Fn(&Status) -> bool,
) -> Status {
    tokio::time::timeout(WAIT, status.wait_for(|status| wanted(status)))
        .await
        .expect("status in time")
        .expect("tunnel alive")
        .clone()
}

#[tokio::test]
async fn a_request_reaches_the_router_as_the_relayed_user_without_credentials() {
    let (relay, tunnel) = Relay::start().await;
    let mut session = relay.session().await;
    wait_for(&mut tunnel.status(), |status| *status == Status::Online).await;
    session
        .request(1, "GET", "/whoami?token=relay-token&x=1", &[])
        .await;
    let (head, body) = session.response(1).await;
    assert_eq!(head.status, 200);
    assert_eq!(
        String::from_utf8(body).expect("utf8"),
        "user-1 false abc.sdrmm.link /whoami?x=1"
    );
}

#[tokio::test]
async fn a_download_stops_at_the_window_until_credit_arrives() {
    let (relay, _tunnel) = Relay::start().await;
    let mut session = relay.session().await;
    session.request(3, "GET", "/big", &[]).await;
    let Frame::Response { head, .. } = session.next_data().await else {
        panic!("expected a response head");
    };
    assert_eq!(head.status, 200);
    let mut received = 0usize;
    while received < WINDOW as usize {
        let Frame::Body { data, .. } = session.next_data().await else {
            panic!("expected body");
        };
        received += data.len();
    }
    assert!(received < WINDOW as usize + 64 * 1024);
    let stalled = tokio::time::timeout(Duration::from_millis(300), session.socket.next()).await;
    assert!(stalled.is_err(), "sent past the window: {stalled:?}");
    session
        .send(Frame::Credit {
            stream: 3,
            bytes: u32::try_from(BIG).expect("size"),
        })
        .await;
    loop {
        match session.next_data().await {
            Frame::Body { data, .. } => received += data.len(),
            Frame::End { .. } => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(received, BIG);
}

#[tokio::test]
async fn an_upload_larger_than_the_window_is_echoed_whole() {
    let (relay, _tunnel) = Relay::start().await;
    let mut session = relay.session().await;
    let chunks: Vec<Bytes> = (0u8..5).map(|n| Bytes::from(vec![n; 100 * 1024])).collect();
    session
        .send(Frame::Request {
            stream: 5,
            head: head("POST", "/echo", false),
        })
        .await;
    let mut in_flight = 0i64;
    for chunk in &chunks {
        while in_flight + chunk.len() as i64 > i64::from(WINDOW) {
            let Frame::Credit { bytes, .. } = session.next().await else {
                panic!("expected credit while uploading");
            };
            in_flight -= i64::from(bytes);
        }
        in_flight += chunk.len() as i64;
        session
            .send(Frame::Body {
                stream: 5,
                data: chunk.clone(),
            })
            .await;
    }
    session.send(Frame::End { stream: 5 }).await;
    let (head, body) = session.response(5).await;
    assert_eq!(head.status, 200);
    assert_eq!(body, chunks.concat());
}

#[tokio::test]
async fn a_websocket_echoes_through_the_tunnel_and_closes_cleanly() {
    let (relay, _tunnel) = Relay::start().await;
    let mut session = relay.session().await;
    session
        .send(Frame::Request {
            stream: 7,
            head: head("GET", "/ws?token=relay-token", true),
        })
        .await;
    let Frame::Response { head, .. } = session.next_data().await else {
        panic!("expected the upgrade answer");
    };
    assert_eq!(head.status, 101);
    session
        .send(Frame::Text {
            stream: 7,
            data: Bytes::from_static(b"hello"),
        })
        .await;
    assert_eq!(
        session.next_data().await,
        Frame::Text {
            stream: 7,
            data: Bytes::from_static(b"hello")
        }
    );
    session
        .send(Frame::Binary {
            stream: 7,
            data: Bytes::from_static(&[1, 2, 3]),
        })
        .await;
    assert_eq!(
        session.next_data().await,
        Frame::Binary {
            stream: 7,
            data: Bytes::from_static(&[1, 2, 3])
        }
    );
    session
        .send(Frame::Close {
            stream: 7,
            code: 1000,
            reason: "bye".to_string(),
        })
        .await;
    let closing = session.next_data().await;
    let Frame::Close { code, .. } = closing else {
        panic!("expected the close echo, got {closing:?}");
    };
    assert_eq!(code, 1000);
}

#[tokio::test]
async fn a_rejected_device_stops_retrying() {
    let (relay, tunnel) = Relay::start().await;
    let mut socket = relay.accept().await;
    socket
        .close(Some(CloseFrame {
            code: CloseCode::from(4004),
            reason: "unknown device".into(),
        }))
        .await
        .expect("close");
    let status = wait_for(&mut tunnel.status(), |status| {
        matches!(status, Status::Rejected { .. })
    })
    .await;
    assert_eq!(
        status,
        Status::Rejected {
            reason: "unknown device".to_string()
        }
    );
    let again = tokio::time::timeout(Duration::from_secs(2), relay.listener.accept()).await;
    assert!(again.is_err(), "a rejected device reconnected");
}

#[tokio::test]
async fn the_device_reconnects_after_the_relay_drops_it() {
    let (relay, tunnel) = Relay::start().await;
    let mut status = tunnel.status();
    let first = relay.session().await;
    wait_for(&mut status, |status| *status == Status::Online).await;
    drop(first);
    wait_for(&mut status, |status| {
        matches!(status, Status::Retrying { .. })
    })
    .await;
    let mut second = relay.session().await;
    wait_for(&mut status, |status| *status == Status::Online).await;
    second.request(1, "GET", "/whoami", &[]).await;
    let (head, _) = second.response(1).await;
    assert_eq!(head.status, 200);
}

#[tokio::test]
async fn a_reset_stream_stops_sending() {
    let (relay, _tunnel) = Relay::start().await;
    let mut session = relay.session().await;
    session.request(9, "GET", "/big", &[]).await;
    let Frame::Response { .. } = session.next_data().await else {
        panic!("expected a response head");
    };
    session
        .send(Frame::Reset {
            stream: 9,
            reason: "browser left".to_string(),
        })
        .await;
    session
        .send(Frame::Credit {
            stream: 9,
            bytes: u32::try_from(BIG).expect("size"),
        })
        .await;
    let mut received = 0usize;
    while let Ok(Some(Ok(Message::Binary(bytes)))) =
        tokio::time::timeout(Duration::from_millis(300), session.socket.next()).await
    {
        if let Frame::Body { data, .. } = Frame::decode(bytes).expect("decode") {
            received += data.len();
        }
    }
    assert!(received <= WINDOW as usize + 64 * 1024, "{received}");
    session.request(11, "GET", "/whoami", &[]).await;
    let (head, _) = session.response(11).await;
    assert_eq!(head.status, 200);
}

#[tokio::test]
async fn the_first_frame_after_ready_reports_health() {
    let (relay, _tunnel) = Relay::reporting(Some(Bytes::from_static(br#"{"clients":1}"#))).await;
    let mut session = relay.session().await;
    let frame = session.next().await;
    let Frame::Health { data } = frame else {
        panic!("expected health, got {frame:?}");
    };
    let health: serde_json::Value = serde_json::from_slice(&data).expect("json");
    assert_eq!(
        health,
        serde_json::json!({"rtt_ms": null, "site": {"clients": 1}})
    );
}

#[tokio::test]
async fn health_from_the_relay_breaks_the_protocol() {
    let (relay, tunnel) = Relay::start().await;
    let mut status = tunnel.status();
    let mut session = relay.session().await;
    wait_for(&mut status, |status| *status == Status::Online).await;
    session
        .send(Frame::Health {
            data: Bytes::from_static(b"{}"),
        })
        .await;
    let status = wait_for(&mut status, |status| {
        matches!(status, Status::Retrying { .. })
    })
    .await;
    let Status::Retrying { error, .. } = status else {
        panic!("expected a retry");
    };
    assert!(error.contains("unexpected frame on stream 0"), "{error}");
}
