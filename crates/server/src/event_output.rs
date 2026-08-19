use std::{collections::HashMap, sync::Arc, time::Duration};

use axum::body::Bytes;
use reqwest::{Client, Response, Url, multipart};
use rumqttc::{
    AsyncClient, AsyncClientBuilder, Event, EventLoop, MqttOptions, Outgoing, Packet,
    PublishOptions, Transport,
};
use sdrmm_engine::Engine;
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, EventOutputTarget, NodeBody, ServerEvent, StateScope,
    StateSnapshot, VoiceCall, WebhookFormat,
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
const MQTT_CAPACITY: usize = 8;
const MQTT_KEEP_ALIVE_SECS: u16 = 30;
const MQTT_DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const MQTT_CLIENT_ID_LEN: usize = 23;
const MQTT_PORT: u16 = 1_883;
const MQTTS_PORT: u16 = 8_883;

#[derive(Clone)]
struct Binding {
    node: String,
    paths: Vec<EventPath>,
    target: EventOutputTarget,
}

#[derive(Default)]
struct Routing {
    bindings: Vec<Binding>,
    decoded_sources: HashMap<(u32, u32), String>,
}

struct Delivery {
    node: String,
    target: EventOutputTarget,
    event: String,
    message: OutputMessage,
}

struct OutputMessage {
    body: String,
    html: Option<String>,
    payload: serde_json::Value,
    transaction: String,
    audio: Option<OutputAudio>,
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

struct OutputAudio {
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
            tracing::error!(%error, "could not build the event output HTTP client");
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
                    tracing::error!(count, "event output missed server events");
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
                    tracing::error!(count, "event output missed decoded events");
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
                "event output delivery queue is full"
            );
        }
        Err(mpsc::error::TrySendError::Closed(delivery)) => {
            tracing::error!(
                output = %delivery.node,
                event = %delivery.event,
                "event output delivery worker has stopped"
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
                message: call_message(&binding.node, record, call, calls.audio(call.id)),
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
            tracing::error!(%error, "could not resolve event outputs");
            Routing::default()
        }
        Err(error) => {
            tracing::error!(%error, "event output resolution panicked");
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
            let NodeBody::EventOutput(settings) = &node.body else {
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
                        "event output delivered"
                    );
                    break;
                }
                Err(DeliveryError::RateLimited(wait)) if attempt < MAX_DELIVERY_ATTEMPTS => {
                    tracing::warn!(
                        output = %delivery.node,
                        event = %delivery.event,
                        wait_ms = wait.as_millis(),
                        "event output rate limited, waiting before the next attempt"
                    );
                    tokio::time::sleep(wait).await;
                }
                Err(error) => {
                    tracing::error!(
                        output = %delivery.node,
                        event = %delivery.event,
                        %error,
                        "event output delivery failed"
                    );
                    break;
                }
            }
        }
    }
}

async fn deliver(client: &Client, delivery: &Delivery) -> Result<(), DeliveryError> {
    match &delivery.target {
        EventOutputTarget::Webhook { url, format } => {
            send_webhook(client, url, *format, &delivery.message).await
        }
        EventOutputTarget::Matrix {
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
        EventOutputTarget::Mqtt {
            broker_url,
            topic,
            username,
            password,
        } => {
            send_mqtt(
                MqttTarget {
                    broker_url,
                    topic,
                    username,
                    password,
                    client_id: &mqtt_client_id(&delivery.node),
                },
                &delivery.message,
            )
            .await
        }
    }
}

async fn send_webhook(
    client: &Client,
    url: &str,
    format: WebhookFormat,
    message: &OutputMessage,
) -> Result<(), DeliveryError> {
    match format {
        WebhookFormat::Discord => send_discord(client, url, message).await,
        WebhookFormat::Json => send_json(client, url, message).await,
    }
}

async fn send_json(
    client: &Client,
    webhook_url: &str,
    message: &OutputMessage,
) -> Result<(), DeliveryError> {
    let response = client
        .post(webhook_url)
        .json(&message.payload)
        .send()
        .await
        .map_err(|error| {
            DeliveryError::Failed(format!("Webhook request: {}", error.without_url()))
        })?;
    checked("Webhook", response).await
}

async fn send_discord(
    client: &Client,
    webhook_url: &str,
    message: &OutputMessage,
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
    message: &OutputMessage,
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

struct MqttTarget<'a> {
    broker_url: &'a str,
    topic: &'a str,
    username: &'a str,
    password: &'a str,
    client_id: &'a str,
}

async fn send_mqtt(target: MqttTarget<'_>, message: &OutputMessage) -> Result<(), DeliveryError> {
    let payload = serde_json::to_vec(&message.payload)
        .map_err(|error| DeliveryError::Failed(format!("MQTT payload: {error}")))?;
    let (client, mut eventloop) = AsyncClientBuilder::new(mqtt_options(&target)?)
        .capacity(MQTT_CAPACITY)
        .try_build()
        .map_err(|error| DeliveryError::Failed(format!("MQTT client: {error}")))?;
    let acknowledged = tokio::time::timeout(REQUEST_TIMEOUT, async {
        client
            .publish(target.topic, payload, PublishOptions::at_least_once())
            .await
            .map_err(|error| DeliveryError::Failed(format!("MQTT publish: {error}")))?;
        loop {
            match eventloop.poll().await {
                Ok(Event::Incoming(Packet::PubAck(_))) => return Ok(()),
                Ok(_) => {}
                Err(error) => return Err(DeliveryError::Failed(format!("MQTT broker: {error}"))),
            }
        }
    })
    .await
    .unwrap_or_else(|_| {
        Err(DeliveryError::Failed(
            "MQTT broker did not acknowledge the publish in time".to_owned(),
        ))
    });
    mqtt_disconnect(&client, &mut eventloop).await;
    acknowledged
}

fn mqtt_options(target: &MqttTarget<'_>) -> Result<MqttOptions, DeliveryError> {
    let url = Url::parse(target.broker_url)
        .map_err(|error| DeliveryError::Failed(format!("MQTT broker URL: {error}")))?;
    let secure = match url.scheme() {
        "mqtts" => true,
        "mqtt" => false,
        scheme => {
            return Err(DeliveryError::Failed(format!(
                "MQTT broker URL uses the unsupported scheme {scheme}"
            )));
        }
    };
    let host = url
        .host_str()
        .ok_or_else(|| DeliveryError::Failed("MQTT broker URL names no host".to_owned()))?;
    let port = url
        .port()
        .unwrap_or(if secure { MQTTS_PORT } else { MQTT_PORT });
    let mut options = MqttOptions::new(target.client_id, (host, port));
    options.set_keep_alive(MQTT_KEEP_ALIVE_SECS);
    if secure {
        let transport = Transport::try_tls_with_default_config()
            .map_err(|error| DeliveryError::Failed(format!("MQTT TLS: {error}")))?;
        options.set_transport(transport);
    }
    if !target.username.is_empty() {
        options.set_credentials(target.username, target.password.to_owned());
    }
    Ok(options)
}

async fn mqtt_disconnect(client: &AsyncClient, eventloop: &mut EventLoop) {
    if client.disconnect().await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(MQTT_DISCONNECT_TIMEOUT, async {
        while let Ok(event) = eventloop.poll().await {
            if matches!(event, Event::Outgoing(Outgoing::Disconnect)) {
                break;
            }
        }
    })
    .await;
}

fn mqtt_client_id(output_node: &str) -> String {
    let mut id = "sdrmm-".to_owned();
    let room = MQTT_CLIENT_ID_LEN.saturating_sub(id.len());
    id.extend(
        output_node
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(room),
    );
    id
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

fn event_payload(output_node: &str, record: &DecodedRecord, text: &str) -> serde_json::Value {
    let encoded = serde_json::to_value(record).unwrap_or_else(|error| {
        tracing::error!(%error, "could not encode a decoded record for an event output");
        serde_json::Value::Null
    });
    json!({
        "output": output_node,
        "kind": record.event.kind(),
        "text": text,
        "record": encoded,
    })
}

fn call_message(
    output_node: &str,
    record: &DecodedRecord,
    call: &VoiceCall,
    audio: Option<Bytes>,
) -> OutputMessage {
    let body = bounded_message(format_call(call));
    OutputMessage {
        payload: event_payload(output_node, record, &body),
        html: Some(bounded_message(format_call_html(call))),
        body,
        transaction: call_transaction(output_node, call),
        audio: audio.map(|bytes| OutputAudio {
            bytes,
            filename: format!("{}-call-{}.wav", call.mode.label().to_lowercase(), call.id),
            duration_ms: call.duration_ms,
        }),
    }
}

fn decoded_message(output_node: &str, record: &DecodedRecord, sequence: u64) -> OutputMessage {
    let body = bounded_message(format_decoded(record));
    OutputMessage {
        payload: event_payload(output_node, record, &body),
        html: None,
        body,
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
mod tests;
