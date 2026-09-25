use std::sync::Arc;

use sdrmm_wire::{
    CreatedRowId,
    device::DeviceSettings,
    patch::RackLayout,
    rest::AuthInfo,
    workspace::{WorkspaceDetail, WorkspaceExport, WorkspaceSettings, WorkspacesResponse},
};
use zgui::prelude::*;

use crate::{
    binding, starter,
    store::{Phase, Store},
    workspace::Session,
};

impl Store {
    pub async fn boot(self) {
        self.phase.set(Phase::Loading);
        let api = self.api();
        match api.get::<AuthInfo>("/api/auth").await {
            Ok(auth) if auth.token_required && api.token().get().is_none() => {
                self.phase.set(Phase::Locked { refused: false });
                return;
            }
            Ok(_) => {}
            Err(error) => {
                self.phase.set(Phase::Unreachable(error.to_string()));
                return;
            }
        }
        let listed = match api.workspaces().await {
            Ok(listed) => listed,
            Err(error) => {
                self.phase.set(Phase::Unreachable(error.to_string()));
                return;
            }
        };
        self.workspaces.set(Arc::new(listed.workspaces.clone()));
        match listed
            .active
            .or_else(|| listed.workspaces.first().map(|found| found.id))
        {
            Some(id) => self.load(id, true).await,
            None => self.phase.set(Phase::NoWorkspace),
        }
    }

    pub fn refresh_workspaces(self) {
        zgui::task::spawn_local(async move {
            match self.api().workspaces().await {
                Ok(listed) => self.take_list(listed),
                Err(error) => tracing::debug!(%error, "no workspace list"),
            }
        });
    }

    fn take_list(self, listed: WorkspacesResponse) {
        if *self.workspaces.get_untracked() != listed.workspaces {
            self.workspaces.set(Arc::new(listed.workspaces));
        }
    }

    pub async fn load(self, id: i64, seed: bool) {
        let api = self.api();
        if let Err(error) = api.activate_workspace(id).await {
            self.fail("Cannot open the workspace", &error);
            return;
        }
        let detail = match api.workspace(id).await {
            Ok(detail) => detail,
            Err(error) => {
                self.fail("Cannot read the workspace", &error);
                return;
            }
        };
        self.take_detail(&detail);
        self.phase.set(Phase::Ready);
        self.refresh_workspaces();
        let seeding = seed && starter::wants_seeding(&detail.snapshot.graph);
        if seeding {
            let devices = api.devices().await.unwrap_or_default();
            let graph = starter::graph(&devices);
            self.graph.set(Arc::new(graph.clone()));
            if let Err(error) = self.write_graph(graph).await {
                self.fail("Cannot save the starter patch", &error);
                return;
            }
        }
        self.apply().await;
        if seeding {
            self.tune_starter().await;
        }
    }

    fn take_detail(self, detail: &WorkspaceDetail) {
        let (editor, worker) = Session::new(self.api(), detail.clone());
        self.editor.set_value(Some(editor));
        zgui::task::spawn_local(worker.run());
        self.workspace.set(Some(detail.info.id));
        self.selected.set(None);
        self.expanded.set(None);
        self.read_detail(detail);
        self.graph.set(Arc::new(detail.snapshot.graph.clone()));
    }

    async fn tune_starter(self) {
        let state = match self.api().state().await {
            Ok(state) => state,
            Err(error) => {
                tracing::debug!(%error, "cannot read the state after seeding");
                return;
            }
        };
        self.state.set(Arc::new(state.clone()));
        let graph = self.graph.get_untracked();
        let devices = binding::device_sets(&graph, &state.device_sets);
        let Some(set) = devices.values().copied().next() else {
            return;
        };
        let centre = DeviceSettings {
            center_hz: Some(starter::DEVICE_CENTRE_HZ),
            ..DeviceSettings::default()
        };
        if let Err(error) = self.api().patch_device(set, &centre).await {
            tracing::debug!(%error, "cannot centre the radio");
        }
        let channels = binding::channels(&graph, &state.device_sets, &devices);
        for channel in channels.values() {
            let mut settings = channel.settings.clone();
            settings.frequency_hz = starter::CHANNEL_HZ;
            if let Err(error) = self.api().patch_channel(set, channel.id, &settings).await {
                tracing::debug!(%error, "cannot tune the starter channel");
            }
        }
        self.refresh_state();
    }

    pub fn activate(self, id: i64) {
        if self.workspace.get_untracked() == Some(id) {
            return;
        }
        zgui::task::spawn_local(async move { self.load(id, false).await });
    }

    pub fn create(self, name: String) {
        zgui::task::spawn_local(async move {
            match self.api().create_workspace(&name).await {
                Ok(id) => self.load(id, true).await,
                Err(error) => self.fail("Cannot create the workspace", &error),
            }
        });
    }

    pub fn rename(self, id: i64, name: String) {
        if self.workspace.get_untracked() == Some(id)
            && let Some(editor) = self.editor.get_value()
        {
            let pending = editor.rename(name);
            zgui::task::spawn_local(async move {
                match pending.await {
                    Ok(detail) => {
                        self.read_detail(&detail);
                        self.refresh_workspaces();
                    }
                    Err(error) => self.fail("Cannot rename the workspace", &error),
                }
            });
            return;
        }
        zgui::task::spawn_local(async move {
            let api = self.api();
            let result = async {
                let detail = api.workspace(id).await?;
                let body = sdrmm_wire::workspace::UpdateWorkspaceRequest {
                    revision: detail.info.revision,
                    name: Some(name),
                    snapshot: None,
                };
                api.put::<_, serde::de::IgnoredAny>(&format!("/api/workspaces/{id}"), &body)
                    .await
            }
            .await;
            match result {
                Ok(_) => self.refresh_workspaces(),
                Err(error) => self.fail("Cannot rename the workspace", &error),
            }
        });
    }

    pub fn duplicate(self, id: i64) {
        zgui::task::spawn_local(async move {
            let api = self.api();
            let result = async {
                let export: WorkspaceExport =
                    api.get(&format!("/api/workspaces/{id}/export")).await?;
                api.post::<_, CreatedRowId>("/api/workspaces/import", &export)
                    .await
            }
            .await;
            match result {
                Ok(_) => self.refresh_workspaces(),
                Err(error) => self.fail("Cannot duplicate the workspace", &error),
            }
        });
    }

    pub fn import(self, export: WorkspaceExport) {
        zgui::task::spawn_local(async move {
            match self
                .api()
                .post::<_, CreatedRowId>("/api/workspaces/import", &export)
                .await
            {
                Ok(created) => self.load(created.id, false).await,
                Err(error) => self.fail("Cannot import the workspace", &error),
            }
        });
    }

    pub fn remove(self, id: i64) {
        zgui::task::spawn_local(async move {
            if let Err(error) = self.api().delete(&format!("/api/workspaces/{id}")).await {
                self.fail("Cannot delete the workspace", &error);
                return;
            }
            if self.workspace.get_untracked() == Some(id) {
                self.editor.set_value(None);
                self.workspace.set(None);
                self.boot().await;
            } else {
                self.refresh_workspaces();
            }
        });
    }

    pub fn edit_rack(self, change: impl FnOnce(&RackLayout) -> RackLayout) {
        let next = change(&self.rack.get_untracked());
        if *self.rack.get_untracked() == next {
            return;
        }
        self.rack.set(Arc::new(next.clone()));
        let Some(editor) = self.editor.get_value() else {
            return;
        };
        let pending = editor.rack(next);
        zgui::task::spawn_local(async move {
            match pending.await {
                Ok(detail) => self.read_detail(&detail),
                Err(error) => self.fail("Cannot save the rack", &error),
            }
        });
    }

    pub fn edit_settings(self, change: impl FnOnce(&mut WorkspaceSettings)) {
        let mut next = (*self.settings.get_untracked()).clone();
        change(&mut next);
        self.settings.set(Arc::new(next.clone()));
        let Some(editor) = self.editor.get_value() else {
            return;
        };
        let pending = editor.settings(next);
        zgui::task::spawn_local(async move {
            match pending.await {
                Ok(detail) => self.read_detail(&detail),
                Err(error) => self.fail("Cannot save the settings", &error),
            }
        });
    }

    pub fn submit_token(self, token: String) {
        self.api().token().set(Some(token));
        self.refresh_all();
        zgui::task::spawn_local(async move { self.boot().await });
    }

    pub fn retry(self) {
        self.refresh_all();
        zgui::task::spawn_local(async move { self.boot().await });
    }
}
