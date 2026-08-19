use std::sync::Mutex;

use axum::{
    Router,
    body::to_bytes,
    extract::{Request, State},
    http::Method,
    response::{IntoResponse, Response as AxumResponse},
    routing::any,
};
use reqwest::StatusCode;
use sdrmm_wire::{
    ChannelNode, DecoderEvent, DvMode, EventAudio, EventFilterNode, EventOutputNode, PatchEdge,
    PatchGraph, PatchNode, PortRef, Position, RackLayout, RttyText, UpdateWorkspaceRequest,
    WorkspaceSnapshot,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[derive(Clone)]
struct Captured {
    method: Method,
    uri: String,
    authorization: Option<String>,
    content_type: Option<String>,
    body: Bytes,
}

type Captures = Arc<Mutex<Vec<Captured>>>;

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
    PatchEdge {
        from: PortRef {
            node: from.0.to_owned(),
            port: from.1.to_owned(),
        },
        to: PortRef {
            node: to.0.to_owned(),
            port: to.1.to_owned(),
        },
    }
}

fn discord_webhook() -> EventOutputTarget {
    EventOutputTarget::Webhook {
        url: "https://discord.com/api/webhooks/1/token".to_owned(),
        format: WebhookFormat::Discord,
    }
}

fn call() -> VoiceCall {
    VoiceCall {
        id: 7,
        node: "trunk".to_owned(),
        source_node: "dmr".to_owned(),
        started_at: "2026-08-15T10:00:00Z".to_owned(),
        ended_at: "2026-08-15T10:00:01Z".to_owned(),
        duration_ms: 1_200,
        device_set: 1,
        channel: 2,
        freq_hz: 451_125_000.0,
        mode: DvMode::Dmr,
        slot: Some(2),
        color_code: Some(3),
        source: Some(1001),
        destination: Some(91),
        group_call: Some(true),
        encrypted: false,
        emergency: false,
        audio: Some(EventAudio {
            url: "/api/calls/7/audio".to_owned(),
            media_type: "audio/wav".to_owned(),
        }),
        audio_error: None,
    }
}

fn call_record() -> DecodedRecord {
    DecodedRecord {
        event: DecoderEvent::Call(call()),
        ..decoded()
    }
}

fn decoded() -> DecodedRecord {
    DecodedRecord {
        device_set: 1,
        channel: 2,
        at: "2026-08-15T10:00:02Z".to_owned(),
        freq_hz: 14_080_000.0,
        event: DecoderEvent::Rtty(RttyText {
            text: "CQ TEST".to_owned(),
        }),
    }
}

async fn server() -> (String, Captures) {
    let captures = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/{*path}", any(capture))
        .with_state(captures.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("local address");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("test server") });
    (format!("http://{address}"), captures)
}

async fn capture(State(captures): State<Captures>, request: Request) -> AxumResponse {
    let method = request.method().clone();
    let uri = request.uri().to_string();
    let authorization = request
        .headers()
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let content_type = request
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .expect("request body");
    captures.lock().expect("captures").push(Captured {
        method,
        uri: uri.clone(),
        authorization,
        content_type,
        body,
    });
    if uri == "/oversized-error" {
        return (
            StatusCode::BAD_REQUEST,
            "x".repeat(MAX_ERROR_BODY.saturating_mul(4)),
        )
            .into_response();
    }
    if uri == "/rate-limited-header" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(reqwest::header::RETRY_AFTER, "1.5")],
            r#"{"errcode":"M_LIMIT_EXCEEDED","retry_after_ms":9000}"#,
        )
            .into_response();
    }
    if uri == "/rate-limited-body" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            r#"{"errcode":"M_LIMIT_EXCEEDED","retry_after_ms":2500}"#,
        )
            .into_response();
    }
    if uri == "/rate-limited-bare" {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    }
    if uri == "/rate-limited-forever" {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(reqwest::header::RETRY_AFTER, "86400")],
            "",
        )
            .into_response();
    }
    if uri.contains("/_matrix/media/v3/upload") {
        return (
            StatusCode::OK,
            r#"{"content_uri":"mxc://matrix.test/audio7"}"#,
        )
            .into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

#[test]
fn summary_is_one_caption_line_holding_the_complete_call_metadata() {
    let text = format_call(&call());
    assert_eq!(
        text,
        "DMR call · talkgroup 91 · from 1001 · TS2 · CC3 · 1.2 s · 451.125000 MHz"
    );
}

#[test]
fn an_emergency_encrypted_call_names_both_flags() {
    let text = format_call(&VoiceCall {
        emergency: true,
        encrypted: true,
        ..call()
    });
    assert!(text.ends_with("· emergency · encrypted"));
}

#[test]
fn the_html_caption_escapes_the_audio_error() {
    let html = format_call_html(&VoiceCall {
        audio_error: Some("lost <2> blocks & counting".to_owned()),
        ..call()
    });
    assert!(html.starts_with("<strong>DMR call</strong> · talkgroup 91"));
    assert!(html.ends_with("<br/><em>lost &lt;2&gt; blocks &amp; counting</em>"));
}

#[test]
fn summary_contains_generic_decoder_metadata() {
    let text = format_decoded(&decoded());
    assert!(text.contains("RTTY decode"));
    assert!(text.contains("CQ TEST"));
    assert!(text.contains("Frequency: 14.080000 MHz"));
    assert!(text.contains("Received: 2026-08-15T10:00:02Z"));
}

#[test]
fn resolve_maps_configured_outputs_and_the_events_port() {
    let store = Store::open(None).expect("open store");
    let active = store
        .active_workspace()
        .expect("read active workspace")
        .expect("seeded workspace");
    let configured = discord_webhook();
    let graph = PatchGraph {
        nodes: vec![
            node(
                "decoder",
                NodeBody::Channel(ChannelNode {
                    channel_type: "rtty".to_owned(),
                    record_calls: false,
                }),
            ),
            node(
                "configured",
                NodeBody::EventOutput(EventOutputNode {
                    target: configured.clone(),
                }),
            ),
            node("empty", NodeBody::EventOutput(EventOutputNode::default())),
        ],
        edges: vec![
            edge(("decoder", "events"), ("configured", "events")),
            edge(("decoder", "events"), ("empty", "events")),
        ],
    };
    store
        .update_workspace(
            active.info.id,
            &UpdateWorkspaceRequest {
                revision: active.info.revision,
                name: None,
                snapshot: Some(WorkspaceSnapshot::new(graph, RackLayout::default())),
            },
        )
        .expect("update workspace");

    let routing = resolve(&store, None).expect("resolve outputs");
    let bindings = routing.bindings;

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].node, "configured");
    assert_eq!(bindings[0].paths.len(), 1);
    assert_eq!(bindings[0].paths[0].source, "decoder");
    assert_eq!(bindings[0].target, configured);
}

fn path(source: &str, filters: Vec<EventFilterNode>) -> EventPath {
    EventPath {
        source: source.to_owned(),
        filters,
    }
}

fn routing_for(paths: Vec<EventPath>) -> Routing {
    Routing {
        bindings: vec![Binding {
            node: "matched".to_owned(),
            paths,
            target: discord_webhook(),
        }],
        decoded_sources: HashMap::from([((1, 2), "decoder".to_owned())]),
    }
}

#[test]
fn decoded_records_route_only_to_outputs_wired_to_the_live_channel() {
    let target = discord_webhook();
    let routing = Routing {
        bindings: vec![
            Binding {
                node: "matched".to_owned(),
                paths: vec![path("decoder", Vec::new())],
                target: target.clone(),
            },
            Binding {
                node: "other".to_owned(),
                paths: vec![path("other-decoder", Vec::new())],
                target,
            },
        ],
        decoded_sources: HashMap::from([((1, 2), "decoder".to_owned())]),
    };

    let deliveries = decoded_deliveries(&routing, &decoded(), 9, &Calls::default());

    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].node, "matched");
    assert!(deliveries[0].message.body.contains("RTTY decode"));
    assert!(deliveries[0].message.transaction.ends_with("-rtty-9"));
}

#[test]
fn a_completed_call_travels_the_events_wire_like_any_other_decode() {
    let routing = routing_for(vec![path("decoder", Vec::new())]);
    let record = DecodedRecord {
        event: DecoderEvent::Call(call()),
        ..decoded()
    };

    let deliveries = decoded_deliveries(&routing, &record, 9, &Calls::default());

    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0].event, "call 7");
    assert!(deliveries[0].message.body.contains("talkgroup 91"));
}

#[test]
fn a_filter_on_the_wire_decides_what_the_output_posts() {
    let only_calls = EventFilterNode {
        kinds: vec!["call".to_owned()],
        ..EventFilterNode::default()
    };
    let routing = routing_for(vec![path("decoder", vec![only_calls])]);

    assert!(
        decoded_deliveries(&routing, &decoded(), 9, &Calls::default()).is_empty(),
        "the filter admits calls only"
    );
    let record = DecodedRecord {
        event: DecoderEvent::Call(call()),
        ..decoded()
    };
    assert_eq!(
        decoded_deliveries(&routing, &record, 9, &Calls::default()).len(),
        1
    );
}

#[test]
fn a_talkgroup_filter_drops_the_calls_it_does_not_name() {
    let routing = routing_for(vec![path(
        "decoder",
        vec![EventFilterNode {
            talkgroups: vec![91],
            ..EventFilterNode::default()
        }],
    )]);
    let wanted = DecodedRecord {
        event: DecoderEvent::Call(call()),
        ..decoded()
    };
    let other = DecodedRecord {
        event: DecoderEvent::Call(VoiceCall {
            destination: Some(4_242),
            ..call()
        }),
        ..decoded()
    };

    assert_eq!(
        decoded_deliveries(&routing, &wanted, 9, &Calls::default()).len(),
        1
    );
    assert!(decoded_deliveries(&routing, &other, 9, &Calls::default()).is_empty());
}

#[test]
fn one_output_can_be_fed_by_a_filtered_and_an_unfiltered_wire() {
    let only_calls = EventFilterNode {
        kinds: vec!["call".to_owned()],
        ..EventFilterNode::default()
    };
    let routing = routing_for(vec![
        path("decoder", vec![only_calls]),
        path("decoder", Vec::new()),
    ]);

    assert_eq!(
        decoded_deliveries(&routing, &decoded(), 9, &Calls::default()).len(),
        1,
        "one open wire is enough to post, and it posts once"
    );
}

#[tokio::test]
async fn a_rate_limited_response_reports_the_header_delay() {
    let (base, _captures) = server().await;
    let response = Client::new()
        .get(format!("{base}/rate-limited-header"))
        .send()
        .await
        .expect("rate limited response");

    let error = checked_response("Test", response)
        .await
        .expect_err("rate limited");

    assert!(matches!(
        error,
        DeliveryError::RateLimited(wait) if wait == Duration::from_millis(1_500)
    ));
}

#[tokio::test]
async fn a_rate_limited_response_falls_back_to_the_matrix_body_field() {
    let (base, _captures) = server().await;
    let response = Client::new()
        .get(format!("{base}/rate-limited-body"))
        .send()
        .await
        .expect("rate limited response");

    let error = checked_response("Test", response)
        .await
        .expect_err("rate limited");

    assert!(matches!(
        error,
        DeliveryError::RateLimited(wait) if wait == Duration::from_millis(2_500)
    ));
}

#[tokio::test]
async fn a_rate_limit_without_any_hint_uses_the_default_delay() {
    let (base, _captures) = server().await;
    let response = Client::new()
        .get(format!("{base}/rate-limited-bare"))
        .send()
        .await
        .expect("rate limited response");

    let error = checked_response("Test", response)
        .await
        .expect_err("rate limited");

    assert!(matches!(
        error,
        DeliveryError::RateLimited(wait) if wait == DEFAULT_RETRY_DELAY
    ));
}

#[tokio::test]
async fn an_absurd_retry_after_is_capped() {
    let (base, _captures) = server().await;
    let response = Client::new()
        .get(format!("{base}/rate-limited-forever"))
        .send()
        .await
        .expect("rate limited response");

    let error = checked_response("Test", response)
        .await
        .expect_err("rate limited");

    assert!(matches!(
        error,
        DeliveryError::RateLimited(wait) if wait == MAX_RETRY_DELAY
    ));
}

#[tokio::test]
async fn discord_sends_metadata_and_wav_in_one_webhook_message() {
    let (base, captures) = server().await;
    let message = call_message(
        "chat",
        &call_record(),
        &call(),
        Some(Bytes::from_static(b"RIFF-wave")),
    );
    send_webhook(
        &Client::new(),
        &format!("{base}/api/webhooks/1/token?thread_id=5&wait=false"),
        WebhookFormat::Discord,
        &message,
    )
    .await
    .expect("Discord delivery");
    let captured = captures.lock().expect("captures");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, Method::POST);
    assert_eq!(
        captured[0].uri,
        "/api/webhooks/1/token?thread_id=5&wait=true"
    );
    assert!(
        captured[0]
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("multipart/form-data; boundary="))
    );
    let body = String::from_utf8_lossy(&captured[0].body);
    assert!(body.contains("payload_json"));
    assert!(body.contains("DMR call"));
    assert!(body.contains("dmr-call-7.wav"));
    assert!(body.contains("RIFF-wave"));
}

#[tokio::test]
async fn discord_sends_decoder_events_as_json() {
    let (base, captures) = server().await;
    let message = decoded_message("chat", &decoded(), 1);
    send_webhook(
        &Client::new(),
        &format!("{base}/api/webhooks/1/token"),
        WebhookFormat::Discord,
        &message,
    )
    .await
    .expect("Discord delivery");
    let captured = captures.lock().expect("captures");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, Method::POST);
    assert!(
        captured[0]
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("application/json"))
    );
    let body: serde_json::Value = serde_json::from_slice(&captured[0].body).expect("Discord JSON");
    assert!(
        body["content"]
            .as_str()
            .is_some_and(|content| content.contains("RTTY decode") && content.contains("CQ TEST"))
    );
    assert_eq!(body["allowed_mentions"]["parse"], json!([]));
}

#[tokio::test]
async fn discord_request_errors_do_not_expose_the_webhook_url() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("local address");
    drop(listener);
    let secret = "webhook-secret";
    let message = decoded_message("chat", &decoded(), 1);

    let error = send_discord(
        &Client::new(),
        &format!("http://{address}/api/webhooks/1/{secret}"),
        &message,
    )
    .await
    .expect_err("connection failure")
    .to_string();

    assert!(error.starts_with("Discord request:"));
    assert!(!error.contains(secret));
}

#[tokio::test]
async fn matrix_creates_one_audio_event_after_uploading_the_wav() {
    let (base, captures) = server().await;
    let message = call_message(
        "chat",
        &call_record(),
        &call(),
        Some(Bytes::from_static(b"RIFF-wave")),
    );
    send_matrix(
        &Client::new(),
        MatrixTarget {
            homeserver_url: &format!("{base}/matrix/"),
            room_id: "!radio:matrix.test",
            access_token: "matrix-secret",
        },
        &message,
    )
    .await
    .expect("Matrix delivery");
    let captured = captures.lock().expect("captures");
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].method, Method::POST);
    assert!(
        captured[0]
            .uri
            .starts_with("/matrix/_matrix/media/v3/upload?filename=")
    );
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some("Bearer matrix-secret")
    );
    assert_eq!(captured[0].content_type.as_deref(), Some("audio/wav"));
    assert_eq!(captured[0].body, Bytes::from_static(b"RIFF-wave"));
    assert_eq!(captured[1].method, Method::PUT);
    assert!(captured[1].uri.contains("/matrix/_matrix/client/v3/rooms/"));
    assert!(captured[1].uri.contains("/send/m.room.message/"));
    let event: serde_json::Value = serde_json::from_slice(&captured[1].body).expect("event JSON");
    assert_eq!(event["msgtype"], "m.audio");
    assert_eq!(event["url"], "mxc://matrix.test/audio7");
    assert_eq!(event["info"]["duration"], 1_200);
    assert_eq!(event["filename"], "dmr-call-7.wav");
    assert_eq!(event["format"], MATRIX_HTML_FORMAT);
    assert!(
        event["formatted_body"]
            .as_str()
            .is_some_and(|html| html.starts_with("<strong>DMR call</strong>"))
    );
    assert!(
        event["body"]
            .as_str()
            .is_some_and(|body| body.contains("talkgroup 91") && body != "dmr-call-7.wav"),
        "body must differ from filename so clients treat it as a caption"
    );
}

#[tokio::test]
async fn matrix_sends_one_text_event_without_audio() {
    let (base, captures) = server().await;
    let homeserver_url = format!("{base}/matrix");
    let message = decoded_message("chat", &decoded(), 1);
    send_matrix(
        &Client::new(),
        MatrixTarget {
            homeserver_url: &homeserver_url,
            room_id: "!radio:matrix.test",
            access_token: "matrix-secret",
        },
        &message,
    )
    .await
    .expect("Matrix delivery");
    let captured = captures.lock().expect("captures");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, Method::PUT);
    assert!(captured[0].uri.contains("/matrix/_matrix/client/v3/rooms/"));
    assert_eq!(
        captured[0].authorization.as_deref(),
        Some("Bearer matrix-secret")
    );
    let event: serde_json::Value = serde_json::from_slice(&captured[0].body).expect("event JSON");
    assert_eq!(event["msgtype"], "m.text");
    assert!(
        event["body"]
            .as_str()
            .is_some_and(|body| body.contains("RTTY decode"))
    );
    assert!(event.get("url").is_none());
}

#[test]
fn oversized_decoder_summaries_are_bounded_and_surface_truncation() {
    let mut record = decoded();
    record.event = DecoderEvent::Rtty(RttyText {
        text: "x".repeat(MAX_MESSAGE_CHARS * 2),
    });

    let message = decoded_message("chat", &record, 1);

    assert_eq!(message.body.chars().count(), MAX_MESSAGE_CHARS);
    assert!(message.body.ends_with("… [truncated]"));
}

#[tokio::test]
async fn error_response_body_is_bounded_before_formatting() {
    let (base, _) = server().await;
    let response = Client::new()
        .get(format!("{base}/oversized-error"))
        .send()
        .await
        .expect("error response");

    let error = checked_response("Test", response)
        .await
        .expect_err("non-success response")
        .to_string();
    let (_, body) = error.split_once(": ").expect("error body");

    assert_eq!(body.len(), MAX_ERROR_BODY);
    assert!(body.bytes().all(|byte| byte == b'x'));
}

#[tokio::test]
async fn a_json_webhook_posts_the_structured_event_without_the_wav() {
    let (base, captures) = server().await;
    let message = call_message(
        "chat",
        &call_record(),
        &call(),
        Some(Bytes::from_static(b"RIFF-wave")),
    );

    send_webhook(
        &Client::new(),
        &format!("{base}/hooks/radio"),
        WebhookFormat::Json,
        &message,
    )
    .await
    .expect("webhook delivery");

    let captured = captures.lock().expect("captures");
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].method, Method::POST);
    assert_eq!(captured[0].uri, "/hooks/radio");
    assert!(
        captured[0]
            .content_type
            .as_deref()
            .is_some_and(|value| value.starts_with("application/json"))
    );
    let body: serde_json::Value = serde_json::from_slice(&captured[0].body).expect("webhook JSON");
    assert_eq!(body["output"], "chat");
    assert_eq!(body["kind"], "call");
    assert!(
        body["text"]
            .as_str()
            .is_some_and(|text| text.contains("talkgroup 91"))
    );
    assert_eq!(body["record"]["at"], "2026-08-15T10:00:02Z");
    assert_eq!(body["record"]["event"]["kind"], "call");
    assert_eq!(body["record"]["event"]["data"]["id"], 7);
    assert!(
        !String::from_utf8_lossy(&captured[0].body).contains("RIFF-wave"),
        "audio rides the Discord format only"
    );
}

#[tokio::test]
async fn a_json_webhook_carries_the_decoded_payload() {
    let (base, captures) = server().await;
    let message = decoded_message("chat", &decoded(), 1);

    send_webhook(
        &Client::new(),
        &format!("{base}/hooks/radio"),
        WebhookFormat::Json,
        &message,
    )
    .await
    .expect("webhook delivery");

    let captured = captures.lock().expect("captures");
    let body: serde_json::Value = serde_json::from_slice(&captured[0].body).expect("webhook JSON");
    assert_eq!(body["kind"], "rtty");
    assert_eq!(body["record"]["event"]["data"]["text"], "CQ TEST");
    assert!(
        body.get("content").is_none(),
        "the generic body is not a Discord payload"
    );
}

#[tokio::test]
async fn a_json_webhook_error_does_not_expose_the_endpoint() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("local address");
    drop(listener);
    let secret = "webhook-secret";
    let message = decoded_message("chat", &decoded(), 1);

    let error = send_webhook(
        &Client::new(),
        &format!("http://{address}/hooks/{secret}"),
        WebhookFormat::Json,
        &message,
    )
    .await
    .expect_err("connection failure")
    .to_string();

    assert!(error.starts_with("Webhook request:"));
    assert!(!error.contains(secret));
}

struct Published {
    topic: String,
    qos: u8,
    payload: Vec<u8>,
}

#[derive(Default)]
struct BrokerLog {
    client_id: String,
    username: Option<String>,
    password: Option<String>,
    published: Vec<Published>,
}

type Broker = Arc<Mutex<BrokerLog>>;

async fn broker() -> (String, Broker) {
    let log: Broker = Arc::new(Mutex::new(BrokerLog::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("broker listener");
    let address = listener.local_addr().expect("broker address");
    let served = log.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(serve_broker(stream, served.clone()));
        }
    });
    (format!("mqtt://{address}"), log)
}

async fn serve_broker(mut stream: tokio::net::TcpStream, log: Broker) {
    while let Some((header, body)) = read_packet(&mut stream).await {
        let reply = match header >> 4 {
            1 => {
                record_connect(&log, &body);
                vec![0x20, 0x02, 0x00, 0x00]
            }
            3 => match record_publish(&log, header, &body) {
                Some(id) => vec![0x40, 0x02, (id >> 8) as u8, id as u8],
                None => return,
            },
            12 => vec![0xD0, 0x00],
            _ => return,
        };
        if stream.write_all(&reply).await.is_err() {
            return;
        }
    }
}

async fn read_packet(stream: &mut tokio::net::TcpStream) -> Option<(u8, Vec<u8>)> {
    let mut header = [0_u8; 1];
    stream.read_exact(&mut header).await.ok()?;
    let mut length = 0_usize;
    let mut shift = 0_u32;
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.ok()?;
        length |= usize::from(byte[0] & 0x7F) << shift;
        if byte[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return None;
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.ok()?;
    Some((header[0], body))
}

fn take_string(body: &[u8], at: &mut usize) -> Option<String> {
    let length = usize::from(u16::from_be_bytes([*body.get(*at)?, *body.get(*at + 1)?]));
    let start = *at + 2;
    let end = start.checked_add(length)?;
    let text = String::from_utf8(body.get(start..end)?.to_vec()).ok()?;
    *at = end;
    Some(text)
}

fn record_connect(log: &Broker, body: &[u8]) {
    let mut at = 0;
    let Some(_protocol) = take_string(body, &mut at) else {
        return;
    };
    let Some(&flags) = body.get(at + 1) else {
        return;
    };
    at += 4;
    let Some(client_id) = take_string(body, &mut at) else {
        return;
    };
    if flags & 0x04 != 0 {
        take_string(body, &mut at);
        take_string(body, &mut at);
    }
    let username = (flags & 0x80 != 0)
        .then(|| take_string(body, &mut at))
        .flatten();
    let password = (flags & 0x40 != 0)
        .then(|| take_string(body, &mut at))
        .flatten();
    let mut log = log.lock().expect("broker log");
    log.client_id = client_id;
    log.username = username;
    log.password = password;
}

fn record_publish(log: &Broker, header: u8, body: &[u8]) -> Option<u16> {
    let mut at = 0;
    let topic = take_string(body, &mut at)?;
    let qos = (header >> 1) & 0x03;
    let id = u16::from_be_bytes([*body.get(at)?, *body.get(at + 1)?]);
    at += 2;
    log.lock().expect("broker log").published.push(Published {
        topic,
        qos,
        payload: body.get(at..)?.to_vec(),
    });
    Some(id)
}

#[tokio::test]
async fn mqtt_publishes_one_acknowledged_event_per_delivery() {
    let (broker_url, log) = broker().await;
    let message = decoded_message("chat", &decoded(), 1);

    send_mqtt(
        MqttTarget {
            broker_url: &broker_url,
            topic: "sdrmm/events",
            username: "radio",
            password: "broker-secret",
            client_id: "sdrmm-chat",
        },
        &message,
    )
    .await
    .expect("MQTT delivery");

    let log = log.lock().expect("broker log");
    assert_eq!(log.client_id, "sdrmm-chat");
    assert_eq!(log.username.as_deref(), Some("radio"));
    assert_eq!(log.password.as_deref(), Some("broker-secret"));
    assert_eq!(log.published.len(), 1);
    assert_eq!(log.published[0].topic, "sdrmm/events");
    assert_eq!(
        log.published[0].qos, 1,
        "an unacknowledged publish could be lost"
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&log.published[0].payload).expect("MQTT JSON");
    assert_eq!(payload["kind"], "rtty");
    assert_eq!(payload["record"]["event"]["data"]["text"], "CQ TEST");
}

#[tokio::test]
async fn mqtt_connects_anonymously_when_no_username_is_set() {
    let (broker_url, log) = broker().await;
    let message = decoded_message("chat", &decoded(), 1);

    send_mqtt(
        MqttTarget {
            broker_url: &broker_url,
            topic: "sdrmm/events",
            username: "",
            password: "",
            client_id: "sdrmm-chat",
        },
        &message,
    )
    .await
    .expect("MQTT delivery");

    let log = log.lock().expect("broker log");
    assert_eq!(log.username, None);
    assert_eq!(log.password, None);
}

#[tokio::test]
async fn an_unreachable_broker_surfaces_the_failure() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener");
    let address = listener.local_addr().expect("local address");
    drop(listener);
    let message = decoded_message("chat", &decoded(), 1);

    let error = send_mqtt(
        MqttTarget {
            broker_url: &format!("mqtt://{address}"),
            topic: "sdrmm/events",
            username: "",
            password: "",
            client_id: "sdrmm-chat",
        },
        &message,
    )
    .await
    .expect_err("connection failure")
    .to_string();

    assert!(error.starts_with("MQTT broker:"), "{error}");
}

#[test]
fn the_broker_url_takes_the_default_port_for_its_scheme() {
    let target = |broker_url| MqttTarget {
        broker_url,
        topic: "sdrmm/events",
        username: "",
        password: "",
        client_id: "sdrmm-chat",
    };
    for (broker_url, port) in [
        ("mqtt://broker.example", MQTT_PORT),
        ("mqtts://broker.example", MQTTS_PORT),
        ("mqtt://broker.example:1884", 1_884),
    ] {
        let options = mqtt_options(&target(broker_url)).expect("broker options");
        assert_eq!(
            options.broker().tcp_address(),
            Some(("broker.example", port)),
            "{broker_url}"
        );
    }
    assert!(mqtt_options(&target("https://broker.example")).is_err());
}

#[test]
fn the_mqtt_client_id_stays_inside_the_protocol_limit() {
    assert_eq!(mqtt_client_id("chat"), "sdrmm-chat");
    assert_eq!(
        mqtt_client_id("an-output-node-with-a-very-long-identifier").len(),
        MQTT_CLIENT_ID_LEN
    );
}
