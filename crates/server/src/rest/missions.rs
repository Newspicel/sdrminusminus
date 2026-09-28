use sdrmm_wire::{MissionAction, MissionActionResponse, MissionsResponse, SwitchWorkspaceRequest};

use super::*;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_missions))
        .routes(routes!(run_mission_action))
        .routes(routes!(switch_mission_workspace))
}

#[utoipa::path(
    get, path = "/api/missions",
    responses((status = 200, description = "What a phone can do in the active workspace", body = MissionsResponse)),
)]
pub(super) async fn list_missions(
    State(_state): State<AppState>,
) -> Result<Json<MissionsResponse>, AppError> {
    Err(AppError::not_built())
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
    State(_state): State<AppState>,
    Path(_node): Path<String>,
    Json(_action): Json<MissionAction>,
) -> Result<Json<MissionActionResponse>, AppError> {
    Err(AppError::not_built())
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
    State(_state): State<AppState>,
    Json(_request): Json<SwitchWorkspaceRequest>,
) -> Result<Json<MissionsResponse>, AppError> {
    Err(AppError::not_built())
}
