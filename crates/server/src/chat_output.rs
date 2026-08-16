use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::body::Bytes;
use reqwest::{Client, Response, Url, multipart};
use sdrmm_engine::Engine;
use sdrmm_wire::{
    ChatOutputTarget, DecodedRecord, DecoderEvent, NodeBody, ServerEvent, StateScope,
    StateSnapshot, VoiceCall,
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast::error::RecvError, mpsc};

use crate::{Store, calls::Calls, events::EventPath};

const DELIVERY_QUEUE: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ERROR_BODY: usize = 1_024;
const MAX_MESSAGE_CHARS: usize = 1_900;
const MAX_DELIVERY_ATTEMPTS: u32 = 4;
const DEFAULT_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const MATRIX_HTML_FORMAT: &str = "org.matrix.custom.html";

#[derive(Clone)]
struct Binding {
    node: String,
    paths: Vec<EventPath>,
    target: ChatOutputTarget,
}

#[derive(Default)]
struct Routing {
    bindings: Vec<Binding>,
    decoded_sources: HashMap<(u32, u32), String>,
}

struct Delivery {
    node: String,
    target: ChatOutputTarget,
    event: String,
    message: ChatMessage,
}

struct ChatMessage {
    body: String,
    html: Option<String>,
    transaction: String,
    audio: Option<ChatAudio>,
}

#[derive(Debug)]
enum DeliveryError {
    RateLimited(Duration),
    Failed(String),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited(wait) => write!(f, "rate limited for {} ms", wait.as_millis()),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

struct ChatAudio {
    bytes: Bytes,
    filename: String,
    duration_ms: u64,
}

pub(crate) async fn run(engine: std::sync::Weak<Engine>, store: Arc<Store>, calls: Arc<Calls>) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut events = strong.subscribe_events();
    let mut decoded = strong.subscribe_decoded();
    drop(strong);
    let client = match Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "could not build the chat output HTTP client");
            return;
        }
    };
    let (delivery_tx, delivery_rx) = mpsc::channel(DELIVERY_QUEUE);
    let worker = tokio::spawn(deliver_all(client, delivery_rx));
    let mut routing = load_routing(store.clone(), engine.clone()).await;
    let mut decoded_open = true;
    let mut decoded_sequence = 0_u64;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(ServerEvent::StateChanged {
                    scope: StateScope::All
                        | StateScope::Devices
                        | StateScope::DeviceSet(_)
                        | StateScope::Workspaces,
                }) => routing = load_routing(store.clone(), engine.clone()).await,
                Ok(_) => {}
                Err(RecvError::Lagged(count)) => {
                    tracing::error!(count, "chat output missed server events");
                    routing = load_routing(store.clone(), engine.clone()).await;
                }
                Err(RecvError::Closed) => break,
            },
            record = decoded.recv(), if decoded_open => match record {
                Ok(record) => {
                    decoded_sequence = decoded_sequence.wrapping_add(1);
                    for delivery in decoded_deliveries(&routing, &record, decoded_sequence, &calls) {
                        enqueue(&delivery_tx, delivery);
                    }
                }
                Err(RecvError::Lagged(count)) => {
                    tracing::error!(count, "chat output missed decoded events");
                }
                Err(RecvError::Closed) => decoded_open = false,
            },
        }
    }
    drop(delivery_tx);
    let _ = worker.await;
}

fn enqueue(delivery_tx: &mpsc::Sender<Delivery>, delivery: Delivery) {
    match delivery_tx.try_send(delivery) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(delivery)) => {
            tracing::error!(
                output = %delivery.node,
                event = %delivery.event,
                "chat output delivery queue is full"
            );
        }
        Err(mpsc::error::TrySendError::Closed(delivery)) => {
            tracing::error!(
                output = %delivery.node,
                event = %delivery.event,
                "chat output delivery worker has stopped"
            );
        }
    }
}

fn decoded_deliveries(
    routing: &Routing,
    record: &DecodedRecord,
    sequence: u64,
    calls: &Calls,
) -> Vec<Delivery> {
    let Some(source) = routing
        .decoded_sources
        .get(&(record.device_set, record.channel))
    else {
        return Vec::new();
    };
    routing
        .bindings
        .iter()
        .filter(|binding| {
            binding
                .paths
                .iter()
                .any(|path| path.source == *source && path.passes(&record.event))
        })
        .map(|binding| match &record.event {
            DecoderEvent::Call(call) => Delivery {
                node: binding.node.clone(),
                target: binding.target.clone(),
                event: format!("call {}", call.id),
                message: call_message(&binding.node, call, calls.audio(call.id)),
            },
            _ => Delivery {
                node: binding.node.clone(),
                target: binding.target.clone(),
                event: format!("{} decode at {}", record.event.kind(), record.at),
                message: decoded_message(&binding.node, record, sequence),
            },
        })
        .collect()
}

async fn load_routing(store: Arc<Store>, engine: std::sync::Weak<Engine>) -> Routing {
    match tokio::task::spawn_blocking(move || {
        let state = engine.upgrade().map(|engine| engine.snapshot());
        resolve(&store, state.as_ref())
    })
    .await
    {
        Ok(Ok(routing)) => routing,
        Ok(Err(error)) => {
            tracing::error!(%error, "could not resolve chat outputs");
            Routing::default()
        }
        Err(error) => {
            tracing::error!(%error, "chat output resolution panicked");
            Routing::default()
        }
    }
}

fn resolve(store: &Store, state: Option<&StateSnapshot>) -> Result<Routing, crate::StoreError> {
    let Some(workspace) = store.active_workspace()? else {
        return Ok(Routing::default());
    };
    let graph = &workspace.snapshot.graph;
    let bindings = graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::ChatOutput(settings) = &node.body else {
                return None;
            };
            settings.target.configured().then(|| Binding {
                node: node.id.clone(),
                paths: crate::events::paths_into(graph, &node.id),
                target: settings.target.clone(),
            })
        })
        .collect();
    let decoded_sources = state.map_or_else(HashMap::new, |state| {
        crate::events::decoder_nodes(graph, state)
    });
    Ok(Routing {
        bindings,
        decoded_sources,
    })
}

async fn deliver_all(client: Client, mut deliveries: mpsc::Receiver<Delivery>) {
    while let Some(delivery) = deliveries.recv().await {
        for attempt in 1..=MAX_DELIVERY_ATTEMPTS {
            match deliver(&client, &delivery).await {
                Ok(()) => {
                    tracing::info!(
                        output = %delivery.node,
                        event = %delivery.event,
                        "chat output delivered"
                    );
                    break;
                }
                Err(DeliveryError::RateLimited(wait)) if attempt < MAX_DELIVERY_ATTEMPTS => {
                    tracing::warn!(
                        output = %delivery.node,
                        event = %delivery.event,
                        wait_ms = wait.as_millis(),
                        "chat output rate limited, waiting before the next attempt"
                    );
                    tokio::time::sleep(wait).await;
                }
                Err(error) => {
                    tracing::error!(
                        output = %delivery.node,
                        event = %delivery.event,
                        %error,
                        "chat output delivery failed"
                    );
                    break;
                }
            }
        }
    }
}

async fn deliver(client: &Client, delivery: &Delivery) -> Result<(), DeliveryError> {
    match &delivery.target {
        ChatOutputTarget::Discord { webhook_url } => {
            send_discord(client, webhook_url, &delivery.message).await
        }
        ChatOutputTarget::Matrix {
            homeserver_url,
            room_id,
            access_token,
        } => {
            send_matrix(
                client,
                MatrixTarget {
                    homeserver_url,
                    room_id,
                    access_token,
                },
                &delivery.message,
            )
            .await
        }
    }
}

async fn send_discord(
    client: &Client,
    webhook_url: &str,
    message: &ChatMessage,
) -> Result<(), DeliveryError> {
    let mut url = Url::parse(webhook_url)
        .map_err(|error| DeliveryError::Failed(format!("Discord webhook URL: {error}")))?;
    set_query(&mut url, "wait", "true");
    let payload = json!({
        "content": message.body,
        "allowed_mentions": { "parse": [] },
    });
    let request = client.post(url);
    let response = match &message.audio {
        Some(audio) => {
            let part = multipart::Part::bytes(audio.bytes.to_vec())
                .file_name(audio.filename.clone())
                .mime_str("audio/wav")
                .map_err(|error| {
                    DeliveryError::Failed(format!("Discord audio attachment: {error}"))
                })?;
            let form = multipart::Form::new()
                .text("payload_json", payload.to_string())
                .part("files[0]", part);
            request.multipart(form).send().await
        }
        None => request.json(&payload).send().await,
    }
    .map_err(|error| DeliveryError::Failed(format!("Discord request: {}", error.without_url())))?;
    checked("Discord", response).await
}

#[derive(Deserialize)]
struct MatrixUpload {
    content_uri: String,
}

#[derive(Clone, Copy)]
struct MatrixTarget<'a> {
    homeserver_url: &'a str,
    room_id: &'a str,
    access_token: &'a str,
}

async fn send_matrix(
    client: &Client,
    target: MatrixTarget<'_>,
    message: &ChatMessage,
) -> Result<(), DeliveryError> {
    let base = Url::parse(target.homeserver_url)
        .map_err(|error| DeliveryError::Failed(format!("Matrix homeserver URL: {error}")))?;
    let mut content = match &message.audio {
        Some(audio) => {
            let mut upload_url = matrix_url(&base, &["_matrix", "media", "v3", "upload"])
                .map_err(|error| DeliveryError::Failed(format!("Matrix upload URL: {error}")))?;
            upload_url
                .query_pairs_mut()
                .append_pair("filename", &audio.filename);
            let response = client
                .post(upload_url)
                .bearer_auth(target.access_token)
                .header(reqwest::header::CONTENT_TYPE, "audio/wav")
                .body(audio.bytes.clone())
                .send()
                .await
                .map_err(|error| {
                    DeliveryError::Failed(format!("Matrix upload: {}", error.without_url()))
                })?;
            let response = checked_response("Matrix upload", response).await?;
            let uploaded: MatrixUpload = response.json().await.map_err(|error| {
                DeliveryError::Failed(format!("Matrix upload response: {}", error.without_url()))
            })?;
            json!({
                "msgtype": "m.audio",
                "body": message.body,
                "filename": audio.filename,
                "url": uploaded.content_uri,
                "info": {
                    "duration": audio.duration_ms,
                    "mimetype": "audio/wav",
                    "size": audio.bytes.len(),
                },
            })
        }
        None => json!({ "msgtype": "m.text", "body": message.body }),
    };
    if let Some(html) = &message.html {
        content["format"] = json!(MATRIX_HTML_FORMAT);
        content["formatted_body"] = json!(html);
    }
    let send_url = matrix_url(
        &base,
        &[
            "_matrix",
            "client",
            "v3",
            "rooms",
            target.room_id,
            "send",
            "m.room.message",
            &message.transaction,
        ],
    )
    .map_err(|error| DeliveryError::Failed(format!("Matrix message URL: {error}")))?;
    let response = client
        .put(send_url)
        .bearer_auth(target.access_token)
        .json(&content)
        .send()
        .await
        .map_err(|error| {
            DeliveryError::Failed(format!("Matrix message: {}", error.without_url()))
        })?;
    checked("Matrix message", response).await
}

fn matrix_url(base: &Url, segments: &[&str]) -> Result<Url, String> {
    let mut url = base.clone();
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| "homeserver cannot be used as a base URL".to_owned())?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

fn call_transaction(output_node: &str, call: &VoiceCall) -> String {
    let stamp = transaction_stamp(&call.ended_at);
    format!("sdrmm-{output_node}-{stamp}-{}", call.id)
}

fn decoded_transaction(output_node: &str, record: &DecodedRecord, sequence: u64) -> String {
    let stamp = transaction_stamp(&record.at);
    format!(
        "sdrmm-{output_node}-{stamp}-{}-{}-{}-{sequence}",
        record.device_set,
        record.channel,
        record.event.kind()
    )
}

fn transaction_stamp(at: &str) -> String {
    at.chars().filter(char::is_ascii_alphanumeric).collect()
}

fn set_query(url: &mut Url, name: &str, value: &str) {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != name)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    pairs.push((name.to_owned(), value.to_owned()));
    url.set_query(None);
    url.query_pairs_mut().extend_pairs(pairs);
}

fn call_parts(call: &VoiceCall) -> Vec<String> {
    let mut parts = vec![format!("{} call", call.mode.label())];
    parts.push(call.destination.map_or_else(
        || "to unknown".to_owned(),
        |id| match call.group_call {
            Some(true) => format!("talkgroup {id}"),
            Some(false) => format!("radio {id}"),
            None => format!("to {id}"),
        },
    ));
    parts.push(
        call.source
            .map_or_else(|| "from unknown".to_owned(), |id| format!("from {id}")),
    );
    if let Some(slot) = call.slot {
        parts.push(format!("TS{slot}"));
    }
    if let Some(code) = call.color_code {
        parts.push(format!("CC{code}"));
    }
    parts.push(format!("{:.1} s", call.duration_ms as f64 / 1_000.0));
    parts.push(format!("{:.6} MHz", call.freq_hz / 1_000_000.0));
    if call.emergency {
        parts.push("emergency".to_owned());
    }
    if call.encrypted {
        parts.push("encrypted".to_owned());
    }
    parts
}

fn format_call(call: &VoiceCall) -> String {
    let mut text = call_parts(call).join(" · ");
    if let Some(error) = &call.audio_error {
        text.push_str(&format!("\nAudio: {error}"));
    }
    text
}

fn format_call_html(call: &VoiceCall) -> String {
    let parts = call_parts(call);
    let (mode, rest) = parts.split_at(1);
    let mut html = format!("<strong>{}</strong>", escape_html(&mode[0]));
    for part in rest {
        html.push_str(" · ");
        html.push_str(&escape_html(part));
    }
    if let Some(error) = &call.audio_error {
        html.push_str(&format!("<br/><em>{}</em>", escape_html(error)));
    }
    html
}

fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn format_decoded(record: &DecodedRecord) -> String {
    let mut lines = vec![format!(
        "{} decode",
        record.event.kind().replace('_', " ").to_uppercase()
    )];
    let summary = record.event.summary();
    if !summary.trim().is_empty() {
        lines.push(summary);
    }
    if let Some(station) = record.event.station() {
        lines.push(format!("Station: {station}"));
    }
    lines.push(format!(
        "Frequency: {:.6} MHz",
        record.freq_hz / 1_000_000.0
    ));
    lines.push(format!("Received: {}", record.at));
    lines.join("\n")
}

fn call_message(output_node: &str, call: &VoiceCall, audio: Option<Bytes>) -> ChatMessage {
    ChatMessage {
        body: bounded_message(format_call(call)),
        html: Some(bounded_message(format_call_html(call))),
        transaction: call_transaction(output_node, call),
        audio: audio.map(|bytes| ChatAudio {
            bytes,
            filename: format!("{}-call-{}.wav", call.mode.label().to_lowercase(), call.id),
            duration_ms: call.duration_ms,
        }),
    }
}

fn decoded_message(output_node: &str, record: &DecodedRecord, sequence: u64) -> ChatMessage {
    ChatMessage {
        body: bounded_message(format_decoded(record)),
        html: None,
        transaction: decoded_transaction(output_node, record, sequence),
        audio: None,
    }
}

fn bounded_message(message: String) -> String {
    let suffix = "\n… [truncated]";
    if message.chars().count() <= MAX_MESSAGE_CHARS {
        return message;
    }
    let keep = MAX_MESSAGE_CHARS.saturating_sub(suffix.chars().count());
    let mut bounded: String = message.chars().take(keep).collect();
    bounded.push_str(suffix);
    bounded
}

async fn checked(service: &str, response: Response) -> Result<(), DeliveryError> {
    checked_response(service, response).await.map(|_| ())
}

async fn checked_response(
    service: &str,
    mut response: Response,
) -> Result<Response, DeliveryError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let header_delay = retry_after_header(&response);
    let mut body = Vec::with_capacity(MAX_ERROR_BODY);
    while body.len() < MAX_ERROR_BODY {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(error) => {
                return Err(DeliveryError::Failed(format!(
                    "{service} returned {status}: unreadable response: {}",
                    error.without_url()
                )));
            }
        };
        let remaining = MAX_ERROR_BODY - body.len();
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let body = String::from_utf8_lossy(&body);
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let delay = header_delay
            .or_else(|| retry_after_body(&body))
            .unwrap_or(DEFAULT_RETRY_DELAY)
            .min(MAX_RETRY_DELAY);
        return Err(DeliveryError::RateLimited(delay));
    }
    Err(DeliveryError::Failed(format!(
        "{service} returned {status}: {body}"
    )))
}

fn retry_after_header(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<f64>().ok())
        .and_then(seconds_to_delay)
}

fn retry_after_body(body: &str) -> Option<Duration> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(ms) = parsed
        .get("retry_after_ms")
        .and_then(serde_json::Value::as_f64)
    {
        return seconds_to_delay(ms / 1_000.0);
    }
    parsed
        .get("retry_after")
        .and_then(serde_json::Value::as_f64)
        .and_then(seconds_to_delay)
}

fn seconds_to_delay(seconds: f64) -> Option<Duration> {
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
}

#[cfg(test)]
mod tests {
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
        ChannelNode, ChatOutputNode, DecoderEvent, DvMode, EventAudio, EventFilterNode, PatchEdge,
        PatchGraph, PatchNode, PortRef, Position, RackLayout, RttyText, UpdateWorkspaceRequest,
        WorkspaceSnapshot,
    };

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
        let configured = ChatOutputTarget::Discord {
            webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
        };
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
                    NodeBody::ChatOutput(ChatOutputNode {
                        target: configured.clone(),
                    }),
                ),
                node("empty", NodeBody::ChatOutput(ChatOutputNode::default())),
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
                target: ChatOutputTarget::Discord {
                    webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
                },
            }],
            decoded_sources: HashMap::from([((1, 2), "decoder".to_owned())]),
        }
    }

    #[test]
    fn decoded_records_route_only_to_outputs_wired_to_the_live_channel() {
        let target = ChatOutputTarget::Discord {
            webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
        };
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
        let message = call_message("chat", &call(), Some(Bytes::from_static(b"RIFF-wave")));
        send_discord(
            &Client::new(),
            &format!("{base}/api/webhooks/1/token?thread_id=5&wait=false"),
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
        send_discord(
            &Client::new(),
            &format!("{base}/api/webhooks/1/token"),
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
        let body: serde_json::Value =
            serde_json::from_slice(&captured[0].body).expect("Discord JSON");
        assert!(
            body["content"].as_str().is_some_and(
                |content| content.contains("RTTY decode") && content.contains("CQ TEST")
            )
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
        let message = call_message("chat", &call(), Some(Bytes::from_static(b"RIFF-wave")));
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
        let event: serde_json::Value =
            serde_json::from_slice(&captured[1].body).expect("event JSON");
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
        let event: serde_json::Value =
            serde_json::from_slice(&captured[0].body).expect("event JSON");
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
}
