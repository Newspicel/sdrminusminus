use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, PoisonError},
};

use sdrmm_wire::{DecoderEvent, NodeBody, PatchGraph, PositionFix, ServerEvent};
use tokio::sync::broadcast::{self, error::RecvError};

use crate::{AppState, decoded::Decoded, df_fusion, events::Routed};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Guided {
    Triangulation(String),
    Hunt(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Table {
    triangulations: HashSet<String>,
    guides: HashMap<String, Vec<Guided>>,
}

impl Table {
    pub(crate) fn from_graph(graph: &PatchGraph) -> Self {
        let mut table = Self::default();
        for node in &graph.nodes {
            let guided = match node.body {
                NodeBody::Triangulation(_) => {
                    table.triangulations.insert(node.id.clone());
                    Guided::Triangulation(node.id.clone())
                }
                NodeBody::Hunt(_) => Guided::Hunt(node.id.clone()),
                _ => continue,
            };
            if let Some(source) = graph.position_source(&node.id) {
                table
                    .guides
                    .entry(source.to_owned())
                    .or_default()
                    .push(guided);
            }
        }
        table
    }

    pub(crate) fn is_triangulation(&self, node: &str) -> bool {
        self.triangulations.contains(node)
    }

    pub(crate) fn guided_by(&self, position: &str) -> &[Guided] {
        self.guides.get(position).map_or(&[], Vec::as_slice)
    }
}

fn table(state: &AppState) -> Arc<Table> {
    state
        .fusion
        .routes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

pub(crate) fn reconcile(state: &AppState, graph: &PatchGraph) -> Vec<(String, String)> {
    let table = Table::from_graph(graph);
    let now_s = df_fusion::now_s();
    for triangulation in &table.triangulations {
        let fix = graph
            .position_source(triangulation)
            .and_then(|source| state.gps.fix(source));
        df_fusion::guide(state, triangulation, fix, now_s);
    }
    *state
        .fusion
        .routes
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Arc::new(table);
    Vec::new()
}

pub(crate) fn start(state: &AppState) {
    match state.store.active_workspace() {
        Ok(Some(active)) => {
            reconcile(state, &active.snapshot.graph);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::error!(%error, "could not read the active workspace for bearing routes")
        }
    }
    let records = state.decoded.subscribe();
    let positions = state.gps.subscribe();
    let routed = state.clone();
    let _detached = crate::spawn_task("sdrmm-fusion-routes", move || {
        run(routed, records, positions)
    });
}

async fn run(
    state: AppState,
    mut records: broadcast::Receiver<Decoded>,
    mut positions: broadcast::Receiver<ServerEvent>,
) {
    loop {
        tokio::select! {
            record = records.recv() => match record {
                Ok(Decoded::Record(routed)) => deliver(&state, &routed),
                Ok(Decoded::Lost(count)) | Err(RecvError::Lagged(count)) => lost(&state, count),
                Err(RecvError::Closed) => break,
            },
            position = positions.recv() => match position {
                Ok(ServerEvent::PositionChanged { node, fix, .. }) => {
                    follow(&state, &node, fix.as_ref());
                }
                Ok(_) => {}
                Err(RecvError::Lagged(count)) => {
                    tracing::warn!(count, "positions skipped by bearing routes; the next fix stands in");
                }
                Err(RecvError::Closed) => break,
            },
        }
    }
}

fn deliver(state: &AppState, routed: &Routed) {
    let DecoderEvent::Df(bearing) = &routed.record.event else {
        return;
    };
    let table = table(state);
    let now_s = df_fusion::now_s();
    for sink in &routed.record.sinks {
        if table.is_triangulation(sink) {
            df_fusion::submit(
                state,
                sink,
                routed.record.device_set,
                bearing.clone(),
                now_s,
                routed.record.at.clone(),
            );
        }
    }
}

fn lost(state: &AppState, count: u64) {
    if state.fusion.count_lost(count) > 0 {
        state.engine.emit_event(ServerEvent::Error {
            message: format!("bearings lost: {count}"),
        });
    }
}

fn follow(state: &AppState, position: &str, fix: Option<&PositionFix>) {
    let table = table(state);
    let now_s = df_fusion::now_s();
    for guided in table.guided_by(position) {
        match guided {
            Guided::Triangulation(node) => df_fusion::guide(state, node, fix.cloned(), now_s),
            Guided::Hunt(node) => {
                if let Some(fix) = fix {
                    pose_hunt(state, node, fix);
                }
            }
        }
    }
}

fn pose_hunt(state: &AppState, node: &str, fix: &PositionFix) {
    let received_ms = u64::try_from(jiff::Timestamp::now().as_millisecond()).unwrap_or(0);
    let snapshot = state.engine.snapshot();
    for set in &snapshot.device_sets {
        for hunt in set
            .hunts
            .iter()
            .filter(|hunt| hunt.settings.node.as_deref() == Some(node))
        {
            if let Err(error) =
                state
                    .engine
                    .hunt_pose(set.id, hunt.settings.channel, fix.clone(), received_ms)
            {
                tracing::debug!(%error, node, "hunt pose refused");
            }
        }
    }
}

#[cfg(test)]
mod tests;
