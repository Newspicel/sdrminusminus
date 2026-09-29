use std::time::Duration;

use axum::{Json as AxumJson, routing::post};
use sdrmm_tunnel::{DeviceKey, Relayed};
use sdrmm_wire::{RemoteState, RemoteStatus};
use serde_json::json;
use tokio::net::TcpListener;

use super::*;

const DEVICE_ID: &str = "0123456789abcdefghjkmnpqrs";

fn remote_router(app: Option<url::Url>) -> (Router, Arc<Store>) {
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let options = ServerOptions {
        remote_app: app,
        ..ServerOptions::default()
    };
    let (router, background) = router_with_state(state_over(store.clone()), &options);
    background.detach();
    (router, store)
}

async fn call(app: &Router, method: &str, uri: &str, relayed: Option<&str>) -> (StatusCode, Bytes) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(user) = relayed {
        builder = builder.extension(Relayed {
            user: user.to_string(),
        });
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, body)
}

async fn remote_status(app: &Router) -> RemoteStatus {
    let (status, body) = call(app, "GET", "/api/remote", None).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("remote status")
}

async fn fake_app(relay: String) -> url::Url {
    let router = Router::new()
        .route(
            "/api/pair/start",
            post(|| async {
                AxumJson(json!({
                    "device_code": "secret",
                    "user_code": "BCDF-GHJK",
                    "verification_uri": "https://app.example/pair",
                    "verification_uri_complete": "https://app.example/pair?code=BCDF-GHJK",
                    "interval": 1,
                    "expires_in": 60,
                }))
            }),
        )
        .route(
            "/api/pair/poll",
            post(move || {
                let relay = relay.clone();
                async move {
                    AxumJson(json!({
                        "status": "approved",
                        "device_id": DEVICE_ID,
                        "relay_url": relay,
                    }))
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{address}").parse().expect("url")
}

async fn closed_port() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    drop(listener);
    format!("ws://{address}/v1/device/{DEVICE_ID}")
}

async fn wait_for_state(app: &Router, wanted: &[RemoteState]) -> RemoteStatus {
    for _ in 0..100 {
        let status = remote_status(app).await;
        if wanted.contains(&status.state) {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("remote access never reached {wanted:?}");
}

#[tokio::test]
async fn remote_access_starts_unpaired_and_knows_who_asks() {
    let (app, _) = remote_router(None);
    let status = remote_status(&app).await;
    assert_eq!(status.state, RemoteState::Unpaired);
    assert_eq!(status.app_origin, "https://app.sdrmm.com");
    assert!(!status.via_relay);
    let (_, body) = call(&app, "GET", "/api/remote", Some("user-1")).await;
    let relayed: RemoteStatus = serde_json::from_slice(&body).expect("status");
    assert!(relayed.via_relay);
}

#[tokio::test]
async fn pairing_and_unpairing_are_refused_through_the_relay() {
    let (app, _) = remote_router(None);
    for (method, uri) in [("POST", "/api/remote/pair"), ("DELETE", "/api/remote")] {
        let (status, body) = call(&app, method, uri, Some("user-1")).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        let error: ApiError = serde_json::from_slice(&body).expect("error");
        assert_eq!(error.code, Some(sdrmm_wire::ErrorCode::Forbidden));
    }
}

#[tokio::test]
async fn files_are_never_revealed_through_the_relay() {
    let (app, _) = remote_router(None);
    for uri in [
        "/api/recordings/reveal",
        "/api/recordings/1/reveal",
        "/api/audiorecordings/a.wav/reveal",
    ] {
        let (relayed, _) = call(&app, "POST", uri, Some("user-1")).await;
        assert_eq!(relayed, StatusCode::FORBIDDEN, "{uri}");
        let (local, _) = call(&app, "POST", uri, None).await;
        assert_ne!(local, StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn a_device_pairs_keeps_the_pairing_and_forgets_it_on_disconnect() {
    let app_origin = fake_app(closed_port().await).await;
    let (app, store) = remote_router(Some(app_origin));
    let (status, body) = call(&app, "POST", "/api/remote/pair", None).await;
    assert_eq!(status, StatusCode::OK);
    let pairing: RemoteStatus = serde_json::from_slice(&body).expect("status");
    assert_eq!(pairing.state, RemoteState::Pairing);
    assert_eq!(pairing.user_code.as_deref(), Some("BCDF-GHJK"));
    assert_eq!(
        pairing.verification_uri_complete.as_deref(),
        Some("https://app.example/pair?code=BCDF-GHJK")
    );
    let paired = wait_for_state(&app, &[RemoteState::Connecting, RemoteState::Retrying]).await;
    assert_eq!(paired.device_id.as_deref(), Some(DEVICE_ID));
    let kept = store.remote_pairing().expect("read").expect("pairing kept");
    assert_eq!(kept.device_id, DEVICE_ID);
    assert!(DeviceKey::from_pkcs8(&kept.key).is_ok());
    let (again, _) = call(&app, "POST", "/api/remote/pair", None).await;
    assert_eq!(again, StatusCode::CONFLICT);
    let (status, _) = call(&app, "DELETE", "/api/remote", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(remote_status(&app).await.state, RemoteState::Unpaired);
    assert_eq!(store.remote_pairing().expect("read"), None);
}

#[tokio::test]
async fn an_unreachable_app_fails_the_pairing_with_a_reason() {
    let dead: url::Url = closed_port()
        .await
        .replace("ws://", "http://")
        .parse()
        .expect("url");
    let (app, _) = remote_router(Some(dead));
    let (status, body) = call(&app, "POST", "/api/remote/pair", None).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let error: ApiError = serde_json::from_slice(&body).expect("error");
    assert!(
        error.error.contains("could not reach the app"),
        "{}",
        error.error
    );
    let status = remote_status(&app).await;
    assert_eq!(status.state, RemoteState::Unpaired);
    assert!(status.error.is_some());
}

#[tokio::test]
async fn a_kept_pairing_connects_when_the_server_starts() {
    let relay = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = relay.local_addr().expect("address");
    let store = Arc::new(Store::open(None).expect("store"));
    let (_, document) = DeviceKey::generate().expect("key");
    store
        .save_remote_pairing(&crate::RemotePairing::new(
            DEVICE_ID.to_string(),
            format!("ws://{address}/v1/device/{DEVICE_ID}"),
            document,
        ))
        .expect("save");
    let (app, background) = router_with_state(state_over(store), &ServerOptions::default());
    let (mut tcp, _) = tokio::time::timeout(Duration::from_secs(5), relay.accept())
        .await
        .expect("the tunnel dials the relay")
        .expect("accept");
    let mut first_line = vec![0u8; 64];
    let read = tokio::io::AsyncReadExt::read(&mut tcp, &mut first_line).await;
    let request = String::from_utf8_lossy(&first_line[..read.expect("read")]).into_owned();
    assert!(
        request.starts_with(&format!("GET /v1/device/{DEVICE_ID} HTTP/1.1")),
        "{request}"
    );
    assert_eq!(
        remote_status(&app).await.device_id.as_deref(),
        Some(DEVICE_ID)
    );
    drop(background);
    assert_eq!(remote_status(&app).await.state, RemoteState::Unpaired);
}
