use std::{collections::HashMap, rc::Rc, sync::Arc};

use sdrmm_wire::{
    channel::{ChannelDescriptor, ChannelInfo, ChannelSettings},
    decode::DecodedRecord,
    device::{DeviceInfo, DeviceSettings},
    frame::FrameKind,
    patch::{PatchCatalog, PatchGraph, RackLayout},
    state::{ChannelLevel, DeviceSet, StateSnapshot},
    workspace::{WorkspaceDetail, WorkspaceInfo, WorkspaceSettings},
    ws::{ClientCommand, ServerEvent, StateScope},
};
use tokio::sync::mpsc;
use zgui::prelude::*;

use crate::{
    api::Api,
    binding,
    bus::{Bus, Frame, Source},
    shell::{
        apply_toasts::apply_toasts,
        toasts::{Toasts, Tone},
    },
    decoded::Decoded,
    coherent,
    socket::{Incoming, Socket},
    workspace::Session,
};

pub const SPECTRUM_BINS: u16 = 512;
pub const SPECTRUM_FPS: u16 = 20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Patch,
    Rack,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Phase {
    Loading,
    Ready,
    NoWorkspace,
    Unreachable(String),
    Locked { refused: bool },
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
    pub toasts: RwSignal<Arc<Toasts>>,
    pub phase: RwSignal<Phase>,
    pub rack: RwSignal<Arc<RackLayout>>,
    pub settings: RwSignal<Arc<WorkspaceSettings>>,
    pub workspaces: RwSignal<Arc<Vec<WorkspaceInfo>>>,
    pub expanded: RwSignal<Option<String>>,
    pub pane: RwSignal<Pane>,
    pub selected: RwSignal<Option<String>>,
    pub palette: RwSignal<bool>,
    pub palette_at: RwSignal<Option<(f32, f32)>>,
    pub levels: RwSignal<Arc<HashMap<(u32, u32), ChannelLevel>>>,
    pub decoded: RwSignal<Arc<Decoded>>,
    staged: StoredValue<Vec<DecodedRecord>>,
    pub coherent: RwSignal<Arc<coherent::Book>>,
    bus: StoredValue<Rc<Bus>, LocalStorage>,
    pub(crate) editor: StoredValue<Option<Session>>,
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
            toasts: RwSignal::new(Arc::new(Toasts::default())),
            phase: RwSignal::new(Phase::Loading),
            rack: RwSignal::new(Arc::new(RackLayout::default())),
            settings: RwSignal::new(Arc::new(WorkspaceSettings::default())),
            workspaces: RwSignal::new(Arc::new(Vec::new())),
            expanded: RwSignal::new(None),
            pane: RwSignal::new(Pane::Patch),
            selected: RwSignal::new(None),
            palette: RwSignal::new(false),
            palette_at: RwSignal::new(None),
            levels: RwSignal::new(Arc::new(HashMap::new())),
            decoded: RwSignal::new(Arc::new(Decoded::default())),
            staged: StoredValue::new(Vec::new()),
            coherent: RwSignal::new(Arc::new(coherent::Book::default())),
            bus: StoredValue::new_local(Rc::new(Bus::default())),
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
        self.toast(&message.into(), Tone::Error, None);
    }

    pub fn note(self, message: impl Into<String>) {
        self.toast(&message.into(), Tone::Info, None);
    }

    pub fn fail(self, context: &str, error: &anyhow::Error) {
        let code = crate::api::error_code(error);
        self.toast(&format!("{context}: {error}"), Tone::Error, code.as_deref());
    }

    fn toast(self, message: &str, tone: Tone, code: Option<&str>) {
        let at = jiff::Timestamp::now();
        let now_ms = u64::try_from(at.as_millisecond()).unwrap_or_default();
        let mut next = (*self.toasts.get_untracked()).clone();
        next.push(message, tone, code, now_ms, &at.to_string());
        self.toasts.set(Arc::new(next));
    }

    pub fn start(self, socket: Socket, incoming: mpsc::UnboundedReceiver<Incoming>) {
        self.socket.set_value(Some(socket));
        zgui::task::spawn_local(async move { self.drain(incoming).await });
        self.refresh_all();
        zgui::task::spawn_local(async move { self.boot().await });
    }

    pub fn command(self, command: ClientCommand) {
        if let Some(socket) = self.socket.get_value() {
            socket.send(command);
        }
    }

    pub fn hold(self, command: ClientCommand) {
        if let Some(first) = self.bus.get_value().hold(&command) {
            self.command(first);
        }
        on_cleanup_local(move || {
            if let Some(last) = self.bus.get_value().release(&command) {
                self.command(last);
            }
        });
    }

    pub fn on_event(self, handler: impl Fn(&ServerEvent) + 'static) {
        let bus = self.bus.get_value();
        let id = bus.events.borrow_mut().add(handler);
        on_cleanup_local(move || bus.events.borrow_mut().remove(id));
    }

    pub fn on_frame(self, handler: impl Fn(&Frame) + 'static) {
        let bus = self.bus.get_value();
        let id = bus.frames.borrow_mut().add(handler);
        on_cleanup_local(move || bus.frames.borrow_mut().remove(id));
    }

    pub fn source_of(self, stream_id: u16) -> Option<Source> {
        self.bus.get_value().source_of(stream_id)
    }

    fn receive_frame(self, frame: &Frame) {
        self.bus.get_value().publish_frame(frame);
    }

    async fn drain(self, mut incoming: mpsc::UnboundedReceiver<Incoming>) {
        while let Some(message) = incoming.recv().await {
            self.receive(message);
            while let Ok(message) = incoming.try_recv() {
                self.receive(message);
            }
            self.publish_decoded();
        }
    }

    fn receive(self, message: Incoming) {
        match message {
            Incoming::Up => {
                self.connected.set(true);
                for command in self.bus.get_value().held() {
                    self.command(command);
                }
                self.refresh_all();
            }
            Incoming::Down => {
                self.connected.set(false);
                self.say("Lost the server: reconnecting");
            }
            Incoming::Event(event) => {
                self.bus.get_value().publish_event(&event);
                self.absorb(*event);
            }
            Incoming::Frame(frame) => self.receive_frame(&frame),
        }
    }

    fn absorb(self, event: ServerEvent) {
        match event {
            ServerEvent::Hello { .. } => self.refresh_all(),
            ServerEvent::StateChanged { scope } => match scope {
                StateScope::Devices => self.refresh_devices(),
                StateScope::Workspaces => self.refresh_workspaces(),
                _ => self.refresh_state(),
            },
            ServerEvent::ChannelLevels { device_set, levels } => {
                let mut next = (*self.levels.get_untracked()).clone();
                for level in levels {
                    next.insert((device_set, level.channel), level);
                }
                self.levels.set(Arc::new(next));
            }
            ServerEvent::Decoded(record) => self.staged.update_value(|staged| staged.push(*record)),
            ServerEvent::DecodedBacklog { records } => {
                let mut next = (*self.decoded.get_untracked()).clone();
                if next.hydrate(&records) {
                    self.decoded.set(Arc::new(next));
                }
            }
            ServerEvent::DecodedLost { count } => {
                self.decoded
                    .update(|decoded| Arc::make_mut(decoded).report_lost(count));
            }
            ServerEvent::Error { message } => self.say(message),
            event @ (ServerEvent::DfUpdate { .. }
            | ServerEvent::DfFusionUpdate { .. }
            | ServerEvent::RadarDetections { .. }) => {
                let mut next = (*self.coherent.get_untracked()).clone();
                next.observe(&event, std::time::SystemTime::now());
                self.coherent.set(Arc::new(next));
            }
            _ => {}
        }
    }

    fn publish_decoded(self) {
        let batch = self
            .staged
            .try_update_value(std::mem::take)
            .unwrap_or_default();
        if !batch.is_empty() {
            self.decoded
                .update(|decoded| Arc::make_mut(decoded).publish(batch));
        }
    }

    pub fn age_out_stations(self, max_age_ms: i64) {
        let now = crate::decoded::now_ms();
        if self
            .decoded
            .with_untracked(|decoded| decoded.stale(max_age_ms, now))
        {
            self.decoded
                .update(|decoded| Arc::make_mut(decoded).age_out(max_age_ms, now));
        }
    }

    pub fn drop_decoded(self, matches: impl Fn(&DecodedRecord) -> bool) -> usize {
        let mut dropped = 0;
        self.decoded
            .update(|decoded| dropped = Arc::make_mut(decoded).drop_frames(matches));
        dropped
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

    pub async fn apply(self) {
        let Some(id) = self.workspace.get_untracked() else {
            return;
        };
        match self.api().apply_workspace(id).await {
            Ok(report) => {
                for message in apply_toasts(&report, &self.graph.get_untracked().nodes) {
                    self.say(message);
                }
                self.refresh_state();
            }
            Err(error) => self.fail("Cannot apply the patch", &error),
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
                Err(error) => self.fail("Cannot save the patch", &error),
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
                self.fail("Cannot save the layout", &error);
            }
        });
    }

    pub(crate) fn read_detail(self, detail: &WorkspaceDetail) {
        self.revision.set(detail.info.revision);
        self.can_undo.set(detail.history.can_undo);
        self.can_redo.set(detail.history.can_redo);
        self.name.set(detail.info.name.clone());
        if *self.rack.get_untracked() != detail.snapshot.rack {
            self.rack.set(Arc::new(detail.snapshot.rack.clone()));
        }
        if *self.settings.get_untracked() != detail.snapshot.settings {
            self.settings
                .set(Arc::new(detail.snapshot.settings.clone()));
        }
    }

    pub(crate) fn write_graph(self, graph: PatchGraph) -> impl Future<Output = anyhow::Result<()>> {
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
                Err(error) => self.fail("Cannot step the history", &error),
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
                    self.fail("Cannot change settings", &error);
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
