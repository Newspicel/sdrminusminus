use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_MISSIONS: usize = 64;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MissionsResponse {
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<MissionWorkspace>,
    pub workspaces: Vec<MissionWorkspace>,
    pub missions: Vec<Mission>,
    #[serde(default)]
    pub truncated: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct MissionWorkspace {
    pub id: i64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Mission {
    pub node: String,
    pub label: String,
    pub ready: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<MissionProblem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<MissionControl>,
    #[serde(flatten)]
    pub body: MissionBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum MissionBody {
    Hunt(HuntMission),
    Df(DfMission),
    Radar(RadarMission),
    Survey(SurveyMission),
    Triangulation(TriangulationMission),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissionControl {
    Tune,
    Calibrate,
    StartHunt,
    StopHunt,
    StartSweep,
    StopSweep,
    Mark,
    ClearFusion,
    StartSurvey,
    StopSurvey,
    ClearSurvey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum MissionProblem {
    Unwired { port: String },
    NotRunning,
    NoPosition,
    PhoneOffline { phone: String },
    PhoneNotPaired { phone: String },
    Scanning,
    OutOfBand,
    Refused { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelTarget {
    pub device_set: u32,
    pub channel: u32,
    pub channel_node: String,
    pub channel_type: String,
    pub frequency_hz: f64,
    pub bandwidth_hz: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PositionLink {
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntMission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ChannelTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<crate::hunt::HuntStatus>,
    pub clicks: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triangulations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfMission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_sets: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionLink>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triangulations: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadarMission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub array: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_sets: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmitter: Option<PositionLink>,
    pub surface: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SurveyMission {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_set: Option<u32>,
    pub stream: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_hz: Option<f64>,
    pub offset_hz: i64,
    pub bandwidth_hz: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionLink>,
    pub recording: bool,
    pub cells: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TriangulationMission {
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<crate::fusion::DfFusionState>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum MissionAction {
    Tune { frequency_hz: f64 },
    Calibrate,
    StartHunt,
    StopHunt,
    StartSweep,
    StopSweep,
    Mark,
    ClearFusion,
    StartSurvey,
    StopSurvey,
    ClearSurvey,
}

impl MissionAction {
    #[must_use]
    pub const fn control(&self) -> MissionControl {
        match self {
            Self::Tune { .. } => MissionControl::Tune,
            Self::Calibrate => MissionControl::Calibrate,
            Self::StartHunt => MissionControl::StartHunt,
            Self::StopHunt => MissionControl::StopHunt,
            Self::StartSweep => MissionControl::StartSweep,
            Self::StopSweep => MissionControl::StopSweep,
            Self::Mark => MissionControl::Mark,
            Self::ClearFusion => MissionControl::ClearFusion,
            Self::StartSurvey => MissionControl::StartSurvey,
            Self::StopSurvey => MissionControl::StopSurvey,
            Self::ClearSurvey => MissionControl::ClearSurvey,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MissionActionResponse {
    pub mission: Mission,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SwitchWorkspaceRequest {
    pub workspace: i64,
}

#[cfg(test)]
mod tests;
