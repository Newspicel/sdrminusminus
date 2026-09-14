use anyhow::{Context, bail};
use sdrmm_wire::{
    channel::{ChannelDescriptor, ChannelSettings},
    device::{DeviceInfo, DeviceSettings},
    patch::PatchCatalog,
    state::StateSnapshot,
    workspace::{
        CreateWorkspaceRequest, PatchApplyReport, UpdateWorkspaceRequest, WorkspaceDetail,
        WorkspaceSnapshot, WorkspacesResponse,
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

    async fn read<T: DeserializeOwned>(&self, path: &str) -> anyhow::Result<T> {
        let url = format!("{}{path}", self.base);
        let response = self.http.get(&url).send().await.context(url.clone())?;
        Self::body(response, &url).await
    }

    async fn write<B: Serialize, T: DeserializeOwned>(
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
        self.read("/api/state").await
    }

    pub async fn devices(&self) -> anyhow::Result<Vec<DeviceInfo>> {
        #[derive(serde::Deserialize)]
        struct Devices {
            devices: Vec<DeviceInfo>,
        }
        Ok(self.read::<Devices>("/api/devices").await?.devices)
    }

    pub async fn channel_types(&self) -> anyhow::Result<Vec<ChannelDescriptor>> {
        #[derive(serde::Deserialize)]
        struct Types {
            types: Vec<ChannelDescriptor>,
        }
        Ok(self.read::<Types>("/api/channeltypes").await?.types)
    }

    pub async fn catalog(&self) -> anyhow::Result<PatchCatalog> {
        self.read("/api/patch/catalog").await
    }

    pub async fn workspaces(&self) -> anyhow::Result<WorkspacesResponse> {
        self.read("/api/workspaces").await
    }

    pub async fn workspace(&self, id: i64) -> anyhow::Result<WorkspaceDetail> {
        self.read(&format!("/api/workspaces/{id}")).await
    }

    pub async fn create_workspace(&self, name: &str) -> anyhow::Result<i64> {
        #[derive(serde::Deserialize)]
        struct Created {
            id: i64,
        }
        let body = CreateWorkspaceRequest {
            name: name.to_owned(),
            snapshot: None,
        };
        Ok(self
            .write::<_, Created>(reqwest::Method::POST, "/api/workspaces", Some(&body))
            .await?
            .id)
    }

    pub async fn save_workspace(
        &self,
        id: i64,
        revision: u64,
        snapshot: WorkspaceSnapshot,
    ) -> anyhow::Result<()> {
        let body = UpdateWorkspaceRequest {
            revision,
            name: None,
            snapshot: Some(snapshot),
        };
        self.write::<_, serde_json::Value>(
            reqwest::Method::PUT,
            &format!("/api/workspaces/{id}"),
            Some(&body),
        )
        .await
        .map(drop)
    }

    pub async fn activate_workspace(&self, id: i64) -> anyhow::Result<()> {
        self.write::<(), serde_json::Value>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/activate"),
            None,
        )
        .await
        .map(drop)
    }

    pub async fn step_history(&self, id: i64, back: bool) -> anyhow::Result<()> {
        let step = if back { "undo" } else { "redo" };
        self.write::<(), serde_json::Value>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/{step}"),
            None,
        )
        .await
        .map(drop)
    }

    pub async fn apply_workspace(&self, id: i64) -> anyhow::Result<PatchApplyReport> {
        self.write::<(), PatchApplyReport>(
            reqwest::Method::POST,
            &format!("/api/workspaces/{id}/apply"),
            None,
        )
        .await
    }

    pub async fn patch_device(&self, set: u32, settings: &DeviceSettings) -> anyhow::Result<()> {
        self.write::<_, serde_json::Value>(
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
        self.write::<_, serde_json::Value>(
            reqwest::Method::PATCH,
            &format!("/api/devicesets/{set}/channels/{channel}"),
            Some(settings),
        )
        .await
        .map(drop)
    }
}
