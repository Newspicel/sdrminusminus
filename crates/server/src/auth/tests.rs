use axum::{
    Extension, Router,
    body::Body,
    http::Request as HttpRequest,
    routing::{get, post},
};
use sdrmm_wire::phone::hex;
use tower::ServiceExt;

use super::*;
use crate::{Store, phones::tests::pair_one};

fn phones() -> Arc<Phones> {
    Arc::new(Phones::new(Arc::new(
        Store::open(None).expect("in-memory store"),
    )))
}

fn gate(role: ListenerRole, tls: bool, token: Option<&str>, phones: &Arc<Phones>) -> AuthGate {
    AuthGate {
        role,
        tls,
        shared: Auth::new(token).token,
        phones: phones.clone(),
        dev_cors: false,
        local_hosts_only: false,
    }
}

async fn echo(Extension(identity): Extension<Identity>) -> String {
    format!("{identity:?}")
}

fn app(gate: AuthGate) -> Router {
    Router::new()
        .route("/api/state", get(echo))
        .route("/api/missions", get(echo))
        .route("/api/recordings", get(echo))
        .route("/api/auth", get(echo))
        .route("/api/about", get(echo))
        .route("/api/status", get(echo))
        .route("/api/phones/pair", post(echo))
        .route("/api/workspaces/1/activate", post(echo))
        .route("/api/docs/index.html", get(|| async { "docs" }))
        .route_layer(axum::middleware::from_fn_with_state(gate, authenticate))
        .fallback(|| async { "spa" })
}

fn main_app(token: Option<&str>) -> Router {
    app(gate(ListenerRole::Main, false, token, &phones()))
}

struct Answer {
    status: StatusCode,
    body: String,
    challenge: Option<String>,
}

impl Answer {
    fn error(&self) -> ApiError {
        serde_json::from_str(&self.body).expect("ApiError body")
    }
}

async fn call(app: &Router, method: &str, uri: &str, headers: &[(&str, &str)]) -> Answer {
    let mut builder = HttpRequest::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = response.status();
    let challenge = response
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("body");
    Answer {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        challenge,
    }
}

async fn status(app: &Router, uri: &str, authorization: Option<&str>) -> StatusCode {
    let headers: Vec<(&str, &str)> = authorization
        .map(|value| vec![("authorization", value)])
        .unwrap_or_default();
    call(app, "GET", uri, &headers).await.status
}

fn bearer_of(token: &str) -> String {
    format!("Bearer {token}")
}

#[tokio::test]
async fn no_token_configured_lets_everything_through() {
    let app = main_app(None);
    let open = call(&app, "GET", "/api/state", &[]).await;
    assert_eq!((open.status, open.body.as_str()), (StatusCode::OK, "Open"));
    assert_eq!(status(&app, "/", None).await, StatusCode::OK);
}

#[tokio::test]
async fn a_configured_token_gates_the_api_but_never_the_ui_shell() {
    let app = main_app(Some("s3cret"));
    assert_eq!(
        status(&app, "/api/state", None).await,
        StatusCode::UNAUTHORIZED
    );
    let operator = call(
        &app,
        "GET",
        "/api/state",
        &[("authorization", "Bearer s3cret")],
    )
    .await;
    assert_eq!(
        (operator.status, operator.body.as_str()),
        (StatusCode::OK, "Operator")
    );
    assert_eq!(
        status(&app, "/api/state?token=s3cret", None).await,
        StatusCode::OK
    );
    assert_eq!(status(&app, "/", None).await, StatusCode::OK);
    assert_eq!(status(&app, "/api/auth", None).await, StatusCode::OK);
    assert_eq!(status(&app, "/api/about", None).await, StatusCode::OK);
    assert_eq!(status(&app, "/api/status", None).await, StatusCode::OK);
    assert_eq!(
        status(&app, "/api/docs/index.html", None).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn wrong_tokens_are_rejected_in_every_form() {
    let app = main_app(Some("s3cret"));
    for uri in [
        "/api/state?token=nope",
        "/api/state?token=",
        "/api/state?other=s3cret",
    ] {
        assert_eq!(
            status(&app, uri, None).await,
            StatusCode::UNAUTHORIZED,
            "{uri}"
        );
    }
    for header in ["s3cret", "Bearer  s3cret", "Basic s3cret", "Bearer s3cre"] {
        assert_eq!(
            status(&app, "/api/state", Some(header)).await,
            StatusCode::UNAUTHORIZED,
            "{header}"
        );
    }
    let wrong = call(
        &app,
        "GET",
        "/api/state",
        &[("authorization", "Bearer nope")],
    )
    .await;
    assert_eq!(wrong.error().error, "Wrong token");
}

#[tokio::test]
async fn unauthorized_answers_in_the_api_error_shape() {
    let answer = call(&main_app(Some("s3cret")), "GET", "/api/state", &[]).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.challenge.as_deref(), Some("Bearer"));
    let error = answer.error();
    assert_eq!(error.error, "Token required");
    assert_eq!(error.code, Some(ErrorCode::Auth));
}

async fn relayed(app: &Router, uri: &str, user: &str) -> Answer {
    let response = app
        .clone()
        .oneshot(
            HttpRequest::builder()
                .uri(uri)
                .extension(sdrmm_tunnel::Relayed {
                    user: user.to_owned(),
                })
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
        .await
        .expect("body");
    Answer {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        challenge: None,
    }
}

#[tokio::test]
async fn a_relayed_user_passes_the_token_check() {
    let answer = relayed(&main_app(Some("s3cret")), "/api/state", "user-1").await;
    assert_eq!(
        (answer.status, answer.body.as_str()),
        (StatusCode::OK, "Operator")
    );
}

#[tokio::test]
async fn an_anonymous_relayed_request_still_needs_the_token() {
    let app = main_app(Some("s3cret"));
    assert_eq!(
        relayed(&app, "/api/state", "").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(relayed(&app, "/api/auth", "").await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_phone_token_opens_allowlisted_routes_only() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, Some("s3cret"), &phones));
    let key = bearer_of(&paired.token);
    let missions = call(&app, "GET", "/api/missions", &[("authorization", &key)]).await;
    assert_eq!(missions.status, StatusCode::OK);
    assert_eq!(missions.body, format!("Phone({:?})", paired.phone.id));
    let recordings = call(&app, "GET", "/api/recordings", &[("authorization", &key)]).await;
    assert_eq!(recordings.status, StatusCode::FORBIDDEN);
    let error = recordings.error();
    assert_eq!(
        (error.error.as_str(), error.code),
        ("Not open to phones", Some(ErrorCode::Auth))
    );
}

#[tokio::test]
async fn a_phone_token_in_the_query_is_refused() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, None, &phones));
    let answer = call(
        &app,
        "GET",
        &format!("/api/missions?token={}", paired.token),
        &[],
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        answer.error().error,
        "Send the phone key in the Authorization header"
    );
}

#[tokio::test]
async fn a_revoked_phone_is_refused() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, None, &phones));
    let key = bearer_of(&paired.token);
    assert_eq!(
        status(&app, "/api/missions", Some(&key)).await,
        StatusCode::OK
    );
    phones.remove(&paired.phone.id).expect("revoke");
    let answer = call(&app, "GET", "/api/missions", &[("authorization", &key)]).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.error().error, "Phone not paired");
    assert_eq!(answer.challenge.as_deref(), Some("Bearer"));
}

#[tokio::test]
async fn a_key_paired_by_another_process_is_found_in_the_store() {
    let store = Arc::new(Store::open(None).expect("in-memory store"));
    let pairing = Phones::new(store.clone());
    let paired = pair_one(&pairing);
    let serving = Arc::new(Phones::new(store));
    let app = app(gate(ListenerRole::Phones, true, None, &serving));
    assert_eq!(
        status(&app, "/api/missions", Some(&bearer_of(&paired.token))).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_unreadable_key_store_is_not_an_unpaired_phone() {
    let file = tempfile::NamedTempFile::new().expect("temp db");
    let store = Arc::new(Store::open(Some(file.path())).expect("store"));
    let paired = pair_one(&Phones::new(store.clone()));
    let serving = Arc::new(Phones::new(store));
    rusqlite::Connection::open(file.path())
        .expect("second connection")
        .execute_batch("ALTER TABLE phones RENAME TO phones_away")
        .expect("hide the keys");
    let app = app(gate(ListenerRole::Phones, true, None, &serving));
    let key = bearer_of(&paired.token);
    let answer = call(&app, "GET", "/api/missions", &[("authorization", &key)]).await;
    assert_eq!(answer.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(answer.error().code, Some(ErrorCode::Storage));
    assert!(serving.known(&paired.phone.id));
}

#[tokio::test]
async fn the_phone_listener_takes_phone_keys_only() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Phones, true, Some("s3cret"), &phones));
    for authorization in [None, Some("Bearer s3cret")] {
        let headers: Vec<(&str, &str)> = authorization
            .map(|value| vec![("authorization", value)])
            .unwrap_or_default();
        let answer = call(&app, "GET", "/api/missions", &headers).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.error().error, "Phone key required");
    }
    assert_eq!(
        status(&app, "/api/missions?token=s3cret", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status(&app, "/api/missions", Some(&bearer_of(&paired.token))).await,
        StatusCode::OK
    );
    assert_eq!(status(&app, "/api/about", None).await, StatusCode::OK);
}

#[tokio::test]
async fn an_open_server_still_names_a_phone() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, None, &phones));
    let answer = call(
        &app,
        "GET",
        "/api/state",
        &[("authorization", &bearer_of(&paired.token))],
    )
    .await;
    assert_eq!(answer.body, format!("Phone({:?})", paired.phone.id));
}

#[tokio::test]
async fn a_bad_phone_key_on_an_open_server_is_refused() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, None, &phones));
    let wrong = PhoneToken::new(paired.phone.id.clone(), [1; 32]).encode();
    let answer = call(
        &app,
        "GET",
        "/api/state",
        &[("authorization", &bearer_of(&wrong))],
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.error().error, "Phone not paired");
    let garbled = call(
        &app,
        "GET",
        "/api/state",
        &[("authorization", "Bearer sdrmm-phone.nonsense")],
    )
    .await;
    assert_eq!(garbled.status, StatusCode::UNAUTHORIZED);
    assert_eq!(garbled.error().error, "Bad credentials");
}

#[tokio::test]
async fn phone_keys_need_tls() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, false, None, &phones));
    let answer = call(
        &app,
        "GET",
        "/api/missions",
        &[("authorization", &bearer_of(&paired.token))],
    )
    .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.error().error, "Phones need HTTPS");
}

#[tokio::test]
async fn pairing_needs_tls() {
    let phones = phones();
    let plain = call(
        &app(gate(ListenerRole::Main, false, None, &phones)),
        "POST",
        "/api/phones/pair",
        &[],
    )
    .await;
    assert_eq!(plain.status, StatusCode::FORBIDDEN);
    assert_eq!(plain.error().error, "Pair over HTTPS");
    let secure = call(
        &app(gate(ListenerRole::Phones, true, Some("s3cret"), &phones)),
        "POST",
        "/api/phones/pair",
        &[],
    )
    .await;
    assert_eq!(
        (secure.status, secure.body.as_str()),
        (StatusCode::OK, "Anonymous")
    );
}

#[tokio::test]
async fn the_socket_subprotocol_carries_the_shared_token() {
    let phones = phones();
    let paired = pair_one(&phones);
    let app = app(gate(ListenerRole::Main, true, Some("s3cret"), &phones));
    let offered = format!("sdrmm, {WS_BEARER_PROTOCOL_PREFIX}{}", hex(b"s3cret"));
    let operator = call(
        &app,
        "GET",
        "/api/state",
        &[("sec-websocket-protocol", &offered)],
    )
    .await;
    assert_eq!(
        (operator.status, operator.body.as_str()),
        (StatusCode::OK, "Operator")
    );
    let phone = format!(
        "sdrmm,{WS_BEARER_PROTOCOL_PREFIX}{}",
        hex(paired.token.as_bytes())
    );
    let named = call(
        &app,
        "GET",
        "/api/state",
        &[("sec-websocket-protocol", &phone)],
    )
    .await;
    assert_eq!(named.body, format!("Phone({:?})", paired.phone.id));
    let bad = call(
        &app,
        "GET",
        "/api/state",
        &[("sec-websocket-protocol", "sdrmm, sdrmm.bearer.ZZ")],
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    assert_eq!(bad.error().error, "Bad credentials");
}

#[tokio::test]
async fn a_phone_prefixed_shared_token_is_refused_at_start() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = sdrmm_engine::Engine::with_registry(registry, None);
    let refused = crate::serve(
        crate::Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: None,
            options: crate::ServerOptions {
                token: Some("sdrmm-phone.shared".to_owned()),
                ..crate::ServerOptions::default()
            },
        },
        engine.clone(),
    )
    .await;
    match refused {
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput),
        Ok(_) => panic!("a phone-shaped token started a server"),
    }
    engine.shutdown();
}

#[tokio::test]
async fn a_served_loopback_listener_answers_only_local_names() {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = sdrmm_engine::Engine::with_registry(registry, None);
    let handle = crate::serve(
        crate::Config {
            bind: "127.0.0.1:0".parse().expect("bind"),
            db_path: None,
            tls: None,
            options: crate::ServerOptions::default(),
        },
        engine.clone(),
    )
    .await
    .expect("serve");
    let port = handle.local_addr.port();
    let state = format!("http://{}/api/state", handle.local_addr);
    let client = reqwest::Client::new();
    let status_for = |host: String| {
        let request = client.get(&state).header(header::HOST, host);
        async move { request.send().await.expect("request").status() }
    };
    assert_eq!(
        status_for(format!("localhost:{port}")).await,
        StatusCode::OK
    );
    assert_eq!(
        status_for(format!("evil.example:{port}")).await,
        StatusCode::FORBIDDEN
    );
    engine.shutdown();
}

#[test]
fn query_tokens_are_percent_decoded() {
    assert_eq!(query_token("token=a%2Fb").as_deref(), Some("a/b"));
    assert_eq!(query_token("x=1&token=a+b&y=2").as_deref(), Some("a b"));
    assert_eq!(query_token("token=100%").as_deref(), Some("100%"));
    assert_eq!(query_token("nope=1"), None);
}

#[test]
fn malformed_escapes_never_panic() {
    for query in [
        "token=%ää",
        "token=%",
        "token=%4",
        "token=%zz",
        "token=%e2%82%ac",
    ] {
        let _ = query_token(query);
    }
    assert_eq!(query_token("token=%e2%82%ac").as_deref(), Some("€"));
    assert_eq!(query_token("token=%zz").as_deref(), Some("%zz"));
}

#[test]
fn token_comparison_is_length_safe() {
    assert!(bytes_eq(b"abc", b"abc"));
    assert!(!bytes_eq(b"abc", b"abcd"));
    assert!(!bytes_eq(b"abcd", b"abc"));
    assert!(!bytes_eq(b"", b"abc"));
}

#[test]
fn empty_token_disables_auth() {
    assert!(!Auth::new(Some("")).required());
    assert!(!Auth::new(None).required());
    assert!(Auth::new(Some("x")).required());
}

#[test]
fn only_operators_administer() {
    assert!(Identity::Open.administers());
    assert!(Identity::Operator.administers());
    assert!(!Identity::Anonymous.administers());
    assert!(!Identity::Phone("p0123456789abcdef".to_owned()).administers());
    assert_eq!(
        Identity::Phone("p0123456789abcdef".to_owned()).phone(),
        Some("p0123456789abcdef")
    );
    assert_eq!(Identity::Operator.phone(), None);
}

#[tokio::test]
async fn a_foreign_page_cannot_post_to_an_open_server() {
    let app = main_app(None);
    let host = ("host", "192.168.1.20:8080");
    let foreign = call(
        &app,
        "POST",
        "/api/workspaces/1/activate",
        &[host, ("origin", "http://evil.example")],
    )
    .await;
    assert_eq!(foreign.status, StatusCode::FORBIDDEN);
    assert_eq!(foreign.error().error, "Cross-origin request refused");
    let own = call(
        &app,
        "POST",
        "/api/workspaces/1/activate",
        &[host, ("origin", "http://192.168.1.20:8080")],
    )
    .await;
    assert_eq!((own.status, own.body.as_str()), (StatusCode::OK, "Open"));
    let native = call(&app, "POST", "/api/workspaces/1/activate", &[host]).await;
    assert_eq!(native.status, StatusCode::OK);
}

#[tokio::test]
async fn dev_cors_lets_a_foreign_page_in() {
    let mut open = gate(ListenerRole::Main, false, None, &phones());
    open.dev_cors = true;
    let answer = call(
        &app(open),
        "POST",
        "/api/workspaces/1/activate",
        &[
            ("host", "127.0.0.1:8080"),
            ("origin", "http://localhost:5173"),
        ],
    )
    .await;
    assert_eq!(answer.status, StatusCode::OK);
}

async fn relayed_with(app: &Router, user: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = HttpRequest::builder()
        .uri("/api/state")
        .extension(sdrmm_tunnel::Relayed {
            user: user.to_owned(),
        });
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(Body::empty()).expect("request"))
        .await
        .expect("response")
        .status()
}

#[tokio::test]
async fn a_signed_in_relay_reaches_an_open_loopback_server() {
    let mut local = gate(ListenerRole::Main, false, None, &phones());
    local.local_hosts_only = true;
    let app = app(local);
    let relay = [
        ("host", "abc.sdrmm.link"),
        ("origin", "https://abc.sdrmm.link"),
    ];
    assert_eq!(relayed_with(&app, "user-1", &relay).await, StatusCode::OK);
    assert_eq!(relayed_with(&app, "", &relay).await, StatusCode::FORBIDDEN);
    assert_eq!(
        relayed_with(
            &app,
            "user-1",
            &[
                ("host", "abc.sdrmm.link"),
                ("origin", "https://evil.example")
            ]
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", "/api/state", &[("host", "abc.sdrmm.link")])
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn a_rebound_name_cannot_reach_an_open_loopback_server() {
    let mut local = gate(ListenerRole::Main, false, None, &phones());
    local.local_hosts_only = true;
    let app = app(local);
    for host in [
        "127.0.0.1:41234",
        "localhost:41234",
        "[::1]:41234",
        "app.localhost",
    ] {
        let answer = call(&app, "GET", "/api/state", &[("host", host)]).await;
        assert_eq!(answer.status, StatusCode::OK, "{host}");
    }
    let rebound = call(
        &app,
        "POST",
        "/api/workspaces/1/activate",
        &[
            ("host", "evil.example:41234"),
            ("origin", "http://evil.example:41234"),
        ],
    )
    .await;
    assert_eq!(rebound.status, StatusCode::FORBIDDEN);
    assert_eq!(rebound.error().error, "Host not allowed");
    let lookalike = call(
        &app,
        "GET",
        "/api/state",
        &[("host", "localhost.evil.example")],
    )
    .await;
    assert_eq!(lookalike.status, StatusCode::FORBIDDEN);
    let without_host = call(&app, "GET", "https://evil.example:41234/api/state", &[]).await;
    assert_eq!(without_host.status, StatusCode::FORBIDDEN);
    let loopback_authority = call(&app, "GET", "https://127.0.0.1:41234/api/state", &[]).await;
    assert_eq!(loopback_authority.status, StatusCode::OK);
}

fn bound_on(ip: std::net::IpAddr) -> AppState {
    let state = crate::tests::state_over(Arc::new(Store::open(None).expect("store")));
    state.gate.set_main(crate::phones::gate::MainListener {
        record: crate::phones::gate::ListenerRecord {
            role: ListenerRole::Main,
            port: 41234,
            bound: ip,
            pin: None,
            stable_key: false,
            names: Vec::new(),
        },
        own_key: None,
    });
    state
}

#[test]
fn only_a_tokenless_loopback_main_listener_checks_the_host() {
    let checks = |state: &AppState, role| AuthGate::new(state, role, false).local_hosts_only;
    let loopback = bound_on(std::net::Ipv4Addr::LOCALHOST.into());
    assert!(checks(&loopback, ListenerRole::Main));
    assert!(checks(
        &bound_on(std::net::Ipv6Addr::LOCALHOST.into()),
        ListenerRole::Main
    ));
    assert!(!checks(&loopback, ListenerRole::Phones));
    let mut guarded = loopback.clone();
    guarded.auth = Auth::new(Some("secret"));
    assert!(!checks(&guarded, ListenerRole::Main));
    let lan = bound_on(std::net::Ipv4Addr::UNSPECIFIED.into());
    assert!(!checks(&lan, ListenerRole::Main));
}
