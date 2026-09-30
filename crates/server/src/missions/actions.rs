use sdrmm_wire::{
    ArrayTuneRequest, HuntMission, Mission, MissionAction, MissionBody, PatchGraph, SurveyAction,
};

use super::{MissionRefusal, hunt, mission};
use crate::{AppState, array, df_fusion, placement, rest::AppError, workspace};

pub(crate) fn act(
    state: &AppState,
    node: &str,
    action: MissionAction,
) -> Result<Mission, AppError> {
    let (graph, mission) = current(state, node)?;
    if !mission.controls.contains(&action.control()) {
        return Err(MissionRefusal::NotAControl.into());
    }
    dispatch(state, &graph, &mission, action)?;
    Ok(current(state, node)?.1)
}

fn current(state: &AppState, node: &str) -> Result<(PatchGraph, Mission), AppError> {
    Ok(mission(state, node)?.ok_or_else(|| MissionRefusal::NoMission(node.to_owned()))?)
}

fn dispatch(
    state: &AppState,
    graph: &PatchGraph,
    mission: &Mission,
    action: MissionAction,
) -> Result<(), AppError> {
    match &mission.body {
        MissionBody::Hunt(hunt) => hunt_action(state, graph, &mission.node, hunt, action),
        MissionBody::Df(df) => array_action(state, df.array.as_deref(), action),
        MissionBody::Radar(radar) => array_action(state, radar.array.as_deref(), action),
        MissionBody::Survey(_) => survey_action(state, &mission.node, action),
        MissionBody::Triangulation(_) => match action {
            MissionAction::ClearFusion => df_fusion::clear(state, &mission.node),
            _ => Err(MissionRefusal::NotAControl.into()),
        },
    }
}

fn hunt_action(
    state: &AppState,
    graph: &PatchGraph,
    node: &str,
    hunt: &HuntMission,
    action: MissionAction,
) -> Result<(), AppError> {
    let target = hunt.target.as_ref().ok_or(MissionRefusal::NotRunning)?;
    let (ds, ch) = (target.device_set, target.channel);
    let engine = &state.engine;
    match action {
        MissionAction::Tune { frequency_hz } => return tune_channel(state, ds, ch, frequency_hz),
        MissionAction::StartHunt | MissionAction::StopSweep => {
            engine.start_hunt(ds, hunt::settings(graph, node, ch))?;
        }
        MissionAction::StopHunt => {
            engine.stop_hunt(ds, ch)?;
        }
        MissionAction::StartSweep => {
            engine.sweep_hunt(ds, ch, Some(hunt::settings(graph, node, ch)))?;
        }
        MissionAction::Mark => {
            engine.hunt_mark(ds, ch)?;
        }
        _ => return Err(MissionRefusal::NotAControl.into()),
    }
    Ok(())
}

fn tune_channel(state: &AppState, ds: u32, ch: u32, frequency_hz: f64) -> Result<(), AppError> {
    let frequency_hz = positive(frequency_hz)?;
    let _serialized = crate::rest::lock_gate(&state.apply_gate);
    let edit = workspace::begin_edit(state, ds, Some(ch));
    state.engine.tune_channel(ds, ch, frequency_hz)?;
    if let Some(edit) = edit {
        workspace::finish_edit(state, edit);
    }
    placement::settle_active(state);
    Ok(())
}

fn array_action(
    state: &AppState,
    array: Option<&str>,
    action: MissionAction,
) -> Result<(), AppError> {
    let array = array.ok_or(MissionRefusal::NotRunning)?;
    match action {
        MissionAction::Tune { frequency_hz } => array::tune(
            state,
            array,
            ArrayTuneRequest {
                center_hz: Some(positive(frequency_hz)?),
                gain: None,
            },
        ),
        MissionAction::Calibrate => array::calibrate(state, array),
        _ => Err(MissionRefusal::NotAControl.into()),
    }
}

fn survey_action(state: &AppState, node: &str, action: MissionAction) -> Result<(), AppError> {
    let survey = match action {
        MissionAction::StartSurvey => SurveyAction::Start,
        MissionAction::StopSurvey => SurveyAction::Stop,
        MissionAction::ClearSurvey => SurveyAction::Clear,
        _ => return Err(MissionRefusal::NotAControl.into()),
    };
    state.survey.act(state, node, survey)?;
    Ok(())
}

fn positive(frequency_hz: f64) -> Result<f64, MissionRefusal> {
    if frequency_hz.is_finite() && frequency_hz > 0.0 {
        Ok(frequency_hz)
    } else {
        Err(MissionRefusal::Frequency)
    }
}
