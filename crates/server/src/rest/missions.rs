use sdrmm_wire::{MissionAction, MissionActionResponse, MissionsResponse, SwitchWorkspaceRequest};

use super::*;
use crate::missions::{MissionRefusal, act, missions};

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_missions))
        .routes(routes!(run_mission_action))
        .routes(routes!(switch_mission_workspace))
}

impl From<MissionRefusal> for AppError {
    fn from(refusal: MissionRefusal) -> Self {
        let text = refusal.to_string();
        match refusal {
            MissionRefusal::NoMission(_) => Self::not_found(text),
            MissionRefusal::NotAControl | MissionRefusal::Frequency => Self::bad_request(text),
            MissionRefusal::NotRunning => {
                Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, text)
            }
        }
    }
}

async fn listing(state: AppState) -> Result<MissionsResponse, AppError> {
    Ok(tokio::task::spawn_blocking(move || missions(&state)).await??)
}

#[utoipa::path(
    get, path = "/api/missions",
    responses((status = 200, description = "What a phone can do in the active workspace", body = MissionsResponse)),
)]
pub(super) async fn list_missions(
    State(state): State<AppState>,
) -> Result<Json<MissionsResponse>, AppError> {
    Ok(Json(listing(state).await?))
}

#[utoipa::path(
    post, path = "/api/missions/{node}/actions",
    params(("node" = String, Path, description = "Mission node id")),
    request_body = MissionAction,
    responses(
        (status = 200, description = "The action was taken", body = MissionActionResponse),
        (status = 400, description = "Not a control of this mission", body = ApiError),
        (status = 404, description = "No mission with that id", body = ApiError),
        (status = 409, description = "The mission cannot do that now", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
        (status = 503, description = "The part that runs it is not up", body = ApiError),
    ),
)]
pub(super) async fn run_mission_action(
    State(state): State<AppState>,
    Path(node): Path<String>,
    Json(action): Json<MissionAction>,
) -> Result<Json<MissionActionResponse>, AppError> {
    let mission = tokio::task::spawn_blocking(move || act(&state, &node, action)).await??;
    Ok(Json(MissionActionResponse { mission }))
}

#[utoipa::path(
    post, path = "/api/missions/workspace",
    request_body = SwitchWorkspaceRequest,
    responses(
        (status = 200, description = "Missions of the workspace now active", body = MissionsResponse),
        (status = 404, description = "No workspace with that id", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn switch_mission_workspace(
    State(state): State<AppState>,
    Json(request): Json<SwitchWorkspaceRequest>,
) -> Result<Json<MissionsResponse>, AppError> {
    let switched = state.clone();
    tokio::task::spawn_blocking(move || switch(&switched, request.workspace)).await??;
    reconcile_graph(state.clone()).await?;
    Ok(Json(listing(state).await?))
}

fn switch(state: &AppState, workspace: i64) -> Result<(), AppError> {
    let _serialized = lock_gate(&state.apply_gate);
    activate(state, workspace)?;
    let report = bring_up_active(state, workspace)?;
    for refusal in &report.refused {
        tracing::warn!(
            workspace,
            node = refusal.node,
            reason = refusal.reason,
            "a node could not be brought up by a phone's workspace switch"
        );
    }
    Ok(())
}
