//! Keeping a station's *settings*, not just its shape (PLAN §7).
//!
//! A workspace stores the patch: which radios, which channels, how they are wired and where the
//! faces sit. Until now that was all that survived a restart — applying a station reopened the
//! radios and recreated the channels at `ChannelParams::default_for`, so a morning spent setting
//! offsets, squelch and gains came back neutral.
//!
//! This module is the missing half. It binds the graph's nodes to the engine's live device sets
//! and channels, captures what those are set to into a [`StationState`] keyed by node id, and
//! hands the settings back on the next apply. The binding rules are apply's own — a device node
//! claims the first unclaimed set its [`DeviceRef`] matches, in stored node order; a channel node
//! claims the first unclaimed channel of its type — which is what makes capture and restore agree
//! on which node is which without storing a per-run id (`crate::rest::apply_station`).

use std::time::Duration;

use sdrmm_wire::{
    ChannelParams, ChannelSettings, DeviceSet, NodeBody, PatchGraph, ServerEvent, StateScope,
    StateSnapshot, StationChannel, StationDevice, StationState,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{AppState, store::StoreError};

/// How long the settings must stay still before they are written, and the longest a change may
/// go unwritten regardless. Both are needed: a scroll-wheel tune emits a change per detent, so
/// idle-only debouncing never writes while the operator is spinning the dial, and interval-only
/// writing costs a transaction per tick of an idle station.
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
/// Apply's rule exactly (`crate::rest::apply_station`): a node takes the first unclaimed channel
/// of the type it declares, in stored order. Nodes without a live channel are omitted — for
/// capture there is nothing to record, and apply creates them separately.
fn bind_channels(graph: &PatchGraph, device_node: &str, set: &DeviceSet) -> Vec<(String, u32)> {
    let mut live: Vec<(u32, &str)> = set
        .channels
        .iter()
        .map(|channel| (channel.id, channel.settings.params.type_id()))
        .collect();
    let mut bound = Vec::new();
    for node in graph.channels_of(device_node) {
        let NodeBody::Channel(channel) = &node.body else {
            continue;
        };
        if let Some(at) = live
            .iter()
            .position(|(_, type_id)| *type_id == channel.channel_type)
        {
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
/// for. Capturing it would persist a step of the sweep as the station's tuning.
pub(crate) fn capture(graph: &PatchGraph, state: &StateSnapshot) -> Vec<StationDevice> {
    bind(graph, state)
        .into_iter()
        .filter_map(|binding| {
            let set = state
                .device_sets
                .iter()
                .find(|set| set.id == binding.device_set)?;
            if set.scanner.is_some() {
                return None;
            }
            Some(StationDevice {
                node: binding.node,
                settings: set.settings.clone(),
                channels: binding
                    .channels
                    .into_iter()
                    .filter_map(|(node, id)| {
                        let channel = set.channels.iter().find(|channel| channel.id == id)?;
                        Some(StationChannel {
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
/// Returns without writing when the station has no active workspace — a database whose last
/// workspace was deleted has nowhere to put this, and the next open re-seeds one.
pub(crate) fn save_active(state: &AppState) -> Result<(), StoreError> {
    let Some(active) = state.store.active_workspace()? else {
        return Ok(());
    };
    let graph = &active.snapshot.graph;
    let mut stored = state.store.station_state(active.info.id)?;
    let captured = capture(graph, &state.engine.snapshot());
    stored.merge(captured);
    stored.retain_nodes(|node| graph.node(node).is_some());
    state.store.put_station_state(active.info.id, &stored)
}

/// Persist the station's settings shortly after they stop changing.
///
/// Driven off the engine's own event stream rather than the endpoints that mutate: tuning arrives
/// over REST, MCP and the scanner alike, and one writer behind all of them cannot be bypassed by
/// a caller that forgets to save. It emits no scope of its own — nothing reads this row live, and
/// an emit would be a change event feeding the loop that produced it.
pub(crate) fn spawn_autosave(state: &AppState) {
    let mut events = state.engine.subscribe_events();
    let state = state.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        // Same guard as the decoded encoder, and just as loud: a station that silently stops
        // remembering where it was tuned is only discovered on the next restart.
        tracing::warn!("no runtime in context: the station's settings will not be saved");
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
            match tokio::task::spawn_blocking(move || save_active(&saving)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => tracing::warn!(%err, "could not persist the station's settings"),
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

/// Hand a freshly opened device set the settings its node last had.
///
/// Applied only to sets *this* apply opened. Apply is additive and idempotent by design — it
/// never disturbs a set someone else is already using (`crate::rest::apply_station`) — and
/// re-tuning a running radio because a second browser loaded the station would be exactly that.
pub(crate) fn restore_device(
    engine: &sdrmm_engine::Engine,
    device_set: u32,
    node: &str,
    saved: &StationState,
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
/// node is what the station draws, and a channel whose mode was changed out of band through the
/// REST surface must not silently redraw it.
pub(crate) fn channel_settings(
    node: &str,
    channel_type: &str,
    saved: &StationState,
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
