//! Optional shared-token auth (PLAN §12, M5). One middleware covers REST, the WebSocket and
//! the MCP mount; without a configured token it is a pass-through, which is the documented
//! default posture (LAN-trusted, same as SDRangel/rtl_tcp).
//!
//! The token may arrive as `Authorization: Bearer <token>` or as a `token=` query parameter.
//! The query form is not a convenience: the browser `WebSocket` constructor cannot set request
//! headers, and the decoder-log export is a plain navigation whose `Content-Disposition` only
//! applies if the browser fetches the URL itself.

use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sdrmm_wire::ApiError;

/// Paths that stay reachable without a token. `/api/auth` is how a client learns a token is
/// needed at all; the OpenAPI document and the Swagger UI that renders it describe the API's
/// *shape*, never its data, and their browser-side fetches cannot carry a header.
const PUBLIC_PATHS: &[&str] = &["/api/auth", "/api/openapi.json"];
const PUBLIC_PREFIXES: &[&str] = &["/api/docs"];

/// The configured token, shared with the middleware layer. `None` disables auth entirely.
#[derive(Clone, Debug, Default)]
pub(crate) struct Auth {
    token: Option<std::sync::Arc<str>>,
}

impl Auth {
    pub(crate) fn new(token: Option<&str>) -> Self {
        Self {
            // An empty token is a configuration mistake that would otherwise "enable" auth
            // while accepting `?token=`; treat it as unset and say so.
            token: match token {
                Some(t) if !t.is_empty() => Some(t.into()),
                Some(_) => {
                    tracing::warn!("empty --token ignored; the server is running without auth");
                    None
                }
                None => None,
            },
        }
    }

    pub(crate) fn required(&self) -> bool {
        self.token.is_some()
    }
}

pub(crate) async fn require_token(
    State(auth): State<Auth>,
    request: Request,
    next: Next,
) -> Response {
    let Some(expected) = auth.token.as_deref() else {
        return next.run(request).await;
    };
    if is_public(request.uri().path()) {
        return next.run(request).await;
    }
    let presented = presented_token(&request);
    if presented.is_some_and(|token| token_eq(&token, expected)) {
        return next.run(request).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        Json(ApiError {
            error: "authentication required".to_string(),
            detail: Some(
                "pass the shared token as `Authorization: Bearer <token>` or `?token=<token>`"
                    .to_string(),
            ),
        }),
    )
        .into_response()
}

fn is_public(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path) || PUBLIC_PREFIXES.iter().any(|p| path.starts_with(p))
}

/// The token the request carries, header first. Returns an owned string because the query
/// form has to be percent-decoded out of a borrowed URI.
fn presented_token(request: &Request) -> Option<String> {
    if let Some(bearer) = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Some(bearer.to_string());
    }
    query_token(request.uri().query()?)
}

/// `token=` out of a raw query string. Hand-parsed rather than pulled through `serde_urlencoded`
/// so an otherwise-malformed query (which the endpoint itself will reject with a 400) cannot
/// turn into a 401 and send a client hunting for a credentials problem it does not have.
fn query_token(query: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "token")
        .map(|(_, value)| percent_decode(value))
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // Sliced as BYTES, never as `&str`: a `%` followed by a multi-byte character
            // would otherwise split it and panic — on an unauthenticated request, from the
            // network.
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    // Not a valid escape: keep it verbatim rather than dropping characters,
                    // so a mistyped token fails comparison instead of silently matching.
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Length-then-content comparison in constant time for equal lengths. The length leak is
/// inherent to any comparison and is not the interesting secret; what this prevents is the
/// byte-at-a-time early exit that makes a token guessable one character per request.
fn token_eq(presented: &str, expected: &str) -> bool {
    let (a, b) = (presented.as_bytes(), expected.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use axum::{Router, body::Body, http::Request as HttpRequest, routing::get};
    use tower::ServiceExt;

    use super::*;

    fn app(token: Option<&str>) -> Router {
        Router::new()
            .route("/api/state", get(|| async { "state" }))
            .route("/api/auth", get(|| async { "auth" }))
            .route("/api/docs/index.html", get(|| async { "docs" }))
            .route_layer(axum::middleware::from_fn_with_state(
                Auth::new(token),
                require_token,
            ))
            .fallback(|| async { "spa" })
    }

    async fn status(app: &Router, uri: &str, header: Option<&str>) -> StatusCode {
        let mut builder = HttpRequest::builder().uri(uri);
        if let Some(value) = header {
            builder = builder.header("authorization", value);
        }
        app.clone()
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("response")
            .status()
    }

    #[tokio::test]
    async fn no_token_configured_lets_everything_through() {
        let app = app(None);
        assert_eq!(status(&app, "/api/state", None).await, StatusCode::OK);
        assert_eq!(status(&app, "/", None).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_configured_token_gates_the_api_but_never_the_ui_shell() {
        let app = app(Some("s3cret"));
        assert_eq!(
            status(&app, "/api/state", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(&app, "/api/state", Some("Bearer s3cret")).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, "/api/state?token=s3cret", None).await,
            StatusCode::OK
        );
        // The login UI has to load before the user can supply a token, and the probe that
        // tells it a token is needed must answer unauthenticated.
        assert_eq!(status(&app, "/", None).await, StatusCode::OK);
        assert_eq!(status(&app, "/api/auth", None).await, StatusCode::OK);
        assert_eq!(
            status(&app, "/api/docs/index.html", None).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn wrong_tokens_are_rejected_in_every_form() {
        let app = app(Some("s3cret"));
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
    }

    #[tokio::test]
    async fn unauthorized_answers_in_the_api_error_shape() {
        let response = app(Some("s3cret"))
            .oneshot(
                HttpRequest::builder()
                    .uri("/api/state")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body");
        let err: ApiError = serde_json::from_slice(&bytes).expect("ApiError body");
        assert_eq!(err.error, "authentication required");
    }

    /// The WS handshake and the export download can only carry the token in the query, and a
    /// token with URL-unsafe bytes must survive that round trip.
    #[test]
    fn query_tokens_are_percent_decoded() {
        assert_eq!(query_token("token=a%2Fb").as_deref(), Some("a/b"));
        assert_eq!(query_token("x=1&token=a+b&y=2").as_deref(), Some("a b"));
        assert_eq!(query_token("token=100%").as_deref(), Some("100%"));
        assert_eq!(query_token("nope=1"), None);
    }

    /// A `%` followed by a multi-byte character used to slice a `&str` off a char boundary
    /// and panic — reachable from an unauthenticated request.
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
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abcd"));
        assert!(!token_eq("abcd", "abc"));
        assert!(!token_eq("", "abc"));
    }

    /// An empty token would enable the middleware while matching `?token=`; it must read as
    /// "no auth configured" instead.
    #[test]
    fn empty_token_disables_auth() {
        assert!(!Auth::new(Some("")).required());
        assert!(!Auth::new(None).required());
        assert!(Auth::new(Some("x")).required());
    }
}
