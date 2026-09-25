use anyhow::Context;
use sdrmm_wire::{
    channel::ChannelSettings,
    device::DeviceSettings,
    patch::{PatchGraph, RackLayout},
    workspace::{
        UpdateWorkspaceRequest, WorkspaceDetail, WorkspaceInfo, WorkspaceSettings,
        WorkspaceSnapshot,
    },
};
use tokio::sync::{mpsc, oneshot};

use crate::{api::Api, shell::rack_grid};

pub mod manage;

type Reply = oneshot::Sender<anyhow::Result<WorkspaceDetail>>;

enum Edit {
    Graph(PatchGraph),
    Rack(RackLayout),
    Settings(WorkspaceSettings),
    Rename(String),
    History(bool),
    Channel(u32, u32, ChannelSettings),
    Device(u32, DeviceSettings),
}

#[derive(Clone)]
pub struct Session {
    edits: mpsc::UnboundedSender<(Edit, Reply)>,
}

pub struct Worker {
    api: Api,
    detail: WorkspaceDetail,
    edits: mpsc::UnboundedReceiver<(Edit, Reply)>,
}

type Pending = std::pin::Pin<Box<dyn Future<Output = anyhow::Result<WorkspaceDetail>>>>;

impl Session {
    pub fn new(api: Api, detail: WorkspaceDetail) -> (Self, Worker) {
        let (edits, receiver) = mpsc::unbounded_channel();
        (
            Self { edits },
            Worker {
                api,
                detail,
                edits: receiver,
            },
        )
    }

    pub fn save(
        &self,
        graph: PatchGraph,
    ) -> impl Future<Output = anyhow::Result<WorkspaceDetail>> + use<> {
        self.enqueue(Edit::Graph(graph))
    }

    pub fn rack(&self, rack: RackLayout) -> Pending {
        Box::pin(self.enqueue(Edit::Rack(rack)))
    }

    pub fn settings(&self, settings: WorkspaceSettings) -> Pending {
        Box::pin(self.enqueue(Edit::Settings(settings)))
    }

    pub fn rename(&self, name: String) -> Pending {
        Box::pin(self.enqueue(Edit::Rename(name)))
    }

    pub fn step(
        &self,
        back: bool,
    ) -> impl Future<Output = anyhow::Result<WorkspaceDetail>> + use<> {
        self.enqueue(Edit::History(back))
    }

    pub fn channel(
        &self,
        set: u32,
        channel: u32,
        settings: ChannelSettings,
    ) -> impl Future<Output = anyhow::Result<WorkspaceDetail>> + use<> {
        self.enqueue(Edit::Channel(set, channel, settings))
    }

    pub fn device(
        &self,
        set: u32,
        settings: DeviceSettings,
    ) -> impl Future<Output = anyhow::Result<WorkspaceDetail>> + use<> {
        self.enqueue(Edit::Device(set, settings))
    }

    fn enqueue(&self, edit: Edit) -> impl Future<Output = anyhow::Result<WorkspaceDetail>> + use<> {
        let (reply, receive) = oneshot::channel();
        let sent = self.edits.send((edit, reply));
        async move {
            sent.map_err(|_| anyhow::anyhow!("workspace writer is closed"))?;
            receive
                .await
                .context("workspace writer stopped before saving")?
        }
    }
}

impl Worker {
    pub async fn run(mut self) {
        while let Some((edit, reply)) = self.edits.recv().await {
            let result = self.edit(edit).await;
            if let Err(undelivered) = reply.send(result)
                && let Err(error) = undelivered
            {
                tracing::error!(%error, "workspace edit failed after its caller closed");
            }
        }
    }

    async fn edit(&mut self, edit: Edit) -> anyhow::Result<WorkspaceDetail> {
        let id = self.detail.info.id;
        match edit {
            Edit::Graph(graph) => {
                let snapshot = with_graph(self.detail.snapshot.clone(), graph);
                self.write(snapshot).await?;
            }
            Edit::Rack(rack) => {
                let snapshot = WorkspaceSnapshot {
                    rack: rack_grid::prune(&rack, &self.detail.snapshot.graph),
                    ..self.detail.snapshot.clone()
                };
                self.write(snapshot).await?;
            }
            Edit::Settings(settings) => {
                let snapshot = WorkspaceSnapshot {
                    settings,
                    ..self.detail.snapshot.clone()
                };
                self.write(snapshot).await?;
            }
            Edit::Rename(name) => {
                let body = UpdateWorkspaceRequest {
                    revision: self.detail.info.revision,
                    name: Some(name),
                    snapshot: None,
                };
                self.detail.info = self
                    .api
                    .put::<_, WorkspaceInfo>(&format!("/api/workspaces/{id}"), &body)
                    .await?;
            }
            Edit::History(back) => self.api.step_history(id, back).await?,
            Edit::Channel(set, channel, settings) => {
                self.api.patch_channel(set, channel, &settings).await?
            }
            Edit::Device(set, settings) => self.api.patch_device(set, &settings).await?,
        }
        self.detail = self.api.workspace(id).await?;
        Ok(self.detail.clone())
    }

    async fn write(&mut self, snapshot: WorkspaceSnapshot) -> anyhow::Result<()> {
        snapshot.validate()?;
        let id = self.detail.info.id;
        self.detail.info = self
            .api
            .save_workspace(id, self.detail.info.revision, snapshot.clone())
            .await?;
        self.detail.snapshot = snapshot;
        Ok(())
    }
}

fn with_graph(mut snapshot: WorkspaceSnapshot, graph: PatchGraph) -> WorkspaceSnapshot {
    snapshot.rack = rack_grid::prune(&snapshot.rack, &graph);
    snapshot.graph = graph;
    snapshot
}

#[cfg(test)]
mod tests;
