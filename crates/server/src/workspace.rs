use std::{collections::HashSet, time::Duration};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    ChannelInfo, ChannelSettings, DeviceSet, NodeBody, PatchGraph, ServerEvent, StateScope,
    StateSnapshot, WorkspaceChannel, WorkspaceDevice, WorkspaceState,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{
    AppState,
    store::{SettingsStep, Store, StoreError},
};

const AUTOSAVE_IDLE: Duration = Duration::from_secs(2);
const AUTOSAVE_MAX_WAIT: Duration = Duration::from_secs(15);

pub(crate) struct DeviceBinding {
    pub node: String,
    pub device_set: u32,
    pub channels: Vec<(String, u32)>,
}

pub(crate) fn adopt_named_devices(engine: &Engine, store: &Store) {
    let Ok(workspaces) = store.list_workspaces() else {
        return;
    };
    for info in &workspaces.workspaces {
        let Ok(detail) = store.workspace(info.id) else {
            continue;
        };
        for node in detail.snapshot.graph.device_nodes() {
            if matches!(node.body, NodeBody::Array(_)) {
                continue;
            }
            let Some(reference) = node.body.device_ref(&node.id) else {
                continue;
            };
            let Some(key) = &reference.key else { continue };
            if let Some(adopted) = engine.adopt_device(&format!("{}:{key}", reference.backend)) {
                tracing::info!(device = %adopted.id(), workspace = info.id, "adopted a named device");
            }
        }
    }
}

pub(crate) fn describe_arrays(engine: &Engine, graph: &PatchGraph) {
    let state = engine.snapshot();
    let bound = bind_devices(graph, &state);
    let definitions = graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::Array(array) = &node.body else {
                return None;
            };
            let members = graph
                .array_members(&node.id)
                .iter()
                .map(|member| {
                    let (_, id) = bound.iter().find(|(node, _)| node == member)?;
                    state
                        .device_sets
                        .iter()
                        .find(|set| set.id == *id)
                        .map(|set| set.device.id())
                })
                .collect::<Option<Vec<_>>>()?;
            Some(array.definition(&node.id, node.label.as_deref(), members))
        })
        .filter(sdrmm_wire::ArrayDefinition::valid)
        .collect();
    engine.arrays().replace(definitions);
}

pub(crate) fn bind_devices(graph: &PatchGraph, state: &StateSnapshot) -> Vec<(String, u32)> {
    let mut bound = Vec::new();
    let mut claimed: Vec<u32> = Vec::new();
    for node in graph.device_nodes() {
        let Some(reference) = node.body.device_ref(&node.id) else {
            continue;
        };
        let reference = &reference;
        if let Some(set) = state
            .device_sets
            .iter()
            .find(|set| !claimed.contains(&set.id) && reference.matches(&set.device))
        {
            claimed.push(set.id);
            bound.push((node.id.clone(), set.id));
        }
    }
    bound
}

/// Channels wired to a direction finder's beam rather than to an antenna, which sit on the lane
/// one past the radio's own.
fn beam_bound(graph: &PatchGraph, device_node: &str, set: &DeviceSet) -> Vec<(String, u32)> {
    let mut bound = Vec::new();
    for node in &graph.nodes {
        if node.body.lane_output().is_none() {
            continue;
        }
        let from_device = graph
            .edges
            .iter()
            .any(|edge| edge.to.node == node.id && edge.from.node == device_node);
        if !from_device {
            continue;
        }
        for listener in crate::coherent::beam_listeners(graph, &node.id) {
            let Some(NodeBody::Channel(wanted)) = graph.node(&listener).map(|node| &node.body)
            else {
                continue;
            };
            if let Some(channel) = set.channels.iter().find(|channel| {
                channel.node.as_deref() == Some(listener.as_str())
                    && carries(channel, &wanted.channel_type, set.capabilities.rx_streams)
            }) {
                bound.push((listener, channel.id));
            }
        }
    }
    bound
}

fn carries(live: &ChannelInfo, channel_type: &str, stream: u32) -> bool {
    live.settings.params.type_id() == channel_type && live.stream == stream
}

/// Every channel node wired into a radio paired with the live decoder that is its own. A decoder
/// opened for a node answers to that node alone; one nobody opened for a node, such as a
/// template's, goes to the first node of its kind on its stream that is still unpaired.
pub(crate) fn bind_channels(
    graph: &PatchGraph,
    device_node: &str,
    set: &DeviceSet,
    reserved: &HashSet<u32>,
) -> Vec<(String, u32)> {
    let mut free: Vec<&ChannelInfo> = set.channels.iter().collect();
    let wired: Vec<(&str, &str, u32)> = graph
        .channels_of(device_node)
        .filter_map(|(node, stream)| match &node.body {
            NodeBody::Channel(channel) => {
                Some((node.id.as_str(), channel.channel_type.as_str(), stream))
            }
            _ => None,
        })
        .collect();
    let mut bound = Vec::new();
    for (node, channel_type, stream) in &wired {
        let own = free.iter().position(|live| {
            live.node.as_deref() == Some(*node) && carries(live, channel_type, *stream)
        });
        if let Some(at) = own {
            bound.push(((*node).to_owned(), free.remove(at).id));
        }
    }
    for (node, channel_type, stream) in &wired {
        if bound.iter().any(|(held, _)| held == node) {
            continue;
        }
        if graph.lanes_of(node).first() != Some(&(device_node, *stream)) {
            continue;
        }
        let unclaimed = free.iter().position(|live| {
            live.node.is_none()
                && !reserved.contains(&live.id)
                && carries(live, channel_type, *stream)
        });
        if let Some(at) = unclaimed {
            bound.push(((*node).to_owned(), free.remove(at).id));
        }
    }
    bound.extend(beam_bound(graph, device_node, set));
    bound
}

/// The channels a trunk system opened for itself on a radio. They belong to the system node and
/// are never handed to a decoder node that happens to share their type.
pub(crate) fn trunk_channels(state: &StateSnapshot, device_set: u32) -> HashSet<u32> {
    state
        .trunk_systems
        .iter()
        .flat_map(|system| {
            system
                .control
                .iter()
                .map(|control| (control.device_set, control.channel))
                .chain(
                    system
                        .followers
                        .iter()
                        .map(|follower| (follower.device_set, follower.channel)),
                )
                .chain(
                    system
                        .probes
                        .iter()
                        .map(|probe| (probe.device_set, probe.channel)),
                )
        })
        .filter(|(set, _)| *set == device_set)
        .map(|(_, channel)| channel)
        .collect()
}

pub(crate) fn bind(graph: &PatchGraph, state: &StateSnapshot) -> Vec<DeviceBinding> {
    bind_devices(graph, state)
        .into_iter()
        .filter_map(|(node, device_set)| {
            let set = state.device_sets.iter().find(|set| set.id == device_set)?;
            let reserved = trunk_channels(state, device_set);
            Some(DeviceBinding {
                channels: bind_channels(graph, &node, set, &reserved)
                    .into_iter()
                    .filter(|(node, id)| {
                        set.channels
                            .iter()
                            .any(|channel| channel.id == *id && channel.node.is_some())
                            || !state.device_sets.iter().any(|set| {
                                set.channels
                                    .iter()
                                    .any(|channel| channel.node.as_deref() == Some(node.as_str()))
                            })
                    })
                    .collect(),
                node,
                device_set,
            })
        })
        .collect()
}

pub(crate) fn capture(
    graph: &PatchGraph,
    state: &StateSnapshot,
    unrestored: &[String],
) -> (Vec<WorkspaceDevice>, Vec<WorkspaceChannel>) {
    let mut devices = Vec::new();
    let mut channels = Vec::new();
    for binding in bind(graph, state) {
        let Some(set) = state
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        if unrestored.contains(&binding.node) {
            continue;
        }
        let scanned = |id: u32| set.scanners.iter().any(|scan| scan.settings.channel == id);
        for (node, id) in binding.channels {
            let Some(channel) = set.channels.iter().find(|channel| channel.id == id) else {
                continue;
            };
            if scanned(id) {
                continue;
            }
            channels.push(WorkspaceChannel {
                node,
                settings: channel.settings.clone(),
            });
        }
        let follows_a_scan = set.scanners.iter().any(|scan| {
            set.channels
                .iter()
                .find(|channel| channel.id == scan.settings.channel)
                .is_some_and(|channel| {
                    set.settings
                        .for_stream(channel.stream, &set.capabilities.per_stream)
                        .tunes_itself()
                })
        });
        if !follows_a_scan {
            devices.push(WorkspaceDevice {
                node: binding.node,
                settings: set.settings.clone(),
            });
        }
    }
    (devices, channels)
}

pub(crate) fn save_active(state: &AppState) -> Result<(), StoreError> {
    let Some(active) = state.store.active_workspace()? else {
        return Ok(());
    };
    let graph = &active.snapshot.graph;
    let mut stored = live_state(state, active.info.id, graph)?;
    stored.merge_trunks(learned_trunks(&state.engine.trunk_systems()));
    let recoverable = state.store.history_nodes(active.info.id)?;
    stored.retain_nodes(|node| graph.node(node).is_some() || recoverable.contains(node));
    state.store.put_workspace_state(active.info.id, &stored)
}

fn live_state(
    state: &AppState,
    workspace: i64,
    graph: &PatchGraph,
) -> Result<WorkspaceState, StoreError> {
    let mut stored = state.store.workspace_state(workspace)?;
    let unrestored = state
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let (devices, channels) = capture(graph, &state.engine.snapshot(), &unrestored);
    stored.merge(devices);
    stored.merge_channels(channels);
    Ok(stored)
}

/// A settings change in flight: which node the operator is turning, and where the workspace stood
/// before the turn. Held across the engine patch so the history can record both sides of it.
pub(crate) struct SettingsEdit {
    workspace: i64,
    node: String,
    graph: PatchGraph,
    before: WorkspaceState,
}

/// Opens a settings change for the node the patch lands on, or `None` when it lands on a radio the
/// active workspace does not draw: an ad-hoc device has no node whose history could hold it.
pub(crate) fn begin_edit(state: &AppState, ds: u32, channel: Option<u32>) -> Option<SettingsEdit> {
    let active = state.store.active_workspace().ok()??;
    let graph = active.snapshot.graph;
    let binding = bind(&graph, &state.engine.snapshot())
        .into_iter()
        .find(|binding| binding.device_set == ds)?;
    let node = match channel {
        Some(channel) => binding
            .channels
            .iter()
            .find(|(_, bound)| *bound == channel)?
            .0
            .clone(),
        None => binding.node,
    };
    let before = live_state(state, active.info.id, &graph).ok()?;
    Some(SettingsEdit {
        workspace: active.info.id,
        node,
        graph,
        before,
    })
}

pub(crate) fn finish_edit(state: &AppState, edit: SettingsEdit) {
    let after = match live_state(state, edit.workspace, &edit.graph) {
        Ok(after) => after,
        Err(err) => {
            tracing::warn!(%err, "could not read back a settings change for the history");
            return;
        }
    };
    let step = SettingsStep {
        node: &edit.node,
        before: &edit.before,
        after: &after,
    };
    match state.store.record_settings(edit.workspace, &step) {
        Ok(true) => state.engine.emit_scope(StateScope::Workspaces),
        Ok(false) => {}
        Err(err) => tracing::warn!(%err, "could not record a settings change in the history"),
    }
}

/// Puts every bound radio and channel back where the settings say, after undo or redo reached a
/// step that moved them.
pub(crate) fn restore_settings(state: &AppState, graph: &PatchGraph, saved: &WorkspaceState) {
    let engine = &state.engine;
    for binding in bind(graph, &engine.snapshot()) {
        if let Err(err) = restore_device(engine, binding.device_set, &binding.node, saved) {
            tracing::warn!(err, node = binding.node, "could not step a radio back");
        }
        for (node, channel) in binding.channels {
            let Some(stored) = saved.channel(&node) else {
                continue;
            };
            if let Err(err) =
                engine.patch_channel(binding.device_set, channel, stored.settings.clone())
            {
                tracing::warn!(%err, node, "could not step a channel back");
            }
        }
    }
}

/// The channel plans the trunk search confirmed for itself, so a restart does not start the hunt
/// over. Only what a call actually answered is kept: an announced frequency comes back on its own
/// and a guess from the band plan is not a measurement.
fn learned_trunks(systems: &[sdrmm_wire::TrunkSystemStatus]) -> Vec<sdrmm_wire::WorkspaceTrunk> {
    systems
        .iter()
        .map(|system| sdrmm_wire::WorkspaceTrunk {
            node: system.node.clone(),
            color_code: system.color_code,
            channels: system
                .channel_map
                .iter()
                .filter(|channel| channel.source == sdrmm_wire::TrunkChannelSource::Learned)
                .map(|channel| sdrmm_wire::DmrChannelEntry {
                    lcn: channel.logical_channel,
                    freq_hz: channel.freq_hz,
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn spawn_autosave(state: &AppState) {
    let mut events = state.engine.subscribe_events();
    let state = state.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: the workspace's settings will not be saved");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if !touches_settings(&event) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
            let hard = Instant::now() + AUTOSAVE_MAX_WAIT;
            let mut idle = Instant::now() + AUTOSAVE_IDLE;
            let mut open = true;
            while open {
                tokio::select! {
                    () = tokio::time::sleep_until(idle.min(hard)) => break,
                    received = events.recv() => match received {
                        Ok(event) => {
                            if touches_settings(&event) {
                                idle = Instant::now() + AUTOSAVE_IDLE;
                            }
                        }
                        Err(RecvError::Lagged(_)) => idle = Instant::now() + AUTOSAVE_IDLE,
                        Err(RecvError::Closed) => open = false,
                    },
                }
            }
            let saving = state.clone();
            match tokio::task::spawn_blocking(move || {
                let _serialized = saving
                    .apply_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                save_active(&saving)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(%err, "could not persist the workspace's settings"),
                Err(err) => tracing::warn!(%err, "settings autosave panicked"),
            }
            if !open {
                break;
            }
        }
    });
}

fn touches_settings(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::StateChanged {
            scope: StateScope::All | StateScope::DeviceSet(_)
        }
    )
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub closed: u32,
    pub dropped_channels: u32,
    pub stopped_scans: u32,
    pub unrestored: Vec<String>,
}

pub(crate) fn reconcile(
    state: &AppState,
    incoming: &PatchGraph,
    saved: &WorkspaceState,
) -> Reconciled {
    let engine = &state.engine;
    let snapshot = engine.snapshot();
    let bindings = bind(incoming, &snapshot);
    let mut report = Reconciled::default();
    state
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();

    for set in &snapshot.device_sets {
        if bindings.iter().any(|bound| bound.device_set == set.id) {
            continue;
        }
        match engine.remove_device_set(set.id) {
            Ok(()) => report.closed += 1,
            Err(err) if err.is_not_found() => {}
            Err(err) => tracing::warn!(%err, set = set.id, "could not close a radio on switch"),
        }
    }

    for binding in &bindings {
        let Some(set) = snapshot
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        for scanner in &set.scanners {
            match engine.stop_scan(set.id, scanner.settings.channel) {
                Ok(_) => report.stopped_scans += 1,
                Err(err) => tracing::warn!(%err, set = set.id, "could not stop a sweep on switch"),
            }
        }
        for hunt in &set.hunts {
            if let Err(err) = engine.stop_hunt(set.id, hunt.settings.channel) {
                tracing::warn!(%err, set = set.id, "could not stop a hunt on switch");
            }
        }
        for channel in &set.channels {
            if binding.channels.iter().any(|(_, id)| *id == channel.id) {
                continue;
            }
            match engine.remove_channel(set.id, channel.id) {
                Ok(()) => report.dropped_channels += 1,
                Err(err) if err.is_not_found() => {}
                Err(err) => {
                    tracing::warn!(%err, set = set.id, channel = channel.id, "could not drop a channel on switch");
                }
            }
        }
        match restore_device(engine, set.id, &binding.node, saved) {
            Ok(Restored::Whole) => {}
            Ok(Restored::Partial) => report.unrestored.push(binding.node.clone()),
            Err(err) => {
                tracing::warn!(err, set = set.id, "could not restore a radio on switch");
                report.unrestored.push(binding.node.clone());
            }
        }
        for (node, channel) in &binding.channels {
            let Some(stored) = saved.channel(node) else {
                continue;
            };
            if let Err(err) = engine.patch_channel(set.id, *channel, stored.settings.clone()) {
                tracing::warn!(%err, set = set.id, channel, "could not restore a channel on switch");
                report.unrestored.push(node.clone());
            }
        }
    }
    state
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone_from(&report.unrestored);
    report
}

/// How much of what a node remembered the radio bound there could actually take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Restored {
    Whole,
    /// A different radio is bound than the one these settings were left by, and it has no bias tee
    /// or no such gain stage. What it can take lands; the node goes on remembering the rest, so the
    /// radio that can take it gets it back.
    Partial,
}

pub(crate) fn restore_device(
    engine: &sdrmm_engine::Engine,
    device_set: u32,
    node: &str,
    saved: &WorkspaceState,
) -> Result<Restored, String> {
    let Some(device) = saved.device(node) else {
        return Ok(Restored::Whole);
    };
    let capabilities = engine
        .capabilities(device_set)
        .ok_or_else(|| format!("device set {device_set} is not open"))?;
    let taken = device.settings.supported_by(&capabilities);
    let whole = taken == device.settings;
    engine
        .patch_device(device_set, taken)
        .map_err(|err| err.to_string())?;
    Ok(if whole {
        Restored::Whole
    } else {
        Restored::Partial
    })
}

pub(crate) fn channel_settings(
    node: &str,
    channel_type: &str,
    saved: &WorkspaceState,
) -> Option<ChannelSettings> {
    saved
        .channel(node)
        .filter(|channel| channel.settings.params.type_id() == channel_type)
        .map(|channel| channel.settings.clone())
        .or_else(|| ChannelSettings::default_for(channel_type))
}

#[cfg(test)]
mod tests {
    use sdrmm_device::{DeviceDriver, DeviceError, DeviceRegistry, SdrDevice};
    use sdrmm_wire::{
        DeviceInfo, DeviceNode, DeviceRef, PatchNode, Position, TrunkChannelSource,
        TrunkSystemStatus, WorkspaceSnapshot,
    };

    use super::*;

    #[derive(Default)]
    struct NamedOnly {
        adopted: std::sync::Mutex<Vec<String>>,
    }

    impl DeviceDriver for NamedOnly {
        fn id(&self) -> &'static str {
            "named"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            self.adopted
                .lock()
                .expect("test lock")
                .iter()
                .map(|key| info(key))
                .collect()
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Err(DeviceError::Io("not in this test".to_string()))
        }

        fn resolve(&self, key: &str) -> Option<DeviceInfo> {
            let mut adopted = self.adopted.lock().expect("test lock");
            if !adopted.iter().any(|held| held == key) {
                adopted.push(key.to_string());
            }
            Some(info(key))
        }
    }

    fn info(key: &str) -> DeviceInfo {
        DeviceInfo {
            driver: "named".to_string(),
            key: key.to_string(),
            label: format!("named {key}"),
            serial: None,
            profile: None,
        }
    }

    struct HardwareOnly;

    impl DeviceDriver for HardwareOnly {
        fn id(&self) -> &'static str {
            "hardware"
        }

        fn probe(&self) -> Vec<DeviceInfo> {
            Vec::new()
        }

        fn open(&self, _info: &DeviceInfo) -> Result<Box<dyn SdrDevice>, DeviceError> {
            Err(DeviceError::NotFound("nothing attached".to_string()))
        }
    }

    fn workspace_naming(reference: DeviceRef) -> WorkspaceSnapshot {
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(PatchNode {
            id: "device".to_string(),
            body: NodeBody::Device(DeviceNode {
                device: Some(reference),
                locked_streams: Vec::new(),
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        snapshot
    }

    fn engine_with_named_driver() -> std::sync::Arc<Engine> {
        let mut registry = DeviceRegistry::new();
        registry.register(10, Box::new(NamedOnly::default()));
        registry.register(20, Box::new(HardwareOnly));
        Engine::with_registry(registry, None)
    }

    #[test]
    fn a_named_device_in_a_stored_workspace_is_back_in_the_probe_after_a_restart() {
        let store = Store::open(None).expect("in-memory store");
        store
            .create_workspace(
                "remote",
                &workspace_naming(DeviceRef {
                    backend: "named".to_string(),
                    serial: None,
                    key: Some("10.0.0.5:1234".to_string()),
                }),
            )
            .expect("stored");

        let engine = engine_with_named_driver();
        assert!(engine.probe_devices().is_empty(), "nothing is discovered");
        adopt_named_devices(&engine, &store);
        assert_eq!(
            engine
                .probe_devices()
                .iter()
                .map(DeviceInfo::id)
                .collect::<Vec<_>>(),
            vec!["named:10.0.0.5:1234".to_string()]
        );
    }

    #[test]
    fn a_reference_to_absent_hardware_adopts_nothing() {
        let store = Store::open(None).expect("in-memory store");
        store
            .create_workspace(
                "bench",
                &workspace_naming(DeviceRef {
                    backend: "hardware".to_string(),
                    serial: None,
                    key: Some("00000001".to_string()),
                }),
            )
            .expect("stored");
        store
            .create_workspace(
                "serial",
                &workspace_naming(DeviceRef {
                    backend: "named".to_string(),
                    serial: Some("00000001".to_string()),
                    key: None,
                }),
            )
            .expect("stored");

        let engine = engine_with_named_driver();
        adopt_named_devices(&engine, &store);
        assert!(engine.probe_devices().is_empty());
    }

    #[test]
    fn a_workspace_with_no_device_node_costs_nothing() {
        let store = Store::open(None).expect("in-memory store");
        store
            .create_workspace("empty", &WorkspaceSnapshot::empty())
            .expect("stored");
        let engine = engine_with_named_driver();
        adopt_named_devices(&engine, &store);
        assert!(engine.probe_devices().is_empty());
    }

    fn status(
        node: &str,
        color_code: Option<u8>,
        map: &[(u16, u64, TrunkChannelSource)],
    ) -> TrunkSystemStatus {
        TrunkSystemStatus {
            node: node.to_owned(),
            detected: None,
            carriers: 1,
            control: None,
            followers: Vec::new(),
            problems: Vec::new(),
            channel_map: map
                .iter()
                .map(
                    |(logical_channel, freq_hz, source)| sdrmm_wire::TrunkChannel {
                        logical_channel: *logical_channel,
                        freq_hz: *freq_hz,
                        source: *source,
                        confidence: 100,
                    },
                )
                .collect(),
            probes: Vec::new(),
            searching: 0,
            candidates: 0,
            other_control_hz: Vec::new(),
            color_code,
        }
    }

    #[test]
    fn only_the_frequencies_the_search_confirmed_are_written_down() {
        let systems = vec![status(
            "sys",
            Some(10),
            &[
                (17, 451_012_500, TrunkChannelSource::Learned),
                (18, 451_025_000, TrunkChannelSource::Announced),
                (19, 451_037_500, TrunkChannelSource::Manual),
                (20, 451_050_000, TrunkChannelSource::Predicted),
            ],
        )];

        let saved = learned_trunks(&systems);

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].color_code, Some(10));
        assert_eq!(
            saved[0].channels,
            vec![sdrmm_wire::DmrChannelEntry {
                lcn: 17,
                freq_hz: 451_012_500,
            }],
            "a guess or an announcement was kept as though it had been measured"
        );
    }

    #[test]
    fn a_learned_plan_survives_a_restart_and_comes_back_to_the_same_site() {
        let mut state = sdrmm_wire::WorkspaceState::new();
        state.merge_trunks(learned_trunks(&[status(
            "sys",
            Some(10),
            &[(17, 451_012_500, TrunkChannelSource::Learned)],
        )]));

        let same_site =
            crate::trunking::learned_for(&state, "sys", &[status("sys", Some(10), &[])]);
        assert_eq!(same_site.len(), 1);
        assert_eq!(same_site[0].freq_hz, 451_012_500);

        let other_site =
            crate::trunking::learned_for(&state, "sys", &[status("sys", Some(3), &[])]);
        assert!(
            other_site.is_empty(),
            "a plan learned at one site was handed to another"
        );
    }

    #[test]
    fn a_site_that_has_not_named_itself_yet_reads_nothing_back() {
        let mut state = sdrmm_wire::WorkspaceState::new();
        state.merge_trunks(learned_trunks(&[status(
            "sys",
            Some(10),
            &[(17, 451_012_500, TrunkChannelSource::Learned)],
        )]));

        assert!(
            crate::trunking::learned_for(&state, "sys", &[status("sys", None, &[])]).is_empty()
        );
    }

    fn dmr_channel(id: u32, node: Option<&str>) -> ChannelInfo {
        ChannelInfo {
            id,
            stream: 0,
            node: node.map(str::to_owned),
            settings: ChannelSettings::default_for("dmr").expect("dmr"),
            out_of_band: false,
            audio_recordings: Vec::new(),
            baseband_recording: None,
            network_export: None,
        }
    }

    fn radio_with(channels: Vec<ChannelInfo>) -> DeviceSet {
        DeviceSet {
            id: 1,
            device: DeviceInfo {
                driver: "virtual".to_owned(),
                key: "siggen".to_owned(),
                label: "Siggen".to_owned(),
                serial: None,
                profile: None,
            },
            capabilities: sdrmm_wire::Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
                sample_rate_ranges: Vec::new(),
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                bandwidth_ranges: Vec::new(),
                bandwidth_auto: false,
                bias_tee: false,
                agc: sdrmm_wire::Agc::None,
                extra: Vec::new(),
                ppm: false,
                duplex: sdrmm_wire::Duplex::RxOnly,
                rx_streams: 1,
                tx_streams: 0,
                per_stream: sdrmm_wire::StreamScope::default(),
                directional: None,
                dc_artifact: sdrmm_wire::DcArtifact::Operator,
                hardware_sweep: false,
                coherence: sdrmm_wire::Coherence::None,
                noise_source: false,
            },
            settings: sdrmm_wire::DeviceSettings::default(),
            status: sdrmm_wire::DeviceSetStatus::Running,
            channels,
            overruns: 0,
            clipping: Vec::new(),
            error: None,
            fault: None,
            refused: None,
            recording: None,
            network_export: None,
            time_machine: None,
            scanners: Vec::new(),
            hunts: Vec::new(),
            playback: None,
            extra_lane: None,
            agc_gains: Vec::new(),
        }
    }

    fn dmr_graph() -> PatchGraph {
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes.push(PatchNode {
            id: "voice".to_owned(),
            body: NodeBody::Channel(sdrmm_wire::ChannelNode {
                channel_type: "dmr".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        snapshot.graph.edges.push(sdrmm_wire::PatchEdge {
            from: sdrmm_wire::PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: sdrmm_wire::PortRef {
                node: "voice".to_owned(),
                port: "iq".to_owned(),
            },
        });
        snapshot.graph
    }

    #[test]
    fn a_decoder_node_never_adopts_a_trunk_systems_own_channel() {
        let graph = dmr_graph();
        let follower = radio_with(vec![dmr_channel(4, None)]);
        assert_eq!(
            bind_channels(&graph, "device", &follower, &HashSet::new()),
            vec![("voice".to_owned(), 4)],
            "a channel nobody opened for a node is adopted"
        );
        assert!(
            bind_channels(&graph, "device", &follower, &HashSet::from([4])).is_empty(),
            "a trunk system's follower stays with the system"
        );
        let state = StateSnapshot {
            device_sets: vec![follower],
            trunk_systems: vec![TrunkSystemStatus {
                node: "trunk".to_owned(),
                detected: None,
                carriers: 1,
                control: None,
                followers: vec![sdrmm_wire::TrunkFollower {
                    device_set: 1,
                    channel: 4,
                    logical_channel: None,
                    slot: 1,
                    freq_hz: 451_000_000,
                }],
                problems: Vec::new(),
                channel_map: Vec::new(),
                probes: Vec::new(),
                searching: 0,
                candidates: 0,
                other_control_hz: Vec::new(),
                color_code: None,
            }],
            revision: 1,
        };
        assert_eq!(trunk_channels(&state, 1), HashSet::from([4]));
        assert!(trunk_channels(&state, 2).is_empty());
    }
}
