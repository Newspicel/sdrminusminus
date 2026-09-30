use sdrmm_wire::{MissionBody, MissionControl, TriangulationMission};

use super::{Built, Scene, graph};

pub(super) fn triangulation(scene: &Scene<'_>, node: &str) -> Built {
    let (position, _) = graph::position_link(scene.graph, &scene.state.phones, node);
    Built {
        body: MissionBody::Triangulation(TriangulationMission {
            sources: graph::event_sources(scene.graph, node),
            position,
            state: scene.state.fusion.state(node),
        }),
        problems: Vec::new(),
        controls: vec![MissionControl::ClearFusion],
    }
}
