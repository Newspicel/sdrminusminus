use std::sync::Arc;

use axum::{
    Json,
    extract::{MatchedPath, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sdrmm_wire::{
    ApiError, ErrorCode, PhoneToken, WS_BEARER_PROTOCOL_PREFIX,
    phone::{PHONE_TOKEN_PREFIX, unhex},
};

use crate::{
    AppState,
    phones::{Phones, Verified, phone_may},
};

const PUBLIC_PATHS: &[&str] = &["/api/auth", "/api/about", "/api/openapi.json"];
const PUBLIC_PREFIXES: &[&str] = &["/api/docs"];
const PAIR_PATH: &str = "/api/phones/pair";

#[derive(Clone, Debug, Default)]
pub(crate) struct Auth {
    token: Option<Arc<str>>,
}

impl Auth {
    pub(crate) fn new(token: Option<&str>) -> Self {
        Self {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Identity {
    Anonymous,
    Open,
    Operator,
    Phone(String),
}

impl Identity {
    pub(crate) fn phone(&self) -> Option<&str> {
        match self {
            Self::Phone(id) => Some(id),
            Self::Anonymous | Self::Open | Self::Operator => None,
        }
    }

    pub(crate) fn administers(&self) -> bool {
        matches!(self, Self::Open | Self::Operator)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListenerRole {
    Main,
    Phones,
}

#[derive(Clone)]
pub(crate) struct AuthGate {
    role: ListenerRole,
    tls: bool,
    shared: Option<Arc<str>>,
    phones: Arc<Phones>,
}

impl AuthGate {
    pub(crate) fn new(state: &AppState, role: ListenerRole, tls: bool) -> Self {
        Self {
            role,
            tls,
            shared: state.auth.token.clone(),
            phones: state.phones.clone(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Refusal {
    status: StatusCode,
    message: &'static str,
}

pub(crate) fn unauthorized(message: &'static str) -> Refusal {
    Refusal {
        status: StatusCode::UNAUTHORIZED,
        message,
    }
}

pub(crate) fn forbidden(message: &'static str) -> Refusal {
    Refusal {
        status: StatusCode::FORBIDDEN,
        message,
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let body = Json(ApiError {
            error: self.message.to_owned(),
            detail: None,
            code: Some(ErrorCode::Auth),
        });
        if self.status == StatusCode::UNAUTHORIZED {
            (self.status, [(header::WWW_AUTHENTICATE, "Bearer")], body).into_response()
        } else {
            (self.status, body).into_response()
        }
    }
}

enum Credential {
    Shared(String),
    Phone(PhoneToken),
    PhoneInQuery,
    Malformed,
}

enum Claim {
    Decided(Result<Identity, Refusal>),
    Phone(PhoneToken),
}

pub(crate) async fn authenticate(
    State(gate): State<AuthGate>,
    mut request: Request,
    next: Next,
) -> Response {
    let identity = match gate.claim(&request) {
        Claim::Decided(decided) => decided,
        Claim::Phone(token) => gate.phone(token).await,
    };
    let identity = match identity {
        Ok(identity) => identity,
        Err(refusal) => return refusal.into_response(),
    };
    if let Identity::Phone(id) = &identity {
        if !phone_may(request.method(), request.extensions().get::<MatchedPath>()) {
            return forbidden("Not open to phones").into_response();
        }
        gate.phones.touch(id);
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

impl AuthGate {
    fn claim(&self, request: &Request) -> Claim {
        let path = request.uri().path();
        if path == PAIR_PATH {
            return Claim::Decided(if self.tls {
                Ok(Identity::Anonymous)
            } else {
                Err(forbidden("Pair over HTTPS"))
            });
        }
        if is_public(path) {
            return Claim::Decided(Ok(Identity::Anonymous));
        }
        Claim::Decided(match credential(request, self.role) {
            Some(Credential::Malformed) => Err(unauthorized("Bad credentials")),
            Some(Credential::PhoneInQuery) => Err(unauthorized(
                "Send the phone key in the Authorization header",
            )),
            Some(Credential::Phone(token)) => return Claim::Phone(token),
            Some(Credential::Shared(shared)) => self.shared(Some(&shared)),
            None => self.shared(None),
        })
    }

    async fn phone(&self, token: PhoneToken) -> Result<Identity, Refusal> {
        if !self.tls {
            return Err(unauthorized("Phones need HTTPS"));
        }
        let paired = match self.phones.verify_cached(&token) {
            Verified::Paired => true,
            Verified::Refused => false,
            Verified::Unknown => self.verify_stored(token.clone()).await,
        };
        if paired {
            Ok(Identity::Phone(token.phone))
        } else {
            Err(unauthorized("Phone not paired"))
        }
    }

    async fn verify_stored(&self, token: PhoneToken) -> bool {
        let phones = self.phones.clone();
        match tokio::task::spawn_blocking(move || phones.verify(&token)).await {
            Ok(paired) => paired,
            Err(error) => {
                tracing::warn!(%error, "checking a phone key stopped");
                false
            }
        }
    }

    fn shared(&self, presented: Option<&str>) -> Result<Identity, Refusal> {
        if self.role == ListenerRole::Phones {
            return Err(unauthorized("Phone key required"));
        }
        match (self.shared.as_deref(), presented) {
            (None, _) => Ok(Identity::Open),
            (Some(expected), Some(presented))
                if bytes_eq(presented.as_bytes(), expected.as_bytes()) =>
            {
                Ok(Identity::Operator)
            }
            (Some(_), Some(_)) => Err(unauthorized("Wrong token")),
            (Some(_), None) => Err(unauthorized("Token required")),
        }
    }
}

fn is_public(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path) || PUBLIC_PREFIXES.iter().any(|p| path.starts_with(p))
}

fn credential(request: &Request, role: ListenerRole) -> Option<Credential> {
    let headers = request.headers();
    if let Some(bearer) = bearer(headers) {
        return Some(classify(bearer.to_owned()));
    }
    if let Some(offered) = subprotocol_credential(headers) {
        return Some(offered);
    }
    if role != ListenerRole::Main {
        return None;
    }
    let token = query_token(request.uri().query()?)?;
    Some(if token.starts_with(PHONE_TOKEN_PREFIX) {
        Credential::PhoneInQuery
    } else {
        Credential::Shared(token)
    })
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn subprotocol_credential(headers: &HeaderMap) -> Option<Credential> {
    let encoded = headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .find_map(|offered| offered.strip_prefix(WS_BEARER_PROTOCOL_PREFIX))?;
    Some(
        unhex(encoded)
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map_or(Credential::Malformed, classify),
    )
}

fn classify(text: String) -> Credential {
    if text.starts_with(PHONE_TOKEN_PREFIX) {
        PhoneToken::parse(&text).map_or(Credential::Malformed, Credential::Phone)
    } else {
        Credential::Shared(text)
    }
}

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
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
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

pub(crate) fn bytes_eq(a: &[u8], b: &[u8]) -> bool {
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
mod tests;
