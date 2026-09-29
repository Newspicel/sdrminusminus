use std::collections::HashMap;

use sdrmm_wire::{
    MissionBody, MissionControl, MissionProblem, SignalMapNode, StateSnapshot, SurveyMission,
    port_stream,
};

use super::{Built, Scene, graph};
use crate::survey::{IQ_PORT, Radio, radio_of};

pub(super) fn survey(scene: &Scene<'_>, node: &str, settings: &SignalMapNode) -> Built {
    let iq = graph::source_of(scene.graph, node, IQ_PORT);
    let devices: HashMap<String, u32> = scene
        .bindings
        .iter()
        .map(|binding| (binding.node.clone(), binding.device_set))
        .collect();
    let radio = iq.and_then(|from| radio_of(&devices, &scene.live, &from.node, &from.port));
    let (position, phone) = graph::position_link(scene.graph, &scene.state.phones, node);
    let mut problems = Vec::new();
    if iq.is_none() {
        problems.push(MissionProblem::Unwired {
            port: IQ_PORT.to_owned(),
        });
    } else if radio.is_none() {
        problems.push(MissionProblem::NotRunning);
    }
    if position.is_none() {
        problems.push(MissionProblem::NoPosition);
    }
    problems.extend(phone);
    let recording = scene.state.survey.recording(node);
    let cells = scene.state.survey.cells(node);
    let mut controls = Vec::new();
    if recording {
        controls.push(MissionControl::StopSurvey);
    } else if radio.is_some() && position.is_some() {
        controls.push(MissionControl::StartSurvey);
    }
    if cells > 0 {
        controls.push(MissionControl::ClearSurvey);
    }
    let stream = radio
        .map(|radio| radio.stream)
        .or_else(|| iq.and_then(|from| port_stream(IQ_PORT, &from.port)))
        .unwrap_or_default();
    Built {
        body: MissionBody::Survey(SurveyMission {
            device_set: radio.map(|radio| radio.device_set),
            stream,
            frequency_hz: radio
                .and_then(|radio| center_of(&scene.live, radio))
                .map(|center| center + settings.offset_hz as f64),
            offset_hz: settings.offset_hz,
            bandwidth_hz: settings.bandwidth_hz,
            position,
            recording,
            cells,
        }),
        problems,
        controls,
    }
}

fn center_of(live: &StateSnapshot, radio: Radio) -> Option<f64> {
    let set = live
        .device_sets
        .iter()
        .find(|set| set.id == radio.device_set)?;
    set.virtual_lanes
        .iter()
        .find(|lane| lane.stream == radio.stream)
        .map(|lane| lane.center_hz)
        .or_else(|| {
            set.settings
                .for_stream(radio.stream, &set.capabilities.per_stream)
                .center_hz
        })
}
