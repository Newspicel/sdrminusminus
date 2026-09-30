use std::time::Duration;

use futures::future::BoxFuture;
use reqwest::{
    Client, RequestBuilder,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use sdrmm_wire::{
    about::AboutResponse,
    fusion::DfFusionState,
    mission::{MissionAction, MissionActionResponse, MissionsResponse, SwitchWorkspaceRequest},
    phone::{PairRequest, PairResponse, PhoneSelf},
    radar::RadarUpdate,
    rest::ApiError,
    state::StateSnapshot,
    survey::SurveyGrid,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{error::CoreError, tls::TlsConfigs};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const PAIR_TIMEOUT: Duration = Duration::from_secs(15);
const NO_ERROR_BODY: &str = "no error body";

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RestError {
    #[error("{status}: {message}")]
    Status { status: u16, message: String },
    #[error("key changed")]
    KeyMismatch { seen: String },
    #[error("timed out")]
    TimedOut,
    #[error("unreachable: {0}")]
    Unreachable(String),
    #[error("bad reply: {0}")]
    Decode(String),
}

impl RestError {
    pub(crate) fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            Self::KeyMismatch { .. } | Self::TimedOut | Self::Unreachable(_) | Self::Decode(_) => {
                None
            }
        }
    }

    pub(crate) fn into_core(self, host: &str) -> CoreError {
        match self {
            Self::Status { status: 401, .. } => CoreError::Revoked,
            Self::Status { status, message } if (400..500).contains(&status) => {
                CoreError::Refused { message }
            }
            Self::Status { status, message } => CoreError::Server { status, message },
            Self::KeyMismatch { .. } => CoreError::KeyMismatch,
            Self::TimedOut | Self::Unreachable(_) => CoreError::Unreachable {
                hosts: vec![host.to_owned()],
            },
            Self::Decode(message) => CoreError::Server {
                status: 200,
                message,
            },
        }
    }
}

pub(crate) trait Api: Send + Sync {
    fn missions(&self) -> BoxFuture<'_, Result<MissionsResponse, RestError>>;
    fn phone_self(&self) -> BoxFuture<'_, Result<PhoneSelf, RestError>>;
    fn act(
        &self,
        node: String,
        action: MissionAction,
    ) -> BoxFuture<'_, Result<MissionActionResponse, RestError>>;
    fn switch_workspace(&self, id: i64) -> BoxFuture<'_, Result<MissionsResponse, RestError>>;
    fn radar(&self, node: String) -> BoxFuture<'_, Result<RadarUpdate, RestError>>;
    fn survey(&self, node: String) -> BoxFuture<'_, Result<SurveyGrid, RestError>>;
    fn fusion(&self, node: String) -> BoxFuture<'_, Result<DfFusionState, RestError>>;
    fn state(&self) -> BoxFuture<'_, Result<StateSnapshot, RestError>>;
    fn unpair(&self) -> BoxFuture<'_, Result<(), RestError>>;
}

#[derive(Clone, Debug)]
pub(crate) struct RestClient {
    http: Client,
    base: url::Url,
}

impl RestClient {
    pub(crate) fn new(
        host: &str,
        tls: &TlsConfigs,
        token: Option<&str>,
    ) -> Result<Self, CoreError> {
        let base = url::Url::parse(&format!("https://{host}/"))
            .map_err(|_| CoreError::internal(format!("Bad host {host}")))?;
        let mut headers = HeaderMap::new();
        if let Some(token) = token {
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| CoreError::internal("Bad token"))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
        }
        let http = Client::builder()
            .tls_backend_preconfigured((*tls.rest).clone())
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .default_headers(headers)
            .build()
            .map_err(|error| CoreError::internal(format!("HTTP setup failed: {error}")))?;
        Ok(Self { http, base })
    }

    pub(crate) async fn about(&self) -> Result<AboutResponse, RestError> {
        self.get(&["api", "about"]).await
    }

    pub(crate) async fn pair(&self, request: &PairRequest) -> Result<PairResponse, RestError> {
        let url = self.url(&["api", "phones", "pair"]);
        self.send(self.http.post(url).json(request).timeout(PAIR_TIMEOUT))
            .await
    }

    fn url(&self, segments: &[&str]) -> url::Url {
        let mut url = self.base.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.clear().extend(segments);
        }
        url
    }

    async fn get<T: DeserializeOwned>(&self, segments: &[&str]) -> Result<T, RestError> {
        self.send(self.http.get(self.url(segments))).await
    }

    async fn post<B: Serialize + Sync, T: DeserializeOwned>(
        &self,
        segments: &[&str],
        body: &B,
    ) -> Result<T, RestError> {
        self.send(self.http.post(self.url(segments)).json(body))
            .await
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, RestError> {
        let response = request.send().await.map_err(|error| classify(&error))?;
        let status = response.status();
        let body = response.bytes().await.map_err(|error| classify(&error))?;
        if !status.is_success() {
            return Err(status_error(status.as_u16(), &body));
        }
        serde_json::from_slice(&body).map_err(|error| RestError::Decode(error.to_string()))
    }

    async fn delete(&self, segments: &[&str]) -> Result<(), RestError> {
        let response = self
            .http
            .delete(self.url(segments))
            .send()
            .await
            .map_err(|error| classify(&error))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.bytes().await.map_err(|error| classify(&error))?;
        Err(status_error(status.as_u16(), &body))
    }
}

impl Api for RestClient {
    fn missions(&self) -> BoxFuture<'_, Result<MissionsResponse, RestError>> {
        Box::pin(self.get(&["api", "missions"]))
    }

    fn phone_self(&self) -> BoxFuture<'_, Result<PhoneSelf, RestError>> {
        Box::pin(self.get(&["api", "phones", "self"]))
    }

    fn act(
        &self,
        node: String,
        action: MissionAction,
    ) -> BoxFuture<'_, Result<MissionActionResponse, RestError>> {
        Box::pin(async move {
            self.post(&["api", "missions", &node, "actions"], &action)
                .await
        })
    }

    fn switch_workspace(&self, id: i64) -> BoxFuture<'_, Result<MissionsResponse, RestError>> {
        Box::pin(async move {
            self.post(
                &["api", "missions", "workspace"],
                &SwitchWorkspaceRequest { workspace: id },
            )
            .await
        })
    }

    fn radar(&self, node: String) -> BoxFuture<'_, Result<RadarUpdate, RestError>> {
        Box::pin(async move { self.get(&["api", "radar", &node]).await })
    }

    fn survey(&self, node: String) -> BoxFuture<'_, Result<SurveyGrid, RestError>> {
        Box::pin(async move { self.get(&["api", "survey", &node]).await })
    }

    fn fusion(&self, node: String) -> BoxFuture<'_, Result<DfFusionState, RestError>> {
        Box::pin(async move { self.get(&["api", "fusion", &node]).await })
    }

    fn state(&self) -> BoxFuture<'_, Result<StateSnapshot, RestError>> {
        Box::pin(self.get(&["api", "state"]))
    }

    fn unpair(&self) -> BoxFuture<'_, Result<(), RestError>> {
        Box::pin(self.delete(&["api", "phones", "self"]))
    }
}

fn classify(error: &reqwest::Error) -> RestError {
    if let Some(seen) = crate::tls::pin_mismatch_in(error) {
        return RestError::KeyMismatch { seen };
    }
    if error.is_timeout() {
        return RestError::TimedOut;
    }
    RestError::Unreachable(error.to_string())
}

fn status_error(status: u16, body: &[u8]) -> RestError {
    let message = serde_json::from_slice::<ApiError>(body)
        .map(|error| error.error)
        .unwrap_or_else(|_| NO_ERROR_BODY.to_owned());
    RestError::Status { status, message }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::mission::MissionAction;

    use super::*;
    use crate::{
        stub_server::{StubRequest, StubResponse, StubServer},
        tls,
    };

    fn missions() -> MissionsResponse {
        MissionsResponse {
            revision: 7,
            workspace: None,
            workspaces: Vec::new(),
            missions: Vec::new(),
            truncated: 0,
        }
    }

    #[tokio::test]
    async fn requests_carry_the_bearer_token_and_encode_path_segments() {
        let stub = StubServer::start(|request: &StubRequest| match request.path() {
            "/api/missions" => StubResponse::json(200, &missions()),
            _ => StubResponse::error(404, "No mission a/b"),
        })
        .await;
        let configs = tls::pinned(&stub.pin).expect("tls");
        let client =
            RestClient::new(&stub.host(), &configs, Some("sdrmm-phone.x")).expect("client");
        assert_eq!(client.missions().await, Ok(missions()));
        let refused = client
            .act("a/b".to_owned(), MissionAction::Calibrate)
            .await
            .expect_err("unknown");
        assert_eq!(
            refused,
            RestError::Status {
                status: 404,
                message: "No mission a/b".to_owned()
            }
        );
        let seen = stub.requests();
        assert_eq!(
            seen[0].header("authorization"),
            Some("Bearer sdrmm-phone.x")
        );
        assert_eq!(seen[1].path(), "/api/missions/a%2Fb/actions");
        assert_eq!(seen[1].method, "POST");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&seen[1].body).expect("json"),
            serde_json::json!({"action": "calibrate"})
        );
        assert!(seen.iter().all(|request| !request.target.contains('?')));
    }

    #[tokio::test]
    async fn an_error_without_a_body_keeps_its_status() {
        let stub = StubServer::start(|_: &StubRequest| StubResponse::status(503)).await;
        let configs = tls::pinned(&stub.pin).expect("tls");
        let client = RestClient::new(&stub.host(), &configs, None).expect("client");
        let refused = client.about().await.expect_err("down");
        assert_eq!(refused.status(), Some(503));
        assert_eq!(
            refused.into_core(&stub.host()),
            CoreError::Server {
                status: 503,
                message: NO_ERROR_BODY.to_owned()
            }
        );
    }

    #[tokio::test]
    async fn a_wrong_key_is_a_key_mismatch_before_any_request() {
        let stub = StubServer::start(|_: &StubRequest| StubResponse::status(200)).await;
        let other = crate::stub_server::StubIdentity::generate();
        let configs = tls::pinned(&other.pin).expect("tls");
        let client = RestClient::new(&stub.host(), &configs, Some("secret")).expect("client");
        let refused = client.about().await.expect_err("pinned elsewhere");
        assert_eq!(
            refused,
            RestError::KeyMismatch {
                seen: stub.pin.clone()
            }
        );
        assert!(stub.requests().is_empty());
        let status_map = [
            (401, CoreError::Revoked),
            (
                409,
                CoreError::Refused {
                    message: "x".to_owned(),
                },
            ),
        ];
        for (status, expected) in status_map {
            let error = RestError::Status {
                status,
                message: "x".to_owned(),
            };
            assert_eq!(error.into_core("h:1"), expected);
        }
    }
}
