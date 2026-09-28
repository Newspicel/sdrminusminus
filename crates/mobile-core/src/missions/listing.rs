use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    fusion::DfFusionState,
    hunt::HuntStatus,
    mission::{self as wire, MissionAction, MissionBody, MissionProblem, MissionsResponse},
};

use super::views::{
    Mission, MissionCommand, MissionControl, MissionKind, MissionsView, TargetMode, WorkspaceRef,
};
use crate::error::CoreError;

pub(crate) const NOT_A_CONTROL: &str = "Not a control of this mission";

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Target {
    Hunt {
        device_set: Option<u32>,
        channel: Option<u32>,
        freq_hz: f64,
        status: Option<HuntStatus>,
        running: bool,
    },
    Df {
        array: Option<String>,
        fusion: Option<String>,
        clear_on: Option<String>,
        freq_hz: f64,
        state: Option<DfFusionState>,
    },
    Fusion {
        state: Option<DfFusionState>,
    },
    Radar {
        surface: bool,
    },
    Survey {
        freq_hz: f64,
        recording: bool,
        cells: u32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub(crate) mission: Mission,
    pub(crate) target: Target,
}

impl Entry {
    pub(crate) fn id(&self) -> &str {
        &self.mission.id
    }

    pub(crate) fn fusion_node(&self) -> Option<&str> {
        match &self.target {
            Target::Df { fusion, .. } => fusion.as_deref(),
            Target::Fusion { .. } => Some(self.id()),
            Target::Hunt { .. } | Target::Radar { .. } | Target::Survey { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Listing {
    pub(crate) response: Option<MissionsResponse>,
    pub(crate) entries: Vec<Entry>,
}

impl Listing {
    pub(crate) fn new(response: MissionsResponse) -> Self {
        Self {
            entries: project(&response),
            response: Some(response),
        }
    }

    pub(crate) fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    pub(crate) fn replace(&mut self, mission: wire::Mission) {
        let Some(mut response) = self.response.take() else {
            return;
        };
        if let Some(slot) = response
            .missions
            .iter_mut()
            .find(|listed| listed.node == mission.node)
        {
            *slot = mission;
        }
        *self = Self::new(response);
    }

    pub(crate) fn view(&self) -> MissionsView {
        let reference = |workspace: &wire::MissionWorkspace| WorkspaceRef {
            id: workspace.id.to_string(),
            name: workspace.name.clone(),
        };
        let response = self.response.as_ref();
        MissionsView {
            workspace: response
                .and_then(|response| response.workspace.as_ref())
                .map_or_else(
                    || WorkspaceRef {
                        id: String::new(),
                        name: String::new(),
                    },
                    reference,
                ),
            workspaces: response
                .map(|response| response.workspaces.iter().map(reference).collect())
                .unwrap_or_default(),
            missions: self
                .entries
                .iter()
                .map(|entry| entry.mission.clone())
                .collect(),
        }
    }

    pub(crate) fn wants_phone(&self, phone: &str) -> bool {
        let names = |link: Option<&wire::PositionLink>| {
            link.and_then(|link| link.phone.as_deref()) == Some(phone)
        };
        self.response
            .iter()
            .flat_map(|response| &response.missions)
            .any(|mission| match &mission.body {
                MissionBody::Hunt(hunt) => names(hunt.position.as_ref()),
                MissionBody::Df(df) => names(df.position.as_ref()),
                MissionBody::Radar(radar) => {
                    names(radar.position.as_ref()) || names(radar.transmitter.as_ref())
                }
                MissionBody::Survey(survey) => names(survey.position.as_ref()),
                MissionBody::Triangulation(triangulation) => names(triangulation.position.as_ref()),
            })
    }
}

pub(crate) fn mhz(hz: Option<f64>) -> String {
    hz.filter(|hz| hz.is_finite() && *hz > 0.0)
        .map_or_else(String::new, |hz| format!("{:.3} MHz", hz / 1e6))
}

pub(crate) fn problem_label(problem: &MissionProblem) -> String {
    match problem {
        MissionProblem::Unwired { port } => format!("Wire {port}"),
        MissionProblem::NotRunning => "Not running".to_owned(),
        MissionProblem::NoPosition => "No position".to_owned(),
        MissionProblem::PhoneOffline { .. } => "Phone offline".to_owned(),
        MissionProblem::PhoneNotPaired { .. } => "Phone not paired".to_owned(),
        MissionProblem::Scanning => "Scanning".to_owned(),
        MissionProblem::OutOfBand => "Out of band".to_owned(),
        MissionProblem::Refused { reason } => reason.clone(),
    }
}

const fn control(control: wire::MissionControl) -> MissionControl {
    match control {
        wire::MissionControl::Tune => MissionControl::Tune,
        wire::MissionControl::Calibrate => MissionControl::Calibrate,
        wire::MissionControl::StartHunt | wire::MissionControl::StopHunt => MissionControl::HuntRun,
        wire::MissionControl::StartSweep | wire::MissionControl::StopSweep => MissionControl::Sweep,
        wire::MissionControl::Mark => MissionControl::Mark,
        wire::MissionControl::ClearFusion => MissionControl::ClearFusion,
        wire::MissionControl::StartSurvey | wire::MissionControl::StopSurvey => {
            MissionControl::SurveyRun
        }
        wire::MissionControl::ClearSurvey => MissionControl::SurveyClear,
    }
}

fn controls(listed: &[wire::MissionControl], extra: &[MissionControl]) -> Vec<MissionControl> {
    let mut out = Vec::new();
    for mapped in listed
        .iter()
        .map(|listed| control(*listed))
        .chain(extra.iter().copied())
    {
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

fn mission(
    listed: &wire::Mission,
    kind: MissionKind,
    detail: String,
    controls: Vec<MissionControl>,
) -> Mission {
    Mission {
        id: listed.node.clone(),
        kind,
        title: listed.label.clone(),
        detail,
        ready: listed.ready,
        blocker: listed.problems.first().map(problem_label),
        controls,
    }
}

fn project(response: &MissionsResponse) -> Vec<Entry> {
    let triangulations: HashMap<&str, &wire::Mission> = response
        .missions
        .iter()
        .filter(|listed| matches!(listed.body, MissionBody::Triangulation(_)))
        .map(|listed| (listed.node.as_str(), listed))
        .collect();
    let reached: HashSet<&str> = response
        .missions
        .iter()
        .filter_map(|listed| match &listed.body {
            MissionBody::Df(df) => Some(df.triangulations.iter().map(String::as_str)),
            _ => None,
        })
        .flatten()
        .collect();
    response
        .missions
        .iter()
        .filter_map(|listed| entry(listed, &triangulations, &reached))
        .collect()
}

fn entry(
    listed: &wire::Mission,
    triangulations: &HashMap<&str, &wire::Mission>,
    reached: &HashSet<&str>,
) -> Option<Entry> {
    let own = |extra: &[MissionControl]| controls(&listed.controls, extra);
    Some(match &listed.body {
        MissionBody::Hunt(hunt) => {
            let freq_hz = hunt.target.as_ref().map(|target| target.frequency_hz);
            Entry {
                mission: mission(listed, MissionKind::Hunt, mhz(freq_hz), own(&[])),
                target: Target::Hunt {
                    device_set: hunt.target.as_ref().map(|target| target.device_set),
                    channel: hunt.target.as_ref().map(|target| target.channel),
                    freq_hz: freq_hz.unwrap_or_default(),
                    status: hunt.status.clone(),
                    running: listed.controls.contains(&wire::MissionControl::StopHunt),
                },
            }
        }
        MissionBody::Df(df) => {
            let first = df.triangulations.first();
            let fused = first.and_then(|node| triangulations.get(node.as_str()));
            let clear_on = fused
                .filter(|fused| fused.controls.contains(&wire::MissionControl::ClearFusion))
                .map(|fused| fused.node.clone());
            let mut extra = Vec::new();
            if clear_on.is_some() {
                extra.push(MissionControl::ClearFusion);
            }
            if first.is_some() {
                extra.push(MissionControl::TargetMode);
            }
            let state = fused.and_then(|fused| match &fused.body {
                MissionBody::Triangulation(triangulation) => triangulation.state.clone(),
                _ => None,
            });
            Entry {
                mission: mission(listed, MissionKind::DfDrive, mhz(df.center_hz), own(&extra)),
                target: Target::Df {
                    array: df.array.clone(),
                    fusion: first.cloned(),
                    clear_on,
                    freq_hz: df.center_hz.unwrap_or_default(),
                    state,
                },
            }
        }
        MissionBody::Radar(radar) => Entry {
            mission: mission(
                listed,
                MissionKind::RadarWatch,
                mhz(radar.center_hz),
                own(&[]),
            ),
            target: Target::Radar {
                surface: radar.surface,
            },
        },
        MissionBody::Survey(survey) => Entry {
            mission: mission(
                listed,
                MissionKind::Survey,
                mhz(survey.frequency_hz),
                own(&[]),
            ),
            target: Target::Survey {
                freq_hz: survey.frequency_hz.unwrap_or_default(),
                recording: survey.recording,
                cells: survey.cells,
            },
        },
        MissionBody::Triangulation(triangulation) => {
            if reached.contains(listed.node.as_str()) {
                return None;
            }
            let detail = match triangulation.sources.len() {
                1 => "1 source".to_owned(),
                count => format!("{count} sources"),
            };
            Entry {
                mission: mission(
                    listed,
                    MissionKind::DfDrive,
                    detail,
                    own(&[MissionControl::TargetMode]),
                ),
                target: Target::Fusion {
                    state: triangulation.state.clone(),
                },
            }
        }
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Route {
    Server { node: String, action: MissionAction },
    TargetMode(TargetMode),
}

fn refused(message: &str) -> CoreError {
    CoreError::Refused {
        message: message.to_owned(),
    }
}

pub(crate) fn route(entry: &Entry, command: MissionCommand) -> Result<Route, CoreError> {
    let server = |action: MissionAction| Route::Server {
        node: entry.id().to_owned(),
        action,
    };
    let (needs, route) = match command {
        MissionCommand::StartHunt => (MissionControl::HuntRun, server(MissionAction::StartHunt)),
        MissionCommand::StopHunt => (MissionControl::HuntRun, server(MissionAction::StopHunt)),
        MissionCommand::Sweep { on: true } => {
            (MissionControl::Sweep, server(MissionAction::StartSweep))
        }
        MissionCommand::Sweep { on: false } => {
            (MissionControl::Sweep, server(MissionAction::StopSweep))
        }
        MissionCommand::Mark => (MissionControl::Mark, server(MissionAction::Mark)),
        MissionCommand::Tune { hz } => {
            if !hz.is_finite() || hz <= 0.0 {
                return Err(refused("Bad frequency"));
            }
            (
                MissionControl::Tune,
                server(MissionAction::Tune { frequency_hz: hz }),
            )
        }
        MissionCommand::Calibrate => (MissionControl::Calibrate, server(MissionAction::Calibrate)),
        MissionCommand::ClearFusion => {
            let node = match &entry.target {
                Target::Df { clear_on, .. } => clear_on.clone(),
                Target::Fusion { .. } => Some(entry.id().to_owned()),
                Target::Hunt { .. } | Target::Radar { .. } | Target::Survey { .. } => None,
            };
            let route = node.map_or_else(
                || server(MissionAction::ClearFusion),
                |node| Route::Server {
                    node,
                    action: MissionAction::ClearFusion,
                },
            );
            (MissionControl::ClearFusion, route)
        }
        MissionCommand::SetTargetMode { mode } => {
            (MissionControl::TargetMode, Route::TargetMode(mode))
        }
        MissionCommand::StartSurvey => (
            MissionControl::SurveyRun,
            server(MissionAction::StartSurvey),
        ),
        MissionCommand::StopSurvey => {
            (MissionControl::SurveyRun, server(MissionAction::StopSurvey))
        }
        MissionCommand::ClearSurvey => (
            MissionControl::SurveyClear,
            server(MissionAction::ClearSurvey),
        ),
    };
    if entry.mission.controls.contains(&needs) {
        Ok(route)
    } else {
        Err(refused(NOT_A_CONTROL))
    }
}

#[cfg(test)]
pub(crate) mod tests;
