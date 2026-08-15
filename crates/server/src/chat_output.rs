use std::{collections::HashSet, sync::Arc, time::Duration};

use axum::body::Bytes;
use reqwest::{Client, Response, Url, multipart};
use sdrmm_engine::Engine;
use sdrmm_wire::{ChatOutputTarget, NodeBody, ServerEvent, StateScope, VoiceCall};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{broadcast::error::RecvError, mpsc};

use crate::{Store, calls::Calls};

const DELIVERY_QUEUE: usize = 64;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_ERROR_BODY: usize = 1_024;

#[derive(Clone)]
struct Binding {
    node: String,
    sources: HashSet<String>,
    target: ChatOutputTarget,
}

struct Delivery {
    node: String,
    target: ChatOutputTarget,
    call: VoiceCall,
    audio: Option<Bytes>,
}

pub(crate) async fn run(engine: std::sync::Weak<Engine>, store: Arc<Store>, calls: Arc<Calls>) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut events = strong.subscribe_events();
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
    let mut bindings = load_bindings(store.clone()).await;
    loop {
        match events.recv().await {
            Ok(ServerEvent::CallCompleted(call)) => {
                for binding in bindings
                    .iter()
                    .filter(|binding| binding.sources.contains(&call.node))
                {
                    let delivery = Delivery {
                        node: binding.node.clone(),
                        target: binding.target.clone(),
                        audio: calls.audio(call.id),
                        call: (*call).clone(),
                    };
                    if let Err(error) = delivery_tx.try_send(delivery) {
                        tracing::error!(
                            output = %binding.node,
                            call = call.id,
                            %error,
                            "chat output delivery queue is full"
                        );
                    }
                }
            }
            Ok(ServerEvent::StateChanged {
                scope: StateScope::All | StateScope::Workspaces,
            }) => bindings = load_bindings(store.clone()).await,
            Ok(_) => {}
            Err(RecvError::Lagged(count)) => {
                tracing::error!(count, "chat output missed server events");
                bindings = load_bindings(store.clone()).await;
            }
            Err(RecvError::Closed) => break,
        }
    }
    drop(delivery_tx);
    let _ = worker.await;
}

async fn load_bindings(store: Arc<Store>) -> Vec<Binding> {
    match tokio::task::spawn_blocking(move || resolve(&store)).await {
        Ok(Ok(bindings)) => bindings,
        Ok(Err(error)) => {
            tracing::error!(%error, "could not resolve chat outputs");
            Vec::new()
        }
        Err(error) => {
            tracing::error!(%error, "chat output resolution panicked");
            Vec::new()
        }
    }
}

fn resolve(store: &Store) -> Result<Vec<Binding>, crate::StoreError> {
    let Some(workspace) = store.active_workspace()? else {
        return Ok(Vec::new());
    };
    let graph = &workspace.snapshot.graph;
    Ok(graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::ChatOutput(settings) = &node.body else {
                return None;
            };
            settings.target.configured().then(|| Binding {
                node: node.id.clone(),
                sources: graph
                    .sources_of(&node.id, "events")
                    .map(str::to_owned)
                    .collect(),
                target: settings.target.clone(),
            })
        })
        .collect())
}

async fn deliver_all(client: Client, mut deliveries: mpsc::Receiver<Delivery>) {
    while let Some(delivery) = deliveries.recv().await {
        let call_id = delivery.call.id;
        if let Err(error) = deliver(&client, &delivery).await {
            tracing::error!(
                output = %delivery.node,
                call = call_id,
                %error,
                "chat output delivery failed"
            );
        } else {
            tracing::info!(
                output = %delivery.node,
                call = call_id,
                "chat output delivered"
            );
        }
    }
}

async fn deliver(client: &Client, delivery: &Delivery) -> Result<(), String> {
    match &delivery.target {
        ChatOutputTarget::Discord { webhook_url } => {
            send_discord(client, webhook_url, &delivery.call, delivery.audio.clone()).await
        }
        ChatOutputTarget::Matrix {
            homeserver_url,
            room_id,
            access_token,
        } => {
            send_matrix(
                client,
                homeserver_url,
                room_id,
                access_token,
                &delivery.node,
                &delivery.call,
                delivery.audio.clone(),
            )
            .await
        }
    }
}

async fn send_discord(
    client: &Client,
    webhook_url: &str,
    call: &VoiceCall,
    audio: Option<Bytes>,
) -> Result<(), String> {
    let mut url =
        Url::parse(webhook_url).map_err(|error| format!("Discord webhook URL: {error}"))?;
    set_query(&mut url, "wait", "true");
    let payload = json!({
        "content": format_call(call),
        "allowed_mentions": { "parse": [] },
    });
    let request = client.post(url);
    let response = match audio {
        Some(audio) => {
            let filename = format!("{}-call-{}.wav", call.mode.label().to_lowercase(), call.id);
            let part = multipart::Part::bytes(audio.to_vec())
                .file_name(filename)
                .mime_str("audio/wav")
                .map_err(|error| format!("Discord audio attachment: {error}"))?;
            let form = multipart::Form::new()
                .text("payload_json", payload.to_string())
                .part("files[0]", part);
            request.multipart(form).send().await
        }
        None => request.json(&payload).send().await,
    }
    .map_err(|error| format!("Discord request: {error}"))?;
    checked("Discord", response).await
}

#[derive(Deserialize)]
struct MatrixUpload {
    content_uri: String,
}

#[allow(clippy::too_many_arguments)]
async fn send_matrix(
    client: &Client,
    homeserver_url: &str,
    room_id: &str,
    access_token: &str,
    output_node: &str,
    call: &VoiceCall,
    audio: Option<Bytes>,
) -> Result<(), String> {
    let base =
        Url::parse(homeserver_url).map_err(|error| format!("Matrix homeserver URL: {error}"))?;
    let summary = format_call(call);
    let content = match audio {
        Some(audio) => {
            let mut upload_url = base
                .join("/_matrix/media/v3/upload")
                .map_err(|error| format!("Matrix upload URL: {error}"))?;
            let filename = format!("{}-call-{}.wav", call.mode.label().to_lowercase(), call.id);
            upload_url
                .query_pairs_mut()
                .append_pair("filename", &filename);
            let response = client
                .post(upload_url)
                .bearer_auth(access_token)
                .header(reqwest::header::CONTENT_TYPE, "audio/wav")
                .body(audio.clone())
                .send()
                .await
                .map_err(|error| format!("Matrix upload: {error}"))?;
            let response = checked_response("Matrix upload", response).await?;
            let uploaded: MatrixUpload = response
                .json()
                .await
                .map_err(|error| format!("Matrix upload response: {error}"))?;
            json!({
                "msgtype": "m.audio",
                "body": summary,
                "url": uploaded.content_uri,
                "info": {
                    "duration": call.duration_ms,
                    "mimetype": "audio/wav",
                    "size": audio.len(),
                },
            })
        }
        None => json!({ "msgtype": "m.text", "body": summary }),
    };
    let transaction = matrix_transaction(output_node, call);
    let mut send_url = base;
    send_url
        .path_segments_mut()
        .map_err(|_| "Matrix homeserver URL cannot be a base URL".to_owned())?
        .clear()
        .extend([
            "_matrix",
            "client",
            "v3",
            "rooms",
            room_id,
            "send",
            "m.room.message",
            &transaction,
        ]);
    let response = client
        .put(send_url)
        .bearer_auth(access_token)
        .json(&content)
        .send()
        .await
        .map_err(|error| format!("Matrix message: {error}"))?;
    checked("Matrix message", response).await
}

fn matrix_transaction(output_node: &str, call: &VoiceCall) -> String {
    let stamp: String = call
        .ended_at
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!("sdrmm-{output_node}-{stamp}-{}", call.id)
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

fn format_call(call: &VoiceCall) -> String {
    let mut lines = vec![format!("{} call", call.mode.label())];
    lines.push(format!(
        "Source: {}",
        call.source
            .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
    ));
    lines.push(format!(
        "Destination: {}",
        call.destination.map_or_else(
            || "unknown".to_owned(),
            |id| match call.group_call {
                Some(true) => format!("talkgroup {id}"),
                Some(false) => format!("radio {id}"),
                None => id.to_string(),
            }
        )
    ));
    if let Some(slot) = call.slot {
        lines.push(format!("Timeslot: {slot}"));
    }
    if let Some(code) = call.color_code {
        lines.push(format!("Colour code: {code}"));
    }
    lines.push(format!("Frequency: {:.6} MHz", call.freq_hz / 1_000_000.0));
    lines.push(format!(
        "Duration: {:.1} s",
        call.duration_ms as f64 / 1_000.0
    ));
    lines.push(format!("Started: {}", call.started_at));
    if call.emergency {
        lines.push("Emergency: yes".to_owned());
    }
    if call.encrypted {
        lines.push("Encrypted: yes".to_owned());
    }
    if let Some(error) = &call.audio_error {
        lines.push(format!("Audio: {error}"));
    }
    lines.join("\n")
}

async fn checked(service: &str, response: Response) -> Result<(), String> {
    checked_response(service, response).await.map(|_| ())
}

async fn checked_response(service: &str, response: Response) -> Result<Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("unreadable response: {error}"));
    let body: String = body.chars().take(MAX_ERROR_BODY).collect();
    Err(format!("{service} returned {status}: {body}"))
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
    use sdrmm_wire::{DvMode, EventAudio};

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
        if uri.starts_with("/_matrix/media/v3/upload") {
            return (
                StatusCode::OK,
                r#"{"content_uri":"mxc://matrix.test/audio7"}"#,
            )
                .into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }

    #[test]
    fn summary_contains_the_complete_call_metadata() {
        let text = format_call(&call());
        assert!(text.contains("Source: 1001"));
        assert!(text.contains("Destination: talkgroup 91"));
        assert!(text.contains("Timeslot: 2"));
        assert!(text.contains("Colour code: 3"));
        assert!(text.contains("Frequency: 451.125000 MHz"));
        assert!(text.contains("Duration: 1.2 s"));
    }

    #[tokio::test]
    async fn discord_sends_metadata_and_wav_in_one_webhook_message() {
        let (base, captures) = server().await;
        send_discord(
            &Client::new(),
            &format!("{base}/api/webhooks/1/token?thread_id=5&wait=false"),
            &call(),
            Some(Bytes::from_static(b"RIFF-wave")),
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
    async fn matrix_creates_one_audio_event_after_uploading_the_wav() {
        let (base, captures) = server().await;
        send_matrix(
            &Client::new(),
            &base,
            "!radio:matrix.test",
            "matrix-secret",
            "chat",
            &call(),
            Some(Bytes::from_static(b"RIFF-wave")),
        )
        .await
        .expect("Matrix delivery");
        let captured = captures.lock().expect("captures");
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].method, Method::POST);
        assert!(
            captured[0]
                .uri
                .starts_with("/_matrix/media/v3/upload?filename=")
        );
        assert_eq!(
            captured[0].authorization.as_deref(),
            Some("Bearer matrix-secret")
        );
        assert_eq!(captured[0].content_type.as_deref(), Some("audio/wav"));
        assert_eq!(captured[0].body, Bytes::from_static(b"RIFF-wave"));
        assert_eq!(captured[1].method, Method::PUT);
        assert!(captured[1].uri.contains("/_matrix/client/v3/rooms/"));
        assert!(captured[1].uri.contains("/send/m.room.message/"));
        let event: serde_json::Value =
            serde_json::from_slice(&captured[1].body).expect("event JSON");
        assert_eq!(event["msgtype"], "m.audio");
        assert_eq!(event["url"], "mxc://matrix.test/audio7");
        assert_eq!(event["info"]["duration"], 1_200);
        assert!(
            event["body"]
                .as_str()
                .is_some_and(|body| body.contains("talkgroup 91"))
        );
    }
}
