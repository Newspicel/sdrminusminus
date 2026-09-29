use std::sync::LazyLock;

use sdrmm_wire::{
    ChannelInfo, DeviceSet, Mission, MissionBody, MissionControl, MissionProblem, MissionWorkspace,
    MissionsResponse, NodeBody, PatchCatalog, PatchGraph, PatchNode, StateSnapshot,
    mission::MAX_MISSIONS,
};
use sha2::{Digest, Sha256};

use crate::{
    AppState, StoreError,
    workspace::{self, DeviceBinding},
};

mod actions;
mod coherent;
mod graph;
mod hunt;
mod survey;
mod triangulation;
pub(crate) mod watch;

#[cfg(test)]
mod tests;

pub(crate) use actions::act;

static CATALOG: LazyLock<PatchCatalog> = LazyLock::new(PatchCatalog::build);

const TRIANGULATION: &str = "triangulation";

#[derive(Debug, thiserror::Error)]
pub(crate) enum MissionRefusal {
    #[error("No mission {0}")]
    NoMission(String),
    #[error("Not a control of this mission")]
    NotAControl,
    #[error("Frequency must be positive")]
    Frequency,
    #[error("Not running")]
    NotRunning,
}

struct Built {
    body: MissionBody,
    problems: Vec<MissionProblem>,
    controls: Vec<MissionControl>,
}

struct Scene<'a> {
    state: &'a AppState,
    graph: &'a PatchGraph,
    live: StateSnapshot,
    bindings: Vec<DeviceBinding>,
}

impl<'a> Scene<'a> {
    fn new(state: &'a AppState, graph: &'a PatchGraph) -> Self {
        let live = state.engine.snapshot();
        let bindings = workspace::bind(graph, &live);
        Self {
            state,
            graph,
            live,
            bindings,
        }
    }

    fn channel(&self, node: &str) -> Option<(&DeviceSet, &ChannelInfo)> {
        let (device_set, id) = self.bindings.iter().find_map(|binding| {
            binding
                .channels
                .iter()
                .find(|(bound, _)| bound == node)
                .map(|(_, id)| (binding.device_set, *id))
        })?;
        let set = self.set(device_set)?;
        let info = set.channels.iter().find(|channel| channel.id == id)?;
        Some((set, info))
    }

    fn set(&self, device_set: u32) -> Option<&DeviceSet> {
        self.live
            .device_sets
            .iter()
            .find(|set| set.id == device_set)
    }

    fn build(&self, node: &PatchNode) -> Option<Mission> {
        let built = match &node.body {
            NodeBody::Hunt(settings) => hunt::hunt(self, &node.id, settings),
            NodeBody::Df(_) => coherent::df(self, &node.id),
            NodeBody::PassiveRadar(_) => coherent::radar(self, &node.id),
            NodeBody::SignalMap(settings) => survey::survey(self, &node.id, settings),
            NodeBody::Triangulation(_) => triangulation::triangulation(self, &node.id),
            _ => return None,
        };
        Some(Mission {
            node: node.id.clone(),
            label: label(node),
            ready: built.problems.is_empty(),
            problems: built.problems,
            controls: built.controls,
            body: built.body,
        })
    }
}

const fn is_mission(body: &NodeBody) -> bool {
    matches!(
        body,
        NodeBody::Hunt(_)
            | NodeBody::Df(_)
            | NodeBody::PassiveRadar(_)
            | NodeBody::SignalMap(_)
            | NodeBody::Triangulation(_)
    )
}

fn label(node: &PatchNode) -> String {
    if let Some(label) = node
        .label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
    {
        return label.to_owned();
    }
    let kind = node.body.kind();
    CATALOG
        .nodes
        .iter()
        .find(|info| info.kind == kind)
        .map_or_else(|| kind.to_owned(), |info| info.name.clone())
}

pub(crate) fn missions(state: &AppState) -> Result<MissionsResponse, StoreError> {
    let active = state.store.active_workspace()?;
    let workspaces = state.store.list_workspaces()?;
    let mut response = MissionsResponse {
        revision: 0,
        workspace: active.as_ref().map(|detail| MissionWorkspace {
            id: detail.info.id,
            name: detail.info.name.clone(),
        }),
        workspaces: workspaces
            .workspaces
            .into_iter()
            .map(|info| MissionWorkspace {
                id: info.id,
                name: info.name,
            })
            .collect(),
        missions: Vec::new(),
        truncated: 0,
    };
    if let Some(active) = &active {
        let graph = &active.snapshot.graph;
        let candidates: Vec<&PatchNode> = graph
            .nodes
            .iter()
            .filter(|node| is_mission(&node.body))
            .collect();
        response.truncated =
            u32::try_from(candidates.len().saturating_sub(MAX_MISSIONS)).unwrap_or(u32::MAX);
        let scene = Scene::new(state, graph);
        response.missions = candidates
            .into_iter()
            .take(MAX_MISSIONS)
            .filter_map(|node| scene.build(node))
            .collect();
    }
    response.revision = revision(&response)?;
    Ok(response)
}

fn mission(state: &AppState, node: &str) -> Result<Option<(PatchGraph, Mission)>, StoreError> {
    let Some(active) = state.store.active_workspace()? else {
        return Ok(None);
    };
    let graph = active.snapshot.graph;
    let mission = graph
        .node(node)
        .and_then(|patch| Scene::new(state, &graph).build(patch));
    Ok(mission.map(|mission| (graph, mission)))
}

fn revision(response: &MissionsResponse) -> Result<u64, serde_json::Error> {
    let mut quiet = response.clone();
    quiet.revision = 0;
    for mission in &mut quiet.missions {
        match &mut mission.body {
            MissionBody::Hunt(hunt) => hunt.status = None,
            MissionBody::Triangulation(triangulation) => triangulation.state = None,
            MissionBody::Survey(survey) => survey.cells = 0,
            MissionBody::Df(_) | MissionBody::Radar(_) => {}
        }
    }
    let digest = Sha256::digest(serde_json::to_vec(&quiet)?);
    Ok(digest
        .iter()
        .take(8)
        .fold(0, |revision, byte| (revision << 8) | u64::from(*byte)))
}
