use std::time::Duration;

use sdrmm_engine::Engine;
use sdrmm_wire::{
    ChannelParams, ChannelSettings, DeviceSet, NodeBody, PatchGraph, ServerEvent, StateScope,
    StateSnapshot, WorkspaceChannel, WorkspaceDevice, WorkspaceState,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{
    AppState,
    store::{Store, StoreError},
};

/// How long the settings must stay still before they are written, and the longest a change may
/// go unwritten regardless. Both are needed: a scroll-wheel tune emits a change per detent, so
/// idle-only debouncing never writes while the operator is spinning the dial, and interval-only
/// writing costs a transaction per tick of an idle workspace.
const AUTOSAVE_IDLE: Duration = Duration::from_secs(2);
const AUTOSAVE_MAX_WAIT: Duration = Duration::from_secs(15);

/// A device node bound to a live device set, and its channel nodes bound to live channels.
pub(crate) struct DeviceBinding {
    pub node: String,
    pub device_set: u32,
    /// Channel node id → live channel id, for the nodes that have one.
    pub channels: Vec<(String, u32)>,
}

/// Hand every device a stored workspace names, but no probe can find, back to the driver that can
/// address it.
///
/// Only the network backends answer, and they are the reason this runs at all: a remote receiver is
/// named by an operator rather than discovered, so after a restart nothing would put its endpoint
/// back into the probe list — and apply only opens devices that are *in* that list, so a device
/// node bound to one would sit at "not attached" forever with the radio online the whole time.
/// The stored workspace is where those endpoints live, which makes this the one place that can
/// restore them.
///
/// Best-effort by nature: a driver that cannot address a key says so by returning nothing, and an
/// unreadable workspace costs its endpoints rather than the startup.
pub(crate) fn adopt_named_devices(engine: &Engine, store: &Store) {
    let Ok(workspaces) = store.list_workspaces() else {
        return;
    };
    for info in &workspaces.workspaces {
        let Ok(detail) = store.workspace(info.id) else {
            continue;
        };
        for node in detail.snapshot.graph.device_nodes() {
            let NodeBody::Device(device) = &node.body else {
                continue;
            };
            // A reference carries a key only where the driver exposes no serial, which is exactly
            // the shape a network endpoint has.
            let Some(reference) = device.device.as_ref().filter(|d| d.key.is_some()) else {
                continue;
            };
            let Some(key) = &reference.key else { continue };
            if let Some(adopted) = engine.adopt_device(&format!("{}:{key}", reference.backend)) {
                tracing::info!(device = %adopted.id(), workspace = info.id, "adopted a named device");
            }
        }
    }
}

pub(crate) fn bind_devices(graph: &PatchGraph, state: &StateSnapshot) -> Vec<(String, u32)> {
    let mut bound = Vec::new();
    let mut claimed: Vec<u32> = Vec::new();
    for node in graph.device_nodes() {
        let NodeBody::Device(device) = &node.body else {
            continue;
        };
        let Some(reference) = &device.device else {
            continue;
        };
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

/// Match a device node's channel nodes to the live channels of the set it is bound to.
///
/// Apply's rule exactly (`crate::rest::bring_up`): a node takes the first unclaimed channel of
/// the type it declares *on the stream its wire taps*, in stored order. The stream is part of
/// the key because two same-type nodes on different streams of one radio would otherwise claim
/// each other's channels and swap settings on capture. Nodes without a live channel are
/// omitted — for capture there is nothing to record, and apply creates them separately.
fn bind_channels(graph: &PatchGraph, device_node: &str, set: &DeviceSet) -> Vec<(String, u32)> {
    let mut live: Vec<(u32, &str, u32)> = set
        .channels
        .iter()
        .map(|channel| {
            (
                channel.id,
                channel.settings.params.type_id(),
                channel.stream,
            )
        })
        .collect();
    let mut bound = Vec::new();
    for (node, stream) in graph.channels_of(device_node) {
        let NodeBody::Channel(channel) = &node.body else {
            continue;
        };
        if let Some(at) = live.iter().position(|(_, type_id, live_stream)| {
            *type_id == channel.channel_type && *live_stream == stream
        }) {
            bound.push((node.id.clone(), live.remove(at).0));
        }
    }
    bound
}

pub(crate) fn bind(graph: &PatchGraph, state: &StateSnapshot) -> Vec<DeviceBinding> {
    bind_devices(graph, state)
        .into_iter()
        .filter_map(|(node, device_set)| {
            let set = state.device_sets.iter().find(|set| set.id == device_set)?;
            Some(DeviceBinding {
                channels: bind_channels(graph, &node, set),
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
) -> Vec<WorkspaceDevice> {
    bind(graph, state)
        .into_iter()
        .filter_map(|binding| {
            let set = state
                .device_sets
                .iter()
                .find(|set| set.id == binding.device_set)?;
            if set.scanner.is_some() || unrestored.contains(&binding.node) {
                return None;
            }
            Some(WorkspaceDevice {
                node: binding.node,
                settings: set.settings.clone(),
                channels: binding
                    .channels
                    .into_iter()
                    .filter_map(|(node, id)| {
                        let channel = set.channels.iter().find(|channel| channel.id == id)?;
                        Some(WorkspaceChannel {
                            node,
                            settings: channel.settings.clone(),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

/// Fold the engine's current settings into the active workspace's stored state.
///
/// Returns without writing when the workspace has no active workspace — a database whose last
/// workspace was deleted has nowhere to put this, and the next open re-seeds one.
pub(crate) fn save_active(state: &AppState) -> Result<(), StoreError> {
    let Some(active) = state.store.active_workspace()? else {
        return Ok(());
    };
    let graph = &active.snapshot.graph;
    let mut stored = state.store.workspace_state(active.info.id)?;
    let unrestored = state
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let captured = capture(graph, &state.engine.snapshot(), &unrestored);
    stored.merge(captured);
    // A node undo can bring back is a node whose settings are still needed: pruning on the graph
    // alone would return a restored channel at its type's defaults rather than where it was.
    let recoverable = state.store.history_nodes(active.info.id)?;
    stored.retain_nodes(|node| graph.node(node).is_some() || recoverable.contains(node));
    state.store.put_workspace_state(active.info.id, &stored)
}

/// Persist the workspace's settings shortly after they stop changing.
///
/// Driven off the engine's own event stream rather than the endpoints that mutate: tuning arrives
/// over REST, MCP and the scanner alike, and one writer behind all of them cannot be bypassed by
/// a caller that forgets to save. It emits no scope of its own — nothing reads this row live, and
/// an emit would be a change event feeding the loop that produced it.
pub(crate) fn spawn_autosave(state: &AppState) {
    let mut events = state.engine.subscribe_events();
    let state = state.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // Same guard as the decoded encoder, and just as loud: a workspace that silently stops
        // remembering where it was tuned is only discovered on the next restart.
        tracing::warn!("no runtime in context: the workspace's settings will not be saved");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        loop {
            // Nothing pending: block until something worth saving happens.
            match events.recv().await {
                Ok(event) if !touches_settings(&event) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
            // Idle window, reset by every further change; `hard` bounds it so a dial nobody stops
            // turning still reaches disk.
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

/// Whether an event can have changed something worth persisting. Scanner progress is the noisy
/// one this excludes: it fires every dwell and the tuning it reports is the sweep's, not the
/// operator's (see [`capture`]).
fn touches_settings(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::StateChanged {
            scope: StateScope::All | StateScope::DeviceSet(_)
        }
    )
}

/// What activating a workspace did to the hardware. Counts rather than ids: nothing downstream
/// acts on them, and a switch that quietly closed four radios is exactly the thing that has to
/// show up in a log (CLAUDE.md no-silent-failure).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Reconciled {
    pub closed: u32,
    pub dropped_channels: u32,
    pub stopped_scans: u32,
    /// Nodes whose saved settings the engine refused — a rate locked by a live recording is the
    /// case that reaches here. Their live settings are the *previous* workspace's, so a capture
    /// that wrote them back would destroy what this workspace had saved; [`capture`] skips them
    /// until a later restore succeeds.
    pub unrestored: Vec<String>,
}

/// Bring the hardware in line with the workspace that just became active.
///
/// Apply is additive on purpose: a second browser loading a workspace must never close a radio
/// somebody else is using (`crate::rest::bring_up`). Activation is the opposite gesture — the
/// operator asked for *this* workspace — so what the previous one left running is not the
/// incoming one's to inherit. Exactly one workspace is active, and after this the hardware says
/// so: radios the graph does not name are closed, channels it does not draw are dropped from the
/// radios it keeps, and a sweep it does not draw gives the tuning back.
///
/// Only the subtractive half lives here. Opening the radios the workspace names and creating the
/// channels it draws stays with apply, which runs next and restores the settings of every set it
/// opens itself. The surviving sets are this function's to restore, because apply deliberately
/// will not retune a radio that was already open — without this, two workspaces naming one radio
/// would each inherit the other's tuning and never get their own back.
pub(crate) fn reconcile(
    state: &AppState,
    incoming: &PatchGraph,
    saved: &WorkspaceState,
) -> Reconciled {
    let engine = &state.engine;
    let snapshot = engine.snapshot();
    let bindings = bind(incoming, &snapshot);
    let mut report = Reconciled::default();
    // Cleared here, not appended to: a switch that restores cleanly must lift the block a
    // previous failed one left, or that workspace never saves its tuning again.
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
            // Already gone is the outcome asked for; anything else is a radio still running that
            // this workspace does not draw, and that has to be visible.
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
        if set.scanner.is_some() {
            match engine.stop_scan(set.id) {
                Ok(_) => report.stopped_scans += 1,
                Err(err) => tracing::warn!(%err, set = set.id, "could not stop a sweep on switch"),
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
        if let Err(err) = restore_device(engine, set.id, &binding.node, saved) {
            tracing::warn!(err, set = set.id, "could not restore a radio on switch");
            report.unrestored.push(binding.node.clone());
        }
        // A channel that *survived* the switch keeps the outgoing workspace's offset, squelch and
        // params unless it is patched here: apply skips a live channel of the right type
        // (`crate::rest::bring_up`), so nothing downstream would ever hand it this workspace's
        // settings. Same failure as the device half, one level down.
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

pub(crate) fn restore_device(
    engine: &sdrmm_engine::Engine,
    device_set: u32,
    node: &str,
    saved: &WorkspaceState,
) -> Result<(), String> {
    let Some(device) = saved.device(node) else {
        return Ok(());
    };
    engine
        .patch_device(device_set, device.settings.clone())
        .map_err(|err| err.to_string())
}

/// The settings a channel node should be created with: the ones it last had, or its type's
/// defaults if it has never been captured.
/// Saved settings whose params are a *different* type than the node declares are ignored: the
/// node is what the workspace draws, and a channel whose mode was changed out of band through the
/// REST surface must not silently redraw it.
pub(crate) fn channel_settings(
    node: &str,
    channel_type: &str,
    saved: &WorkspaceState,
) -> Option<ChannelSettings> {
    let stored = saved
        .channel(node)
        .filter(|channel| channel.settings.params.type_id() == channel_type);
    if let Some(channel) = stored {
        return Some(channel.settings.clone());
    }
    Some(ChannelSettings {
        offset_hz: 0.0,
        squelch_db: None,
        params: ChannelParams::default_for(channel_type)?,
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_device::{DeviceDriver, DeviceError, DeviceRegistry, SdrDevice};
    use sdrmm_wire::{DeviceInfo, DeviceNode, DeviceRef, PatchNode, Position, WorkspaceSnapshot};

    use super::*;

    /// A backend that can address any key but discovers nothing — the shape of a network client,
    /// without a socket.
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

    /// A driver that only ever reports what is attached, which is every real-hardware backend.
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

    /// The restart case this whole path exists for: the endpoint lives only in the stored
    /// workspace, and until it is handed back to its driver no probe reports it — so apply, which
    /// only opens what the probe lists, would leave the node waiting for a radio that is online.
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

    /// A workspace naming an unplugged dongle must not make the device list claim it is there:
    /// only a driver that can address a key by name answers, and hardware backends never do.
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
        // …and one that names a serial rather than a key carries no endpoint to adopt at all.
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
}
