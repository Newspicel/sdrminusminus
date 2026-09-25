use std::{collections::HashMap, sync::Arc};

use sdrmm_wire::{
    channel::{ChannelDescriptor, ChannelInfo, ChannelSettings},
    decode::DecodedRecord,
    device::{DeviceInfo, DeviceSettings},
    patch::{PatchCatalog, PatchGraph},
    state::{ChannelLevel, DeviceSet, StateSnapshot},
    workspace::WorkspaceDetail,
    ws::{ClientCommand, ServerEvent, StateScope, StreamKind},
};
use tokio::sync::mpsc;
use zgui::prelude::*;

use crate::{
    api::Api,
    binding,
    socket::{Incoming, Socket, Spectrum},
    starter,
    workspace::Session,
};

pub const SPECTRUM_BINS: u16 = 512;
pub const SPECTRUM_FPS: u16 = 20;
const DECODED_KEEP: usize = 120;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Patch,
    Rack,
}

#[derive(Clone, Copy)]
pub struct Store {
    pub state: RwSignal<Arc<StateSnapshot>>,
    pub devices: RwSignal<Arc<Vec<DeviceInfo>>>,
    pub catalog: RwSignal<Arc<PatchCatalog>>,
    pub channel_types: RwSignal<Arc<Vec<ChannelDescriptor>>>,
    pub graph: RwSignal<Arc<PatchGraph>>,
    pub workspace: RwSignal<Option<i64>>,
    pub revision: RwSignal<u64>,
    pub name: RwSignal<String>,
    pub connected: RwSignal<bool>,
    pub can_undo: RwSignal<bool>,
    pub can_redo: RwSignal<bool>,
    pub notice: RwSignal<Option<String>>,
    pub pane: RwSignal<Pane>,
    pub selected: RwSignal<Option<String>>,
    pub palette: RwSignal<bool>,
    pub palette_at: RwSignal<Option<(f32, f32)>>,
    pub spectra: RwSignal<Arc<HashMap<u32, Arc<Spectrum>>>>,
    pub levels: RwSignal<Arc<HashMap<(u32, u32), ChannelLevel>>>,
    pub decoded: RwSignal<Arc<Vec<DecodedRecord>>>,
    streams: RwSignal<Arc<HashMap<u16, u32>>>,
    editor: StoredValue<Option<Session>>,
    api: StoredValue<Api>,
    socket: StoredValue<Option<Socket>>,
}

impl Store {
    pub fn new(api: Api) -> Self {
        Self {
            state: RwSignal::new(Arc::new(StateSnapshot::default())),
            devices: RwSignal::new(Arc::new(Vec::new())),
            catalog: RwSignal::new(Arc::new(PatchCatalog { nodes: Vec::new() })),
            channel_types: RwSignal::new(Arc::new(Vec::new())),
            graph: RwSignal::new(Arc::new(PatchGraph::default())),
            workspace: RwSignal::new(None),
            revision: RwSignal::new(0),
            name: RwSignal::new(String::from("sdr--")),
            connected: RwSignal::new(false),
            can_undo: RwSignal::new(false),
            can_redo: RwSignal::new(false),
            notice: RwSignal::new(None),
            pane: RwSignal::new(Pane::Patch),
            selected: RwSignal::new(None),
            palette: RwSignal::new(false),
            palette_at: RwSignal::new(None),
            spectra: RwSignal::new(Arc::new(HashMap::new())),
            levels: RwSignal::new(Arc::new(HashMap::new())),
            decoded: RwSignal::new(Arc::new(Vec::new())),
            streams: RwSignal::new(Arc::new(HashMap::new())),
            editor: StoredValue::new(None),
            api: StoredValue::new(api),
            socket: StoredValue::new(None),
        }
    }

    pub fn api(self) -> Api {
        self.api.get_value()
    }

    pub fn device_set_of(self, node: &str) -> Option<u32> {
        let graph = self.graph.get();
        let owner = binding::device_node_of(&graph, node)?;
        binding::device_sets(&graph, &self.state.get().device_sets)
            .get(&owner)
            .copied()
    }

    pub fn channel_of(self, node: &str) -> Option<ChannelInfo> {
        let graph = self.graph.get();
        let state = self.state.get();
        let devices = binding::device_sets(&graph, &state.device_sets);
        binding::channels(&graph, &state.device_sets, &devices).remove(node)
    }

    pub fn set_of(self, id: u32) -> Option<DeviceSet> {
        self.state
            .get()
            .device_sets
            .iter()
            .find(|set| set.id == id)
            .cloned()
    }

    pub fn say(self, message: impl Into<String>) {
        self.notice.set(Some(message.into()));
    }

    pub fn start(self, socket: Socket, incoming: mpsc::UnboundedReceiver<Incoming>) {
        self.socket.set_value(Some(socket));
        zgui::task::spawn_local(async move { self.drain(incoming).await });
        self.refresh_all();
        zgui::task::spawn_local(async move { self.open_workspace().await });
    }

    pub fn command(self, command: ClientCommand) {
        if let Some(socket) = self.socket.get_value() {
            socket.send(command);
        }
    }

    pub fn watch_spectrum(self, set: u32) {
        self.command(ClientCommand::SubscribeSpectrum {
            device_set: set,
            fps: SPECTRUM_FPS,
            bins: SPECTRUM_BINS,
            stream: 0,
        });
    }

    async fn drain(self, mut incoming: mpsc::UnboundedReceiver<Incoming>) {
        while let Some(message) = incoming.recv().await {
            match message {
                Incoming::Up => {
                    self.connected.set(true);
                    self.refresh_all();
                }
                Incoming::Down => {
                    self.connected.set(false);
                    self.say("the server connection dropped");
                }
                Incoming::Event(event) => self.absorb(*event),
                Incoming::Spectrum(spectrum) => {
                    if let Some(set) = self.streams.get_untracked().get(&spectrum.stream_id) {
                        let mut next = (*self.spectra.get_untracked()).clone();
                        next.insert(*set, spectrum);
                        self.spectra.set(Arc::new(next));
                    }
                }
            }
        }
    }

    fn absorb(self, event: ServerEvent) {
        match event {
            ServerEvent::Hello { .. } => self.refresh_all(),
            ServerEvent::StateChanged { scope } => match scope {
                StateScope::Devices => self.refresh_devices(),
                StateScope::Workspaces => {}
                _ => self.refresh_state(),
            },
            ServerEvent::StreamStarted {
                stream_id,
                device_set,
                ..
            } => {
                let mut next = (*self.streams.get_untracked()).clone();
                next.insert(stream_id, device_set);
                self.streams.set(Arc::new(next));
            }
            ServerEvent::StreamStopped { stream_id, kind } => {
                if kind == StreamKind::Spectrum {
                    let mut next = (*self.streams.get_untracked()).clone();
                    next.remove(&stream_id);
                    self.streams.set(Arc::new(next));
                }
            }
            ServerEvent::ChannelLevels { device_set, levels } => {
                let mut next = (*self.levels.get_untracked()).clone();
                for level in levels {
                    next.insert((device_set, level.channel), level);
                }
                self.levels.set(Arc::new(next));
            }
            ServerEvent::Decoded(record) => {
                let mut next = (*self.decoded.get_untracked()).clone();
                next.push(*record);
                let overflow = next.len().saturating_sub(DECODED_KEEP);
                next.drain(..overflow);
                self.decoded.set(Arc::new(next));
            }
            ServerEvent::Error { message } => self.say(message),
            _ => {}
        }
    }

    pub fn refresh_all(self) {
        self.refresh_state();
        self.refresh_devices();
        zgui::task::spawn_local(async move {
            match self.api().catalog().await {
                Ok(catalog) => self.catalog.set(Arc::new(catalog)),
                Err(error) => tracing::debug!(%error, "no catalog"),
            }
            match self.api().channel_types().await {
                Ok(types) => self.channel_types.set(Arc::new(types)),
                Err(error) => tracing::debug!(%error, "no channel types"),
            }
        });
    }

    pub fn descriptor_of(self, type_id: &str) -> Option<ChannelDescriptor> {
        self.channel_types
            .get()
            .iter()
            .find(|descriptor| descriptor.type_id == type_id)
            .cloned()
    }

    pub fn refresh_state(self) {
        zgui::task::spawn_local(async move {
            match self.api().state().await {
                Ok(state) => self.state.set(Arc::new(state)),
                Err(error) => tracing::debug!(%error, "no state"),
            }
        });
    }

    pub fn refresh_devices(self) {
        zgui::task::spawn_local(async move {
            match self.api().devices().await {
                Ok(devices) => self.devices.set(Arc::new(devices)),
                Err(error) => tracing::debug!(%error, "no devices"),
            }
        });
    }

    async fn open_workspace(self) {
        let api = self.api();
        let listed = match api.workspaces().await {
            Ok(listed) => listed,
            Err(error) => {
                self.say(format!("cannot reach the server: {error}"));
                return;
            }
        };
        let id = match listed
            .active
            .or_else(|| listed.workspaces.first().map(|w| w.id))
        {
            Some(id) => id,
            None => match api.create_workspace("Native").await {
                Ok(id) => id,
                Err(error) => {
                    self.say(format!("cannot create a workspace: {error}"));
                    return;
                }
            },
        };
        if let Err(error) = api.activate_workspace(id).await {
            tracing::debug!(%error, "cannot activate the workspace");
        }
        let detail = match api.workspace(id).await {
            Ok(detail) => detail,
            Err(error) => {
                self.say(format!("cannot read the workspace: {error}"));
                return;
            }
        };
        let (editor, worker) = Session::new(api.clone(), detail.clone());
        self.editor.set_value(Some(editor));
        zgui::task::spawn_local(worker.run());
        self.workspace.set(Some(id));
        self.revision.set(detail.info.revision);
        self.name.set(detail.info.name.clone());
        self.can_undo.set(detail.history.can_undo);
        self.can_redo.set(detail.history.can_redo);

        let seeding = starter::wants_seeding(&detail.snapshot.graph);
        let graph = if seeding {
            let devices = api.devices().await.unwrap_or_default();
            let graph = starter::graph(&devices);
            if let Err(error) = self.write_graph(graph.clone()).await {
                self.say(format!("cannot save the starter patch: {error}"));
                return;
            }
            graph
        } else {
            detail.snapshot.graph.clone()
        };
        self.graph.set(Arc::new(graph));
        self.apply().await;
        if seeding {
            self.tune_starter().await;
        }
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

    pub async fn apply(self) {
        let Some(id) = self.workspace.get_untracked() else {
            return;
        };
        match self.api().apply_workspace(id).await {
            Ok(report) => {
                for refusal in &report.refused {
                    self.say(format!("{}: {}", refusal.node, refusal.reason));
                }
                self.refresh_state();
            }
            Err(error) => self.say(format!("cannot apply the patch: {error}")),
        }
    }

    pub fn edit_graph(self, edit: impl FnOnce(&mut PatchGraph)) {
        let mut graph = (*self.graph.get_untracked()).clone();
        edit(&mut graph);
        self.graph.set(Arc::new(graph.clone()));
        let save = self.write_graph(graph);
        zgui::task::spawn_local(async move {
            match save.await {
                Ok(()) => self.apply().await,
                Err(error) => self.say(format!("cannot save the patch: {error}")),
            }
        });
    }

    pub fn move_node(self, node: String, x: f32, y: f32) {
        let mut graph = (*self.graph.get_untracked()).clone();
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node) {
            found.position.x = x;
            found.position.y = y;
        }
        self.graph.set(Arc::new(graph));
    }

    pub fn open_palette_at(self, x: f32, y: f32) {
        self.palette_at.set(Some((x, y)));
        self.palette.set(true);
    }

    pub fn resize_node(self, node: String, x: f32, y: f32, width: f32, height: f32) {
        let mut graph = (*self.graph.get_untracked()).clone();
        if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == node) {
            found.position.x = x;
            found.position.y = y;
            found.size = Some(sdrmm_wire::patch::Size {
                w: width,
                h: height,
            });
        }
        self.graph.set(Arc::new(graph));
        self.commit_layout();
    }

    pub fn commit_layout(self) {
        let save = self.write_graph((*self.graph.get_untracked()).clone());
        zgui::task::spawn_local(async move {
            if let Err(error) = save.await {
                self.say(format!("cannot save the layout: {error}"));
            }
        });
    }

    fn read_detail(self, detail: &WorkspaceDetail) {
        self.revision.set(detail.info.revision);
        self.can_undo.set(detail.history.can_undo);
        self.can_redo.set(detail.history.can_redo);
        self.name.set(detail.info.name.clone());
    }

    fn write_graph(self, graph: PatchGraph) -> impl Future<Output = anyhow::Result<()>> {
        let pending = self.editor.get_value().map(|editor| editor.save(graph));
        async move {
            let pending = pending.ok_or_else(|| anyhow::anyhow!("workspace is still loading"))?;
            self.read_detail(&pending.await?);
            Ok(())
        }
    }

    pub fn step_history(self, back: bool) {
        let Some(editor) = self.editor.get_value() else {
            return;
        };
        let pending = editor.step(back);
        zgui::task::spawn_local(async move {
            match pending.await {
                Ok(detail) => {
                    self.read_detail(&detail);
                    self.graph.set(Arc::new(detail.snapshot.graph));
                    self.apply().await;
                }
                Err(error) => self.say(format!("cannot step the history: {error}")),
            }
        });
    }

    pub fn tune_device(self, node: String, hz: f64) {
        let Some(set) = self.device_set_of(&node) else {
            return;
        };
        self.set_device(
            set,
            DeviceSettings {
                center_hz: Some(hz),
                ..DeviceSettings::default()
            },
        );
    }

    pub fn set_device(self, set: u32, settings: DeviceSettings) {
        let Some(editor) = self.editor.get_value() else {
            self.say("Workspace is still loading");
            return;
        };
        self.receive_settings(editor.device(set, settings));
    }

    fn receive_settings(
        self,
        pending: impl Future<Output = anyhow::Result<WorkspaceDetail>> + 'static,
    ) {
        zgui::task::spawn_local(async move {
            match pending.await {
                Ok(detail) => self.read_detail(&detail),
                Err(error) => {
                    self.say(format!("cannot change settings: {error}"));
                    self.refresh_state();
                }
            }
        });
    }

    pub fn set_channel(self, node: String, settings: ChannelSettings) {
        let Some(set) = self.device_set_of(&node) else {
            return;
        };
        let Some(channel) = self.channel_of(&node) else {
            return;
        };
        let Some(editor) = self.editor.get_value() else {
            self.say("Workspace is still loading");
            return;
        };
        let mut state = (*self.state.get_untracked()).clone();
        if let Some(current) = state
            .device_sets
            .iter_mut()
            .find(|current| current.id == set)
            .and_then(|current| {
                current
                    .channels
                    .iter_mut()
                    .find(|current| current.id == channel.id)
            })
        {
            current.settings = settings.clone();
        }
        self.state.set(Arc::new(state));
        self.receive_settings(editor.channel(set, channel.id, settings));
    }
}
