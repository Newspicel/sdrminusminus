use sdrmm_wire::RadarUpdate;

use super::*;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_radar))
        .routes(routes!(clear_radar_tracks))
}

#[utoipa::path(
    get, path = "/api/radar/{node}",
    params(("node" = String, Path, description = "Passive radar node id")),
    responses(
        (status = 200, description = "The latest radar picture", body = RadarUpdate),
        (status = 404, description = "No radar with that id", body = ApiError),
    ),
)]
pub(super) async fn get_radar(
    State(_state): State<AppState>,
    Path(_node): Path<String>,
) -> Result<Json<RadarUpdate>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    delete, path = "/api/radar/{node}/tracks",
    params(("node" = String, Path, description = "Passive radar node id")),
    responses(
        (status = 204, description = "Every track dropped"),
        (status = 404, description = "No radar with that id", body = ApiError),
        (status = 409, description = "The radar cannot drop its tracks now", body = ApiError),
    ),
)]
pub(super) async fn clear_radar_tracks(
    State(_state): State<AppState>,
    Path(_node): Path<String>,
) -> Result<StatusCode, AppError> {
    Err(AppError::not_built())
}
