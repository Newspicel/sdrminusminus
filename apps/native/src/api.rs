use anyhow::{Context, bail};
use sdrmm_wire::{
    ChannelTypesResponse, CreatedRowId, DevicesResponse,
    channel::{ChannelDescriptor, ChannelSettings},
    device::{DeviceInfo, DeviceSettings},
    patch::PatchCatalog,
    state::StateSnapshot,
    workspace::{
        CreateWorkspaceRequest, PatchApplyReport, UpdateWorkspaceRequest, WorkspaceDetail,
        WorkspaceInfo, WorkspaceSnapshot, WorkspacesResponse,
    },
};
use serde::{Serialize, de::DeserializeOwned};

#[derive(Clone)]
pub struct Api {
    base: String,
    http: reqwest::Client,
}

impl Api {
    pub fn new(base: String) -> anyhow::Result<Self> {
        Ok(Self {
            base,
            http: reqwest::Client::builder()
                .build()
                .context("cannot build the http client")?,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let response = self.http.get(&url).send().await.context(url.clone())?;
        Self::body(response, &url).await
    }

    pub async fn send<B: Serialize, T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let mut request = self.http.request(method, &url);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.context(url.clone())?;
        Self::body(response, &url).await
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

    pub async fn bytes(&self, path: &str) -> anyhow::Result<Vec<u8>> {
        let url = self.url(path);
        let response = self.http.get(&url).send().await.context(url.clone())?;
        let status = response.status();
        if !status.is_success() {
            bail!("{url}: {status}");
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

    async fn body<T: DeserializeOwned>(
        response: reqwest::Response,
        url: &str,
    ) -> anyhow::Result<T> {
        let status = response.status();
        let text = response.text().await.context("cannot read the response")?;
        if !status.is_success() {
            bail!("{url}: {status}: {}", text.trim());
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
