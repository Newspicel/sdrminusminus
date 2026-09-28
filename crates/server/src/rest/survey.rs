use sdrmm_wire::{SurveyGrid, SurveyRequest};

use super::*;
use crate::survey::SurveyRefusal;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_survey, control_survey))
}

impl From<SurveyRefusal> for AppError {
    fn from(refusal: SurveyRefusal) -> Self {
        match refusal {
            SurveyRefusal::Missing(node) => Self::not_found(format!("no survey {node}")),
            SurveyRefusal::NoRadio => Self::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Wire a radio".to_owned(),
            ),
            SurveyRefusal::NoPosition => Self::new(
                StatusCode::CONFLICT,
                ErrorCode::Conflict,
                "Wire a position".to_owned(),
            ),
        }
    }
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
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<Json<SurveyGrid>, AppError> {
    state
        .survey
        .grid(&node)
        .map(Json)
        .ok_or_else(|| SurveyRefusal::Missing(node).into())
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
    State(state): State<AppState>,
    Path(node): Path<String>,
    Json(request): Json<SurveyRequest>,
) -> Result<Json<SurveyGrid>, AppError> {
    state.survey.act(&state, &node, request.action).map(Json)
}
