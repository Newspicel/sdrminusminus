use sdrmm_wire::RadarUpdate;

use super::*;
use crate::radar::{self, RadarRefusal};

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_radar))
        .routes(routes!(clear_radar_tracks))
}

impl From<RadarRefusal> for AppError {
    fn from(refusal: RadarRefusal) -> Self {
        let text = refusal.to_string();
        match refusal {
            RadarRefusal::NoRadar(_) => Self::not_found(text),
            RadarRefusal::Refused(_) => Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, text),
        }
    }
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
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<Json<RadarUpdate>, AppError> {
    match state.radar.latest(&node) {
        Some(update) => Ok(Json(update)),
        None => Err(RadarRefusal::NoRadar(node).into()),
    }
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
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || radar::clear(&state, &node)).await??;
    Ok(StatusCode::NO_CONTENT)
}
