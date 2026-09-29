use sdrmm_wire::{
    Attitude, GpsNode, HeadingSource, NodeBody, PatchNode, Position, PositionSource,
    SurfaceRefusal, WorkspaceSnapshot,
};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{HeaderValue, StatusCode},
};

use super::*;
use crate::phones::tests::pair_one;

async fn serve_phones(engine: Arc<Engine>) -> (std::net::SocketAddr, AppState) {
    let store = Arc::new(crate::Store::open(None).expect("in-memory store"));
    let state = AppState::new(engine, store);
    let (app, background) =
        crate::main_router(state.clone(), &crate::ServerOptions::default(), true);
    background.detach();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app).into_future());
    (addr, state)
}

async fn dial_with(
    addr: std::net::SocketAddr,
    headers: &[(&'static str, String)],
) -> Result<WsClient, tungstenite::Error> {
    let mut request = format!("ws://{addr}/api/ws")
        .into_client_request()
        .expect("request");
    for (name, value) in headers {
        request
            .headers_mut()
            .insert(*name, HeaderValue::from_str(value).expect("header"));
    }
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(ws, _)| ws)
}

async fn dial_phone(addr: std::net::SocketAddr, token: &str) -> WsClient {
    dial_with(addr, &[("authorization", format!("Bearer {token}"))])
        .await
        .expect("phone socket")
}

async fn hello(ws: &mut WsClient) -> Option<String> {
    match next_event(ws).await {
        ServerEvent::Hello { phone, .. } => phone,
        other => panic!("expected Hello, got {other:?}"),
    }
}

fn bind_gps(state: &AppState, node: &str, phone: &str) {
    let mut snapshot = WorkspaceSnapshot::empty();
    snapshot.graph.nodes.push(PatchNode {
        id: node.to_owned(),
        body: NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Phone {
                phone: phone.to_owned(),
            }),
        }),
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    });
    let id = state
        .store
        .create_workspace("field", &snapshot)
        .expect("workspace");
    state.store.activate_workspace(id).expect("activate");
    state.gps.reconcile(state);
}

fn pose(latitude: f64) -> PositionFix {
    PositionFix {
        latitude,
        longitude: 13.405,
        altitude_m: None,
        accuracy_m: Some(4.0),
        speed_mps: None,
        track_deg: None,
        time: "2026-09-28T12:00:00Z".to_owned(),
        attitude: Attitude {
            heading_deg: Some(87.5),
            heading_accuracy_deg: Some(6.0),
            heading_source: Some(HeadingSource::Fused),
            ..Attitude::default()
        },
    }
}

async fn position_of(ws: &mut WsClient, node: &str) -> (Option<PositionFix>, Option<String>) {
    loop {
        if let ServerEvent::PositionChanged {
            node: changed,
            fix,
            error,
        } = next_event(ws).await
            && changed == node
        {
            return (fix, error);
        }
    }
}

async fn wait_until(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn close_code(ws: &mut WsClient) -> Option<u16> {
    loop {
        match timeout(WAIT, ws.next()).await.expect("timed out") {
            Some(Ok(tungstenite::Message::Close(frame))) => {
                return frame.map(|frame| u16::from(frame.code));
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return None,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_names_the_phone_and_protocol() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let mut ws = dial_phone(addr, &paired.token).await;
    match next_event(&mut ws).await {
        ServerEvent::Hello {
            protocol, phone, ..
        } => {
            assert_eq!(protocol, API_PROTOCOL);
            assert_eq!(phone, Some(paired.phone.id));
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_socket_gets_only_phone_events() {
    let (tx, rx) = broadcast::channel::<ServerEvent>(16);
    let (out_tx, mut out_rx) = outbox::channel();
    let task = spawn_events(rx, out_tx, true);
    let kept = [
        ServerEvent::StateChanged {
            scope: StateScope::Phones,
        },
        ServerEvent::PositionChanged {
            node: "car".to_owned(),
            fix: None,
            error: Some("phone offline".to_owned()),
        },
        ServerEvent::Error {
            message: "x".to_owned(),
        },
    ];
    let dropped = [
        ServerEvent::DecodedLost { count: 1 },
        ServerEvent::ChannelLevels {
            device_set: 0,
            levels: Vec::new(),
        },
        ServerEvent::StreamStarted {
            stream_id: 1,
            device_set: 0,
            stream: 0,
        },
    ];
    for event in dropped.iter().chain(&kept) {
        tx.send(event.clone()).expect("send");
    }
    drop(tx);
    let mut forwarded = Vec::new();
    while let Some(Message::Text(text)) = out_rx.recv().await {
        forwarded.push(serde_json::from_str::<ServerEvent>(&text).expect("event json"));
    }
    task.await.expect("events task");
    assert_eq!(forwarded, kept);

    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    let _ = state
        .decoded_text
        .send(r#"{"type":"DecodedLost","data":{"count":7}}"#.to_owned().into());
    state.engine.emit_scope(StateScope::Devices);
    loop {
        match next_event(&mut ws).await {
            ServerEvent::StateChanged {
                scope: StateScope::Devices,
            } => break,
            ServerEvent::DecodedLost { .. } => panic!("a decoder record reached a phone"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_command_outside_the_list_is_refused() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    send(
        &mut ws,
        &ClientCommand::SubscribeAudio {
            device_set: 0,
            channel: 0,
            fx: Vec::new(),
        },
    )
    .await;
    loop {
        if let ServerEvent::Error { message } = next_event(&mut ws).await {
            assert_eq!(message, NOT_FOR_PHONES);
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_hears_why_its_surface_was_refused() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    send(
        &mut ws,
        &ClientCommand::SubscribeSurface {
            node: "tri".to_owned(),
            fit: None,
        },
    )
    .await;
    loop {
        if let ServerEvent::SurfaceRefused { node, reason } = next_event(&mut ws).await {
            assert_eq!((node.as_str(), reason), ("tri", SurfaceRefusal::NoSurface));
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_phone_socket_closes_with_4003() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    let revoking = state.clone();
    let id = paired.phone.id.clone();
    tokio::task::spawn_blocking(move || revoking.phones.revoke(&revoking, &id))
        .await
        .expect("revoke task")
        .expect("revoke");
    assert_eq!(close_code(&mut ws).await, Some(WS_CLOSE_REVOKED));
    let again = dial_with(
        addr,
        &[("authorization", format!("Bearer {}", paired.token))],
    )
    .await;
    match again {
        Err(tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        other => panic!("a revoked key opened a socket: {:?}", other.is_ok()),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn revoke_closes_the_phone_socket() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    bind_gps(&state, "car", &paired.phone.id);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    let waiting = ServerEvent::PositionChanged {
        node: "car".to_owned(),
        fix: None,
        error: Some("waiting for a position fix".to_owned()),
    };
    wait_until(|| state.gps.snapshot().contains(&waiting)).await;
    let revoking = state.clone();
    let id = paired.phone.id.clone();
    tokio::task::spawn_blocking(move || revoking.phones.revoke(&revoking, &id))
        .await
        .expect("revoke task")
        .expect("revoke");
    assert_eq!(close_code(&mut ws).await, Some(WS_CLOSE_REVOKED));
    let offline = ServerEvent::PositionChanged {
        node: "car".to_owned(),
        fix: None,
        error: Some("phone not paired".to_owned()),
    };
    wait_until(|| state.gps.snapshot().contains(&offline)).await;
    assert!(!state.phones.online(&paired.phone.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_origin_is_refused() {
    let (addr, state) = serve_phones(test_engine()).await;
    match dial_with(addr, &[("origin", "http://evil.example".to_owned())]).await {
        Err(tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            let body = response.body().as_deref().unwrap_or_default();
            let error: sdrmm_wire::ApiError = serde_json::from_slice(body).expect("error body");
            assert_eq!(error.error, "Cross-origin request refused");
        }
        other => panic!("a foreign page opened a socket: {:?}", other.is_ok()),
    }
    let mut own = dial_with(addr, &[("origin", format!("http://{addr}"))])
        .await
        .expect("same origin");
    hello(&mut own).await;
    let mut native = dial_with(addr, &[]).await.expect("no origin");
    hello(&mut native).await;
    drop(state);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_speaks_the_sdrmm_subprotocol() {
    let (addr, _state) = serve_phones(test_engine()).await;
    let mut request = format!("ws://{addr}/api/ws")
        .into_client_request()
        .expect("request");
    request
        .headers_mut()
        .insert("sec-websocket-protocol", HeaderValue::from_static("sdrmm"));
    let (_ws, response) = tokio_tungstenite::connect_async(request)
        .await
        .expect("socket");
    assert_eq!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some(WS_SUBPROTOCOL)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_phone_socket_moves_its_gps_node() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    bind_gps(&state, "car", &paired.phone.id);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    let fix = pose(52.52);
    send(
        &mut ws,
        &ClientCommand::PublishPose {
            fix: Some(fix.clone()),
            error: None,
        },
    )
    .await;
    loop {
        let (shown, _) = position_of(&mut ws, "car").await;
        if let Some(shown) = shown {
            assert_eq!(shown, fix);
            assert_eq!(shown.attitude.heading_deg, Some(87.5));
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pose_rate_is_limited_per_connection() {
    let (addr, state) = serve_phones(test_engine()).await;
    state.gps.set_pose_interval(Duration::ZERO);
    let paired = pair_one(&state.phones);
    bind_gps(&state, "car", &paired.phone.id);
    let mut ws = dial_phone(addr, &paired.token).await;
    hello(&mut ws).await;
    for step in 0..40 {
        send(
            &mut ws,
            &ClientCommand::PublishPose {
                fix: Some(pose(52.0 + f64::from(step) * 1e-4)),
                error: None,
            },
        )
        .await;
    }
    let mut limited = 0;
    let quiet = Instant::now() + Duration::from_millis(500);
    while let Ok(Some(Ok(message))) =
        timeout(quiet.saturating_duration_since(Instant::now()), ws.next()).await
    {
        if let tungstenite::Message::Text(text) = message
            && let Ok(ServerEvent::Error { message }) = serde_json::from_str(text.as_str())
        {
            assert_eq!(message, POSE_TOO_FAST);
            limited += 1;
        }
    }
    assert_eq!(limited, 1, "the limit was reported {limited} times");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_phone_socket_makes_its_nodes_offline() {
    let (addr, state) = serve_phones(test_engine()).await;
    let paired = pair_one(&state.phones);
    let id = paired.phone.id.clone();
    bind_gps(&state, "car", &id);
    let error = |state: &AppState| {
        state
            .gps
            .snapshot()
            .into_iter()
            .find_map(|event| match event {
                ServerEvent::PositionChanged { error, .. } => error,
                _ => None,
            })
    };
    assert_eq!(error(&state).as_deref(), Some("phone offline"));
    let mut first = dial_phone(addr, &paired.token).await;
    hello(&mut first).await;
    let mut second = dial_phone(addr, &paired.token).await;
    hello(&mut second).await;
    wait_until(|| error(&state).as_deref() == Some("waiting for a position fix")).await;
    first.close(None).await.expect("close");
    drop(first);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(state.phones.online(&id));
    assert_eq!(error(&state).as_deref(), Some("waiting for a position fix"));
    second.close(None).await.expect("close");
    drop(second);
    wait_until(|| error(&state).as_deref() == Some("phone offline")).await;
    assert!(!state.phones.online(&id));
}
