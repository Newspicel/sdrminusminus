//! Keeping a workspace's *settings*, not just its shape (PLAN §7).
//!
//! A workspace stores the patch: which radios, which channels, how they are wired and where the
//! faces sit. Until now that was all that survived a restart — applying a workspace reopened the
//! radios and recreated the channels at `ChannelParams::default_for`, so a morning spent setting
//! offsets, squelch and gains came back neutral.
//!
//! This module is the missing half. It binds the graph's nodes to the engine's live device sets
//! and channels, captures what those are set to into a [`WorkspaceState`] keyed by node id, and
//! hands the settings back on the next apply. The binding rules are apply's own — a device node
//! claims the first unclaimed set its [`DeviceRef`] matches, in stored node order; a channel node
//! claims the first unclaimed channel of its (type, stream) — which is what makes capture and
//! restore agree on which node is which without storing a per-run id (`crate::rest::bring_up`).

use std::time::Duration;

use sdrmm_wire::{
    ChannelParams, ChannelSettings, DeviceSet, NodeBody, PatchGraph, ServerEvent, StateScope,
    StateSnapshot, WorkspaceChannel, WorkspaceDevice, WorkspaceState,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{AppState, store::StoreError};

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

/// Match the graph's device nodes to device sets that are already open.
///
/// Stored node order, one set per node: two nodes naming the same serial-less clone bind to one
/// set each rather than both to the first, which is what makes the assignment stable across runs
/// (CANVAS §3). Nodes with no open set are simply absent from the result — opening one is
/// apply's job and never capture's.
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

/// Bind the whole graph against the engine's current state.
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

/// What the engine currently has every bound node set to.
///
/// A set the scanner owns is skipped: a running scan retunes the device every dwell (PLAN §4), so
/// its centre frequency is wherever the sweep happens to be and not something the operator asked
/// for. Capturing it would persist a step of the sweep as the workspace's tuning.
///
/// A node in `unrestored` is skipped for the same reason one level up: the switch could not hand
/// that radio this workspace's settings, so what it is running is the *previous* workspace's, and
/// writing it back would overwrite the tuning this workspace had saved.
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
    stored.retain_nodes(|node| graph.node(node).is_some());
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
                // The gate activation and apply take. Without it a debounced save can land in
                // the middle of a switch — after the active row moved but before the reconcile
                // finished — and write half-reconciled state into the incoming workspace's row.
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
        // A sweep owns the set's centre frequency (PLAN §18) and `patch_device` refuses every
        // client retune while one runs, so it has to stop before the restore below or the whole
        // settings delta is dropped and the workspace comes up on the sweep's dial.
        //
        // Unconditionally, even when the incoming workspace draws a scanner of its own: the sweep
        // that is running belongs to the *outgoing* workspace — its ranges, its dwell — and a scan
        // is never persisted, so an incoming scanner node means an idle scanner, not this one.
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
            // The workspace is now on settings that are not its own, and the autosave would
            // shortly write them into its row — losing the tuning this whole feature exists to
            // keep. Say so where a capture can see it (`capture` skips these nodes) rather than
            // letting the loss happen quietly.
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

/// Hand a device set the settings its node last had.
///
/// Apply calls this only for sets *it* opened: apply is additive and idempotent by design — it
/// never disturbs a set someone else is already using (`crate::rest::bring_up`) — and
/// re-tuning a running radio because a second browser loaded the workspace would be exactly that.
/// [`reconcile`] calls it for the sets an explicit switch keeps, where retuning is the point.
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
