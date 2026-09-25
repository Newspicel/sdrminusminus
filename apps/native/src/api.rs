use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Context;
use sdrmm_wire::{
    ChannelTypesResponse, CreatedRowId, DevicesResponse,
    channel::{ChannelDescriptor, ChannelSettings},
    device::{DeviceInfo, DeviceSettings},
    patch::PatchCatalog,
    rest::{ApiError, ErrorCode},
    state::StateSnapshot,
    workspace::{
        CreateWorkspaceRequest, PatchApplyReport, UpdateWorkspaceRequest, WorkspaceDetail,
        WorkspaceInfo, WorkspaceSnapshot, WorkspacesResponse,
    },
};
use serde::{Serialize, de::DeserializeOwned};

const UNAUTHORIZED: u16 = 401;

#[derive(Clone, Default)]
pub struct Token {
    held: Arc<RwLock<Option<String>>>,
    rejected: Arc<AtomicBool>,
}

impl Token {
    #[must_use]
    pub fn get(&self) -> Option<String> {
        self.held.read().ok().and_then(|held| held.clone())
    }

    pub fn set(&self, token: Option<String>) {
        if let Ok(mut held) = self.held.write() {
            *held = token;
        }
    }

    fn reject(&self) {
        self.set(None);
        self.rejected.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn take_rejected(&self) -> bool {
        self.rejected.swap(false, Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiFailure {
    pub status: u16,
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl std::fmt::Display for ApiFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiFailure {}

impl ApiFailure {
    #[must_use]
    pub fn read(status: u16, body: &str) -> Self {
        match serde_json::from_str::<ApiError>(body) {
            Ok(error) => Self {
                status,
                code: error.code,
                message: match error.detail {
                    Some(detail) if !detail.is_empty() => format!("{}: {detail}", error.error),
                    _ => error.error,
                },
            },
            Err(_) => Self {
                status,
                code: None,
                message: format!(
                    "HTTP {status}: {}",
                    body.trim().chars().take(200).collect::<String>()
                ),
            },
        }
    }
}

#[must_use]
pub fn error_code(error: &anyhow::Error) -> Option<String> {
    let failure = error.downcast_ref::<ApiFailure>()?;
    let code = serde_json::to_value(failure.code?).ok()?;
    code.as_str().map(str::to_owned)
}

#[derive(Clone)]
pub struct Api {
    base: String,
    http: reqwest::Client,
    token: Token,
}

impl Api {
    pub fn new(base: String) -> anyhow::Result<Self> {
        Ok(Self {
            base,
            http: reqwest::Client::builder()
                .build()
                .context("cannot build the http client")?,
            token: Token::default(),
        })
    }

    #[must_use]
    pub fn with_token(self, token: Token) -> Self {
        Self { token, ..self }
    }

    #[must_use]
    pub fn token(&self) -> &Token {
        &self.token
    }

    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn authorised(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.token.get() {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let response = self
            .authorised(self.http.get(&url))
            .send()
            .await
            .context(url.clone())?;
        self.body(response, &url).await
    }

    pub async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let mut request = self.authorised(self.http.request(method, &url));
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.context(url.clone())?;
        self.body(response, &url).await
    }

    pub async fn post_empty<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        self.send::<(), T>(reqwest::Method::POST, path, None).await
    }

    pub async fn post_form<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let response = self
            .authorised(self.http.post(&url))
            .multipart(form)
            .send()
            .await
            .context(url.clone())?;
        self.body(response, &url).await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    pub async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        self.send(reqwest::Method::PUT, path, Some(body)).await
    }

    pub async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> anyhow::Result<T> {
        self.send(reqwest::Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> anyhow::Result<()> {
        self.send::<(), serde::de::IgnoredAny>(reqwest::Method::DELETE, path, None)
            .await
            .map(|_| ())
    }

    pub async fn multipart<T: DeserializeOwned>(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<T> {
        self.post_form(path, form).await
    }

    pub async fn bytes(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let url = self.url(path);
        let response = self
            .authorised(self.http.get(&url))
            .send()
            .await
            .context(url.clone())?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(self.failure(status.as_u16(), &text).into());
        }
        Ok(response
            .bytes()
            .await
            .context("cannot read the response")?
            .to_vec())
    }

    #[must_use]
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn failure(&self, status: u16, text: &str) -> ApiFailure {
        if status == UNAUTHORIZED {
            self.token.reject();
        }
        ApiFailure::read(status, text)
    }

    async fn body<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
        url: &str,
    ) -> anyhow::Result<T> {
        let status = response.status();
        let text = response.text().await.context("cannot read the response")?;
        if !status.is_success() {
            return Err(self.failure(status.as_u16(), &text).into());
        }
        if text.trim().is_empty() {
            return serde_json::from_str("null").context("the reply was empty");
        }
        serde_json::from_str(&text).with_context(|| format!("{url}: cannot read {text:.200}"))
    }

    pub async fn state(&self) -> anyhow::Result<StateSnapshot> {
        self.get("/api/state").await
    }

    pub async fn devices(&self) -> anyhow::Result<Vec<DeviceInfo>> {
        Ok(self.get::<DevicesResponse>("/api/devices").await?.devices)
    }

    pub async fn channel_types(&self) -> anyhow::Result<Vec<ChannelDescriptor>> {
        Ok(self
            .get::<ChannelTypesResponse>("/api/channeltypes")
            .await?
            .types)
    }

    pub async fn catalog(&self) -> anyhow::Result<PatchCatalog> {
        self.get("/api/patch/catalog").await
    }

    pub async fn workspaces(&self) -> anyhow::Result<WorkspacesResponse> {
        self.get("/api/workspaces").await
    }

    pub async fn workspace(&self, id: i64) -> anyhow::Result<WorkspaceDetail> {
        self.get(&format!("/api/workspaces/{id}")).await
    }

    pub async fn create_workspace(&self, name: &str) -> anyhow::Result<i64> {
        let body = CreateWorkspaceRequest {
            name: name.to_owned(),
            snapshot: None,
        };
        Ok(self
            .send::<_, CreatedRowId>(reqwest::Method::POST, "/api/workspaces", Some(&body))
            .await?
            .id)
    }

    pub async fn save_workspace(
        &self,
        id: i64,
        revision: u64,
        snapshot: WorkspaceSnapshot,
    ) -> anyhow::Result<WorkspaceInfo> {
        let body = UpdateWorkspaceRequest {
            revision,
            name: None,
            snapshot: Some(snapshot),
        };
        self.send::<_, WorkspaceInfo>(
            reqwest::Method::PUT,
            &format!("/api/workspaces/{id}"),
            Some(&body),
        )
        .await
    }

    pub async fn activate_workspace(&self, id: i64) -> anyhow::Result<()> {
        self.send::<(), serde_json::Value>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/activate"),
            None,
        )
        .await
        .map(drop)
    }

    pub async fn step_history(&self, id: i64, back: bool) -> anyhow::Result<()> {
        let step = if back { "undo" } else { "redo" };
        self.send::<(), serde_json::Value>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/{step}"),
            None,
        )
        .await
        .map(drop)
    }

    pub async fn apply_workspace(&self, id: i64) -> anyhow::Result<PatchApplyReport> {
        self.send::<(), PatchApplyReport>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/apply"),
            None,
        )
        .await
    }

    pub async fn patch_device(&self, set: u32, settings: &DeviceSettings) -> anyhow::Result<()> {
        self.send::<_, serde_json::Value>(
            reqwest::Method::PATCH,
            &format!("/api/devicesets/{set}/device"),
            Some(settings),
        )
        .await
        .map(drop)
    }

    pub async fn patch_channel(
        &self,
        set: u32,
        channel: u32,
        settings: &ChannelSettings,
    ) -> anyhow::Result<()> {
        self.send::<_, serde_json::Value>(
            reqwest::Method::PATCH,
            &format!("/api/devicesets/{set}/channels/{channel}"),
            Some(settings),
        )
        .await
        .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_refusal_reads_as_its_error_and_detail_with_its_code() {
        let failure = ApiFailure::read(
            409,
            r#"{"error":"stale revision","detail":"reload","code":"conflict"}"#,
        );
        assert_eq!(failure.message, "stale revision: reload");
        assert_eq!(failure.code, Some(ErrorCode::Conflict));
        assert_eq!(
            error_code(&anyhow::Error::from(failure)).as_deref(),
            Some("conflict")
        );
    }

    #[test]
    fn a_body_that_is_not_an_api_error_is_quoted_short() {
        let failure = ApiFailure::read(502, &"x".repeat(500));
        assert_eq!(failure.message.len(), "HTTP 502: ".len() + 200);
        assert_eq!(failure.code, None);
    }

    #[test]
    fn a_rejected_token_is_forgotten_and_reported_once() {
        let token = Token::default();
        token.set(Some("s3cret".to_owned()));
        token.reject();
        assert_eq!(token.get(), None);
        assert!(token.take_rejected());
        assert!(!token.take_rejected());
    }
}
