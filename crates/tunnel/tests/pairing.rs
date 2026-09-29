#![allow(clippy::expect_used)]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use sdrmm_tunnel::{
    DeviceKey,
    pairing::{Pairing, PairingError, PollRequest, StartRequest, public_key_text},
};
use serde_json::json;
use tokio::net::TcpListener;

const DEVICE_ID: &str = "0123456789abcdefghjkmnpqrs";

#[derive(Clone)]
struct App {
    started: Arc<Mutex<Option<StartRequest>>>,
    polls: Arc<AtomicUsize>,
    answers: Arc<Vec<(StatusCode, serde_json::Value)>>,
    start: Arc<(StatusCode, serde_json::Value)>,
}

impl App {
    fn new(
        start: (StatusCode, serde_json::Value),
        answers: Vec<(StatusCode, serde_json::Value)>,
    ) -> Self {
        Self {
            started: Arc::new(Mutex::new(None)),
            polls: Arc::new(AtomicUsize::new(0)),
            answers: Arc::new(answers),
            start: Arc::new(start),
        }
    }

    async fn serve(&self) -> url::Url {
        let router = Router::new()
            .route("/api/pair/start", post(start))
            .route("/api/pair/poll", post(poll))
            .with_state(self.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, router).await });
        format!("http://{address}").parse().expect("url")
    }
}

async fn start(State(app): State<App>, Json(request): Json<StartRequest>) -> Response {
    *app.started.lock().expect("lock") = Some(request);
    let (status, body) = app.start.as_ref().clone();
    (status, Json(body)).into_response()
}

async fn poll(State(app): State<App>, Json(request): Json<PollRequest>) -> Response {
    assert_eq!(request.device_code, "secret-code");
    let index = app.polls.fetch_add(1, Ordering::SeqCst);
    let last = app.answers.len() - 1;
    let (status, body) = app.answers[index.min(last)].clone();
    (status, Json(body)).into_response()
}

fn started(expires_in: u64) -> (StatusCode, serde_json::Value) {
    (
        StatusCode::OK,
        json!({
            "device_code": "secret-code",
            "user_code": "BCDF-GHJK",
            "verification_uri": "https://app.sdrmm.com/pair",
            "verification_uri_complete": "https://app.sdrmm.com/pair?code=BCDF-GHJK",
            "interval": 1,
            "expires_in": expires_in,
        }),
    )
}

fn pending() -> (StatusCode, serde_json::Value) {
    (StatusCode::OK, json!({"status": "pending"}))
}

fn approved() -> (StatusCode, serde_json::Value) {
    (
        StatusCode::OK,
        json!({
            "status": "approved",
            "device_id": DEVICE_ID,
            "relay_url": format!("wss://sdrmm.link/v1/device/{DEVICE_ID}"),
        }),
    )
}

#[tokio::test]
async fn a_device_pairs_after_the_user_approves() {
    let app = App::new(started(900), vec![pending(), pending(), approved()]);
    let origin = app.serve().await;
    let (key, _) = DeviceKey::generate().expect("key");
    let pairing = Pairing::start(&origin, &key, "  shack pi  ")
        .await
        .expect("start");
    assert_eq!(pairing.started().user_code, "BCDF-GHJK");
    assert_eq!(
        pairing.started().verification_uri_complete,
        "https://app.sdrmm.com/pair?code=BCDF-GHJK"
    );
    let sent = app
        .started
        .lock()
        .expect("lock")
        .clone()
        .expect("start request");
    assert_eq!(sent.name, "shack pi");
    assert_eq!(sent.public_key, public_key_text(&key).expect("public key"));
    assert_eq!(sent.public_key.len(), 43);
    let paired = pairing.wait().await.expect("paired");
    assert_eq!(paired.device_id, DEVICE_ID);
    assert_eq!(
        paired.relay_url.as_str(),
        format!("wss://sdrmm.link/v1/device/{DEVICE_ID}")
    );
    assert_eq!(app.polls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn an_expired_code_ends_the_wait() {
    let app = App::new(
        started(900),
        vec![pending(), (StatusCode::GONE, json!({"status": "expired"}))],
    );
    let origin = app.serve().await;
    let (key, _) = DeviceKey::generate().expect("key");
    let pairing = Pairing::start(&origin, &key, "pi").await.expect("start");
    assert!(matches!(pairing.wait().await, Err(PairingError::Expired)));
}

#[tokio::test]
async fn the_wait_ends_when_the_code_runs_out_even_if_the_app_never_says_so() {
    let app = App::new(started(1), vec![pending()]);
    let origin = app.serve().await;
    let (key, _) = DeviceKey::generate().expect("key");
    let pairing = Pairing::start(&origin, &key, "pi").await.expect("start");
    assert!(matches!(pairing.wait().await, Err(PairingError::Expired)));
}

#[tokio::test]
async fn server_errors_while_polling_are_retried() {
    let app = App::new(
        started(900),
        vec![
            (StatusCode::BAD_GATEWAY, json!({"error": "blip"})),
            approved(),
        ],
    );
    let origin = app.serve().await;
    let (key, _) = DeviceKey::generate().expect("key");
    let pairing = Pairing::start(&origin, &key, "pi").await.expect("start");
    assert_eq!(pairing.wait().await.expect("paired").device_id, DEVICE_ID);
}

#[tokio::test]
async fn a_refused_start_reports_the_apps_reason() {
    let app = App::new(
        (StatusCode::BAD_REQUEST, json!({"error": "bad public key"})),
        vec![pending()],
    );
    let origin = app.serve().await;
    let (key, _) = DeviceKey::generate().expect("key");
    let error = Pairing::start(&origin, &key, "pi")
        .await
        .expect_err("refused");
    assert_eq!(error.to_string(), "the app refused: bad public key");
}

#[tokio::test]
async fn an_unreachable_app_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let origin: url::Url = format!("http://{}", listener.local_addr().expect("address"))
        .parse()
        .expect("url");
    drop(listener);
    let (key, _) = DeviceKey::generate().expect("key");
    assert!(matches!(
        Pairing::start(&origin, &key, "pi").await,
        Err(PairingError::Unreachable(_))
    ));
}
