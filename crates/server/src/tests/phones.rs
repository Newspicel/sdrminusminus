use std::time::Duration;

use sdrmm_wire::{
    ErrorCode, OfferState, PairResponse, Phone, PhoneListenerState, PhoneSelf, PhonesResponse,
    ServerEvent, StateScope,
};

use super::*;
use crate::{
    auth::ListenerRole,
    phones::tests::{SERVER, endpoint, pair_one},
};

fn offer_code(state: &AppState) -> String {
    state
        .phones
        .create_offer(None, &endpoint(), SERVER, jiff::Timestamp::now())
        .expect("offer")
        .code
}

fn pair_body(code: &str) -> String {
    serde_json::json!({
        "code": code,
        "name": "Pixel",
        "platform": "android",
        "protocol": sdrmm_wire::API_PROTOCOL,
    })
    .to_string()
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<&str>,
    token: Option<&str>,
) -> (StatusCode, Bytes) {
    let bearer = token.map(|token| format!("Bearer {token}"));
    let headers: Vec<(&str, &str)> = bearer
        .as_deref()
        .map(|value| vec![("authorization", value)])
        .unwrap_or_default();
    let (status, _, bytes) = request_parts(app.clone(), method, uri, body, &headers).await;
    (status, bytes)
}

fn error_of(bytes: &Bytes) -> ApiError {
    serde_json::from_slice(bytes).expect("ApiError body")
}

async fn phones_scope(events: &mut tokio::sync::broadcast::Receiver<ServerEvent>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(ServerEvent::StateChanged {
                scope: StateScope::Phones,
            }) = events.recv().await
            {
                return;
            }
        }
    })
    .await
    .expect("a Phones scope event");
}

#[tokio::test]
async fn an_offer_needs_an_endpoint() {
    let (app, _state) = tls_router_with_state();
    let (status, body) = call(&app, "POST", "/api/phones/offers", Some("{}"), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_of(&body).error, "Turn on Allow phones");
}

#[tokio::test]
async fn phones_cannot_mint_offers_or_list_phones() {
    let (app, state) = tls_router_with_state();
    let paired = pair_one(&state.phones);
    for (method, uri, body) in [
        ("GET", "/api/phones", None),
        ("POST", "/api/phones/offers", Some("{}")),
        ("DELETE", "/api/phones/offers", None),
        (
            "PUT",
            "/api/phones/access",
            Some(r#"{"enabled":true,"port":8443}"#),
        ),
        ("DELETE", "/api/recordings/1", None),
    ] {
        let (status, answer) = call(&app, method, uri, body, Some(&paired.token)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
        let error = error_of(&answer);
        assert_eq!(
            (error.error.as_str(), error.code),
            ("Not open to phones", Some(ErrorCode::Auth))
        );
    }
    let path = format!("/api/phones/{}", paired.phone.id);
    let (status, _) = call(&app, "DELETE", &path, None, Some(&paired.token)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn pairing_and_revoke_emit_the_phones_scope() {
    let (app, state) = tls_router_with_state();
    let mut events = state.engine.subscribe_events();
    let code = offer_code(&state);
    let (status, body) = call(
        &app,
        "POST",
        "/api/phones/pair",
        Some(&pair_body(&code)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let paired: PairResponse = serde_json::from_slice(&body).expect("pair response");
    assert_eq!(paired.server_id, &*state.server_id);
    assert_eq!(paired.server_name, &*state.server_name);
    phones_scope(&mut events).await;

    let (status, body) = call(&app, "GET", "/api/phones", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let listed: PhonesResponse = serde_json::from_slice(&body).expect("phones");
    assert_eq!(listed.phones.len(), 1);
    assert!(!listed.phones[0].online);
    assert_eq!(
        listed.offer.expect("the used offer").state,
        OfferState::Used {
            phone: paired.phone.id.clone()
        }
    );
    assert_eq!(listed.access.listener, PhoneListenerState::Off);

    let path = format!("/api/phones/{}", paired.phone.id);
    let (status, _) = call(&app, "DELETE", &path, None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    phones_scope(&mut events).await;
    let (status, _) = call(&app, "GET", "/api/phones/self", None, Some(&paired.token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = call(&app, "DELETE", &path, None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_phone_reads_and_forgets_itself() {
    let (app, state) = tls_router_with_state();
    let paired = pair_one(&state.phones);
    let (status, body) = call(&app, "GET", "/api/phones/self", None, Some(&paired.token)).await;
    assert_eq!(status, StatusCode::OK);
    let me: PhoneSelf = serde_json::from_slice(&body).expect("self");
    assert_eq!(me.phone.id, paired.phone.id);
    assert_eq!(me.server_id, &*state.server_id);
    let (status, body) = call(&app, "GET", "/api/phones/self", None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(error_of(&body).error, "Not allowed");
    let (status, _) = call(
        &app,
        "DELETE",
        "/api/phones/self",
        None,
        Some(&paired.token),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(&app, "GET", "/api/phones/self", None, Some(&paired.token)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_of(&body).error, "Phone not paired");
}

#[tokio::test]
async fn a_phone_is_renamed_and_names_are_checked() {
    let (app, state) = tls_router_with_state();
    let paired = pair_one(&state.phones);
    let path = format!("/api/phones/{}", paired.phone.id);
    let (status, body) = call(&app, "PATCH", &path, Some(r#"{"name":" Car "}"#), None).await;
    assert_eq!(status, StatusCode::OK);
    let renamed: Phone = serde_json::from_slice(&body).expect("phone");
    assert_eq!(renamed.name, "Car");
    let (status, body) = call(&app, "PATCH", &path, Some(r#"{"name":""}"#), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_of(&body).error, "Name must be 1 to 64 characters");
    let (status, _) = call(
        &app,
        "PATCH",
        "/api/phones/p0000000000000000",
        Some(r#"{"name":"x"}"#),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pair_errors_answer_with_their_status() {
    let (app, state) = tls_router_with_state();
    let (status, body) = call(
        &app,
        "POST",
        "/api/phones/pair",
        Some(&pair_body("12345678")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_of(&body).error, "No pairing code is open");
    let code = offer_code(&state);
    let wrong = format!(
        "{:08}",
        (code.parse::<u32>().expect("digits") + 1) % 100_000_000
    );
    let started = std::time::Instant::now();
    let (status, body) = call(
        &app,
        "POST",
        "/api/phones/pair",
        Some(&pair_body(&wrong)),
        None,
    )
    .await;
    assert!(started.elapsed() >= Duration::from_millis(sdrmm_wire::phone::PAIR_FAILURE_DELAY_MS));
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_of(&body).error, "Wrong code, 4 tries left");
    let newer = serde_json::json!({
        "code": code,
        "name": "Pixel",
        "platform": "ios",
        "protocol": sdrmm_wire::API_PROTOCOL + 1,
    })
    .to_string();
    let (status, _) = call(&app, "POST", "/api/phones/pair", Some(&newer), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, "DELETE", "/api/phones/offers", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(&app, "DELETE", "/api/phones/offers", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn about_is_public_on_both_listeners() {
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let mut state = state_over(store);
    state.auth = crate::auth::Auth::new(Some("s3cret"));
    let main = crate::app(&state, ListenerRole::Main, true);
    let phones = crate::app(&state, ListenerRole::Phones, true);
    for app in [&main, &phones] {
        let (status, body) = call(app, "GET", "/api/about", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let about: sdrmm_wire::AboutResponse = serde_json::from_slice(&body).expect("about");
        assert_eq!(about.protocol, sdrmm_wire::API_PROTOCOL);
        let (status, _) = call(app, "GET", "/api/state", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = call(&phones, "GET", "/api/openapi.json", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&phones, "GET", "/", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&main, "GET", "/api/openapi.json", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_phone_pairs_and_connects_over_https() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("sdrmm.db");
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = Engine::with_registry(registry, None);
    let handle = crate::serve(
        Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: Some(db.clone()),
            tls: Some(crate::tls::Tls::SelfSigned {
                dir: dir.path().to_path_buf(),
                names: vec!["radio.example".to_owned()],
            }),
            options: ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    let offering = crate::phones::Phones::new(Arc::new(Store::open(Some(&db)).expect("store")));
    let code = offering
        .create_offer(None, &endpoint(), SERVER, jiff::Timestamp::now())
        .expect("offer")
        .code;
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("client");
    let base = format!("https://{}", handle.local_addr);
    let paired: PairResponse = client
        .post(format!("{base}/api/phones/pair"))
        .header("content-type", "application/json")
        .body(pair_body(&code))
        .send()
        .await
        .expect("pair")
        .json()
        .await
        .expect("pair response");
    let me = client
        .get(format!("{base}/api/phones/self"))
        .bearer_auth(&paired.token)
        .send()
        .await
        .expect("self");
    assert_eq!(me.status(), reqwest::StatusCode::OK);
    let recordings = client
        .get(format!("{base}/api/recordings"))
        .bearer_auth(&paired.token)
        .send()
        .await
        .expect("recordings");
    assert_eq!(recordings.status(), reqwest::StatusCode::FORBIDDEN);
    let plain = reqwest::Client::new()
        .get(format!("http://{}/api/phones/self", handle.local_addr))
        .bearer_auth(&paired.token)
        .send()
        .await;
    assert!(plain.is_err(), "the TLS listener answered plain HTTP");
    engine.shutdown();
}

#[tokio::test]
async fn a_re_paired_phone_keeps_its_gps_nodes() {
    let (app, state) = tls_router_with_state();
    let first = pair_one(&state.phones);
    let mut snapshot = sdrmm_wire::WorkspaceSnapshot::empty();
    snapshot.graph.nodes.push(sdrmm_wire::PatchNode {
        id: "car".to_owned(),
        body: sdrmm_wire::NodeBody::Gps(sdrmm_wire::GpsNode {
            source: Some(sdrmm_wire::PositionSource::Phone {
                phone: first.phone.id.clone(),
            }),
        }),
        position: sdrmm_wire::Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    });
    let workspace = state
        .store
        .create_workspace("field", &snapshot)
        .expect("workspace");
    state.store.activate_workspace(workspace).expect("activate");
    let rebind = serde_json::json!({
        "code": offer_code(&state),
        "name": "Pixel",
        "platform": "android",
        "protocol": sdrmm_wire::API_PROTOCOL,
        "rebind": first.token,
    })
    .to_string();
    let (status, body) = call(&app, "POST", "/api/phones/pair", Some(&rebind), None).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let again: PairResponse = serde_json::from_slice(&body).expect("pair response");
    assert_eq!(again.phone.id, first.phone.id);
    assert_eq!(again.phone.gps_nodes, ["car"]);
}
