use std::time::Duration;

use sdrmm_engine::{Lane, Placeable};
use sdrmm_wire::{
    ChannelInfo, DeviceSet, NodeBody, PatchApplyReport, PatchGraph, PatchRefusal, ServerEvent,
    StateScope, StateSnapshot, WorkspaceState,
};
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{AppState, workspace};

const SETTLE_IDLE: Duration = Duration::from_millis(150);
const SETTLE_MAX_WAIT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Carried {
    lane: Lane,
    channel: u32,
}

struct Slot {
    decoder: Placeable,
    carried: Option<Carried>,
}

pub(crate) struct Plan {
    slots: Vec<Slot>,
    pub refused: Vec<PatchRefusal>,
}

impl Plan {
    fn decoders(&self) -> Vec<Placeable> {
        self.slots.iter().map(|slot| slot.decoder.clone()).collect()
    }

    fn flexible_only(mut self) -> Self {
        self.slots.retain(|slot| slot.decoder.lanes.len() > 1);
        self
    }
}

fn carried_by<'a>(
    state: &'a StateSnapshot,
    node: &str,
    channel_type: &str,
) -> Option<(Carried, &'a DeviceSet, &'a ChannelInfo)> {
    state.device_sets.iter().find_map(|set| {
        set.channels
            .iter()
            .find(|channel| {
                channel.node.as_deref() == Some(node)
                    && channel.settings.params.type_id() == channel_type
            })
            .map(|channel| {
                let carried = Carried {
                    lane: Lane {
                        device_set: set.id,
                        stream: channel.stream,
                    },
                    channel: channel.id,
                };
                (carried, set, channel)
            })
    })
}

fn holds_channel(set: &DeviceSet, channel: &ChannelInfo) -> bool {
    set.scanner
        .as_ref()
        .is_some_and(|scan| scan.settings.channel == channel.id)
        || set
            .hunt
            .as_ref()
            .is_some_and(|hunt| hunt.settings.channel == channel.id)
        || channel.audio_recording.is_some()
        || channel.baseband_recording.is_some()
        || channel.network_export.is_some()
}

pub(crate) fn plan(
    graph: &PatchGraph,
    state: &StateSnapshot,
    bound: &[(String, u32)],
    saved: &WorkspaceState,
) -> Plan {
    let mut slots = Vec::new();
    let mut refused = Vec::new();
    for node in &graph.nodes {
        let NodeBody::Channel(channel) = &node.body else {
            continue;
        };
        let lanes: Vec<Lane> = graph
            .lanes_of(&node.id)
            .into_iter()
            .filter_map(|(device_node, stream)| {
                let (_, device_set) = bound.iter().find(|(held, _)| held == device_node)?;
                Some(Lane {
                    device_set: *device_set,
                    stream,
                })
            })
            .collect();
        if lanes.is_empty() {
            continue;
        }
        let live = carried_by(state, &node.id, &channel.channel_type);
        let settings = live
            .map(|(_, _, channel)| channel.settings.clone())
            .or_else(|| workspace::channel_settings(&node.id, &channel.channel_type, saved));
        let Some(settings) = settings else {
            refused.push(PatchRefusal {
                node: node.id.clone(),
                reason: format!("this build has no channel type {:?}", channel.channel_type),
            });
            continue;
        };
        slots.push(Slot {
            decoder: Placeable {
                node: node.id.clone(),
                settings,
                lanes,
                held: live.map(|(carried, _, _)| carried.lane),
                pinned: live.is_some_and(|(_, set, channel)| holds_channel(set, channel)),
            },
            carried: live.map(|(carried, _, _)| carried),
        });
    }
    Plan { slots, refused }
}

fn open(app: &AppState, slot: &Slot, lane: Lane, report: &mut PatchApplyReport) -> Option<Carried> {
    let decoder = &slot.decoder;
    match app.engine.add_channel_for(
        lane.device_set,
        lane.stream,
        decoder.settings.clone(),
        Some(&decoder.node),
    ) {
        Ok(channel) => {
            report.created += 1;
            Some(Carried { lane, channel })
        }
        Err(err) => {
            report.refused.push(PatchRefusal {
                node: decoder.node.clone(),
                reason: err.to_string(),
            });
            None
        }
    }
}

fn close(app: &AppState, slot: &Slot, carried: Carried, report: &mut PatchApplyReport) -> bool {
    match app
        .engine
        .remove_channel(carried.lane.device_set, carried.channel)
    {
        Ok(()) => {
            report.closed += 1;
            true
        }
        Err(err) if err.is_not_found() => true,
        Err(err) => {
            report.refused.push(PatchRefusal {
                node: slot.decoder.node.clone(),
                reason: err.to_string(),
            });
            false
        }
    }
}

pub(crate) fn settle(app: &AppState, plan: &Plan, report: &mut PatchApplyReport) {
    report.refused.extend(plan.refused.iter().cloned());
    let placements = app.engine.place_channels(&plan.decoders());
    for slot in &plan.slots {
        let target = placements
            .iter()
            .find(|placement| placement.node == slot.decoder.node)
            .map(|placement| placement.lane);
        match (slot.carried, target) {
            (Some(carried), Some(lane)) if carried.lane == lane => {}
            (Some(carried), Some(lane)) => {
                if let Some(created) = open(app, slot, lane, report)
                    && !close(app, slot, carried, report)
                {
                    close(app, slot, created, report);
                }
            }
            (None, Some(lane)) => {
                open(app, slot, lane, report);
            }
            (None, None) => {
                if let Some(lane) = slot.decoder.lanes.first() {
                    open(app, slot, *lane, report);
                }
            }
            (Some(_), None) => report.refused.push(PatchRefusal {
                node: slot.decoder.node.clone(),
                reason: "no wired receive stream is available".to_owned(),
            }),
        }
    }
}

fn adopt_channels(app: &AppState, graph: &PatchGraph, report: &mut PatchApplyReport) {
    let state = app.engine.snapshot();
    for binding in workspace::bind(graph, &state) {
        let Some(set) = state
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        for (node, id) in binding.channels {
            let Some(channel) = set.channels.iter().find(|channel| channel.id == id) else {
                continue;
            };
            if channel.node.is_some()
                || graph.lanes_of(&node).is_empty()
                || carried_by(&state, &node, channel.settings.params.type_id()).is_some()
            {
                continue;
            }
            if let Err(err) = app.engine.bind_channel_node(set.id, id, &node) {
                report.refused.push(PatchRefusal {
                    node,
                    reason: err.to_string(),
                });
            }
        }
    }
}

pub(crate) fn settle_workspace(
    app: &AppState,
    graph: &PatchGraph,
    saved: &WorkspaceState,
    report: &mut PatchApplyReport,
) {
    adopt_channels(app, graph, report);
    let state = app.engine.snapshot();
    let bound = workspace::bind_devices(graph, &state);
    let plan = plan(graph, &state, &bound, saved);
    settle(app, &plan, report);
}

pub(crate) fn settle_active(app: &AppState) -> bool {
    let active = match app.store.active_workspace() {
        Ok(Some(active)) => active,
        Ok(None) => return false,
        Err(err) => {
            tracing::warn!(%err, "could not read the active workspace");
            return false;
        }
    };
    let graph = &active.snapshot.graph;
    let flexible = graph.nodes.iter().any(|node| {
        matches!(node.body, NodeBody::Channel(_)) && graph.lanes_of(&node.id).len() > 1
    });
    if !flexible {
        return false;
    }
    let saved = match app.store.workspace_state(active.info.id) {
        Ok(saved) => saved,
        Err(err) => {
            tracing::warn!(%err, "could not read the workspace to place its decoders");
            return false;
        }
    };
    let state = app.engine.snapshot();
    let bound = workspace::bind_devices(graph, &state);
    let plan = plan(graph, &state, &bound, &saved).flexible_only();
    let mut report = PatchApplyReport::default();
    settle(app, &plan, &mut report);
    for refusal in &report.refused {
        tracing::warn!(
            node = refusal.node,
            reason = refusal.reason,
            "a decoder could not be placed"
        );
    }
    report.created > 0 || report.closed > 0
}

fn moves_a_radio(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::StateChanged {
            scope: StateScope::All | StateScope::DeviceSet(_)
        }
    )
}

pub(crate) fn spawn_settling(state: &AppState) {
    let mut events = state.engine.subscribe_events();
    let state = state.clone();
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: decoders will not follow their radios");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) if !moves_a_radio(&event) => continue,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
            let hard = Instant::now() + SETTLE_MAX_WAIT;
            let mut idle = Instant::now() + SETTLE_IDLE;
            let mut open = true;
            while open {
                tokio::select! {
                    () = tokio::time::sleep_until(idle.min(hard)) => break,
                    received = events.recv() => match received {
                        Ok(event) => {
                            if moves_a_radio(&event) {
                                idle = Instant::now() + SETTLE_IDLE;
                            }
                        }
                        Err(RecvError::Lagged(_)) => idle = Instant::now() + SETTLE_IDLE,
                        Err(RecvError::Closed) => open = false,
                    },
                }
            }
            let settling = state.clone();
            if let Err(err) = tokio::task::spawn_blocking(move || {
                let _serialized = settling
                    .apply_gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                settle_active(&settling)
            })
            .await
            {
                tracing::warn!(%err, "decoder placement panicked");
            }
            if !open {
                break;
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/placement.rs"]
mod tests;
