use sdrmm_wire::{
    ARRAY_PORT, DfMission, MissionBody, MissionControl, MissionProblem, PositionLink,
    RADAR_TX_PORT, RadarMission,
};

use super::{Built, Scene, TRIANGULATION, graph};
use crate::array;

#[derive(Default)]
struct Wired {
    array: Option<String>,
    device_sets: Vec<u32>,
    center_hz: Option<f64>,
    position: Option<PositionLink>,
    problems: Vec<MissionProblem>,
    controls: Vec<MissionControl>,
}

pub(super) fn df(scene: &Scene<'_>, node: &str) -> Built {
    let wired = wired(scene, node);
    Built {
        body: MissionBody::Df(DfMission {
            array: wired.array,
            device_sets: wired.device_sets,
            center_hz: wired.center_hz,
            position: wired.position,
            triangulations: graph::event_targets(scene.graph, node, TRIANGULATION),
        }),
        problems: wired.problems,
        controls: wired.controls,
    }
}

pub(super) fn radar(scene: &Scene<'_>, node: &str) -> Built {
    let mut wired = wired(scene, node);
    let (transmitter, phone) = graph::link(scene.graph, &scene.state.phones, node, RADAR_TX_PORT);
    wired.problems.extend(phone);
    Built {
        body: MissionBody::Radar(RadarMission {
            array: wired.array,
            device_sets: wired.device_sets,
            center_hz: wired.center_hz,
            position: wired.position,
            transmitter,
            surface: true,
        }),
        problems: wired.problems,
        controls: wired.controls,
    }
}

fn wired(scene: &Scene<'_>, node: &str) -> Wired {
    let unwired = || Wired {
        problems: vec![MissionProblem::Unwired {
            port: ARRAY_PORT.to_owned(),
        }],
        ..Wired::default()
    };
    let Some(array) = scene.graph.array_of_processor(node) else {
        return unwired();
    };
    let Some(summary) = array::summary(scene.state, scene.graph, array) else {
        return unwired();
    };
    let (position, phone) = graph::position_link(scene.graph, &scene.state.phones, array);
    let mut problems = Vec::new();
    if let Some(reason) = summary.problem {
        problems.push(MissionProblem::Refused { reason });
    } else if summary.center_hz.is_none() {
        problems.push(MissionProblem::NotRunning);
    }
    if let Some(reason) = processor_error(scene, array, node) {
        problems.push(MissionProblem::Refused { reason });
    }
    problems.extend(phone);
    let mut controls = Vec::new();
    if summary.center_hz.is_some() {
        controls.push(MissionControl::Tune);
    }
    if summary.can_calibrate {
        controls.push(MissionControl::Calibrate);
    }
    Wired {
        array: Some(array.to_owned()),
        device_sets: summary.device_sets,
        center_hz: summary.center_hz,
        position,
        problems,
        controls,
    }
}

fn processor_error(scene: &Scene<'_>, array: &str, node: &str) -> Option<String> {
    scene
        .live
        .arrays
        .iter()
        .find(|status| status.node == array)?
        .processors
        .iter()
        .find(|processor| processor.node == node)?
        .error
        .clone()
}
