use super::*;
use crate::df_fusion::NoTriangulation;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_fusion, reset_fusion))
}

impl From<NoTriangulation> for AppError {
    fn from(missing: NoTriangulation) -> Self {
        Self::not_found(format!("no triangulation {}", missing.0))
    }
}

#[utoipa::path(
    get, path = "/api/fusion/{node}",
    params(("node" = String, Path, description = "Triangulation node id")),
    responses(
        (status = 200, description = "Where the bearings so far say the transmitter is", body = DfFusionState),
        (status = 404, description = "No triangulation with that id", body = ApiError),
    ),
)]
pub(super) async fn get_fusion(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<Json<DfFusionState>, AppError> {
    state
        .fusion
        .state(&node)
        .map(Json)
        .ok_or_else(|| NoTriangulation(node).into())
}

#[utoipa::path(
    delete, path = "/api/fusion/{node}",
    params(("node" = String, Path, description = "Triangulation node id")),
    responses(
        (status = 204, description = "The grid is empty again"),
        (status = 404, description = "No triangulation with that id", body = ApiError),
    ),
)]
pub(super) async fn reset_fusion(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<StatusCode, AppError> {
    crate::df_fusion::clear(&state, &node)?;
    Ok(StatusCode::NO_CONTENT)
}
