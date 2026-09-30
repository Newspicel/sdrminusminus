use sdrmm_wire::{ArrayRecordingRequest, ArrayRecordingStarted, ArrayStatus, ArrayTuneRequest};

use super::*;
use crate::array::{self, ArrayRefusal};

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_arrays))
        .routes(routes!(calibrate_array))
        .routes(routes!(tune_array))
        .routes(routes!(start_array_recording, stop_array_recording))
}

impl From<ArrayRefusal> for AppError {
    fn from(refusal: ArrayRefusal) -> Self {
        let text = refusal.to_string();
        match refusal {
            ArrayRefusal::NoArray(_) | ArrayRefusal::NotRecording => Self::not_found(text),
            ArrayRefusal::Refused(_) => Self::new(StatusCode::CONFLICT, ErrorCode::Conflict, text),
        }
    }
}

#[utoipa::path(
    get, path = "/api/arrays",
    responses((status = 200, description = "Every Array node of the active workspace", body = Vec<ArrayStatus>)),
)]
pub(super) async fn list_arrays(
    State(state): State<AppState>,
) -> Result<Json<Vec<ArrayStatus>>, AppError> {
    Ok(Json(state.engine.array_statuses()))
}

#[utoipa::path(
    post, path = "/api/arrays/{node}/calibrate",
    params(("node" = String, Path, description = "Array node id")),
    responses(
        (status = 202, description = "Calibration started; the result arrives as `ArrayUpdate`"),
        (status = 404, description = "No Array node with that id", body = ApiError),
        (status = 409, description = "The Array cannot calibrate now", body = ApiError),
    ),
)]
pub(super) async fn calibrate_array(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || array::calibrate(&state, &node)).await??;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    patch, path = "/api/arrays/{node}/tune",
    params(("node" = String, Path, description = "Array node id")),
    request_body = ArrayTuneRequest,
    responses(
        (status = 204, description = "Every member lane follows"),
        (status = 400, description = "Gain out of range", body = ApiError),
        (status = 404, description = "No Array node with that id", body = ApiError),
        (status = 409, description = "A lane is held or cannot follow", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn tune_array(
    State(state): State<AppState>,
    Path(node): Path<String>,
    Json(request): Json<ArrayTuneRequest>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || array::tune(&state, &node, request)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/arrays/{node}/recording",
    params(("node" = String, Path, description = "Array node id")),
    request_body = ArrayRecordingRequest,
    responses(
        (status = 200, description = "Recording every lane", body = ArrayRecordingStarted),
        (status = 400, description = "Bad name or no recordings directory", body = ApiError),
        (status = 404, description = "No Array node with that id", body = ApiError),
        (status = 409, description = "The Array is not running or already records", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn start_array_recording(
    State(state): State<AppState>,
    Path(node): Path<String>,
    Json(request): Json<ArrayRecordingRequest>,
) -> Result<Json<ArrayRecordingStarted>, AppError> {
    let started =
        tokio::task::spawn_blocking(move || array::start_recording(&state, &node, request))
            .await??;
    Ok(Json(started))
}

#[utoipa::path(
    delete, path = "/api/arrays/{node}/recording",
    params(("node" = String, Path, description = "Array node id")),
    responses(
        (status = 204, description = "Recording stopped"),
        (status = 404, description = "No Array node with that id or no recording", body = ApiError),
    ),
)]
pub(super) async fn stop_array_recording(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || array::stop_recording(&state, &node)).await??;
    Ok(StatusCode::NO_CONTENT)
}
