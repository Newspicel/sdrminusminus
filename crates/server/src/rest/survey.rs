use sdrmm_wire::{SurveyGrid, SurveyRequest};

use super::*;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_survey, control_survey))
}

#[utoipa::path(
    get, path = "/api/survey/{node}",
    params(("node" = String, Path, description = "Signal survey node id")),
    responses(
        (status = 200, description = "Every surveyed cell", body = SurveyGrid),
        (status = 404, description = "No survey with that id", body = ApiError),
    ),
)]
pub(super) async fn get_survey(
    State(_state): State<AppState>,
    Path(_node): Path<String>,
) -> Result<Json<SurveyGrid>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    post, path = "/api/survey/{node}",
    params(("node" = String, Path, description = "Signal survey node id")),
    request_body = SurveyRequest,
    responses(
        (status = 200, description = "The grid after the action", body = SurveyGrid),
        (status = 404, description = "No survey with that id", body = ApiError),
        (status = 409, description = "The survey cannot do that now", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn control_survey(
    State(_state): State<AppState>,
    Path(_node): Path<String>,
    Json(_request): Json<SurveyRequest>,
) -> Result<Json<SurveyGrid>, AppError> {
    Err(AppError::not_built())
}
