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
        .route("/api/phones/pair", post(echo))
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
