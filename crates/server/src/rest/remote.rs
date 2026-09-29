use axum::{Extension, extract::State, http::StatusCode};
use sdrmm_tunnel::Relayed;
use sdrmm_wire::{ApiError, RemoteStatus};

use super::{AppError, Json, LocalOnly};
use crate::{AppState, remote::RemoteError};

impl From<RemoteError> for AppError {
    fn from(error: RemoteError) -> Self {
        match error {
            RemoteError::AlreadyPaired => AppError::conflict(error.to_string()),
            RemoteError::Pairing(_) => AppError::bad_gateway(error.to_string()),
            RemoteError::Key(_) => AppError::internal(error.to_string()),
            RemoteError::Store(store) => store.into(),
        }
    }
}

#[utoipa::path(
    get, path = "/api/remote",
    responses((status = 200, description = "Remote access through the app", body = RemoteStatus)),
)]
pub(super) async fn get_remote(
    State(state): State<AppState>,
    relayed: Option<Extension<Relayed>>,
) -> Json<RemoteStatus> {
    Json(state.remote.status(relayed.is_some()))
}

#[utoipa::path(
    post, path = "/api/remote/pair",
    responses(
        (status = 200, description = "A code to approve in the app", body = RemoteStatus),
        (status = 403, description = "Asked through remote access", body = ApiError),
        (status = 409, description = "Already paired", body = ApiError),
        (status = 502, description = "The app refused or could not be reached", body = ApiError),
    ),
)]
pub(super) async fn pair_remote(
    State(state): State<AppState>,
    _local: LocalOnly,
) -> Result<Json<RemoteStatus>, AppError> {
    Ok(Json(state.remote.pair().await?))
}

#[utoipa::path(
    delete, path = "/api/remote",
    responses(
        (status = 204, description = "Remote access is off and the pairing forgotten"),
        (status = 403, description = "Asked through remote access", body = ApiError),
    ),
)]
pub(super) async fn unpair_remote(
    State(state): State<AppState>,
    _local: LocalOnly,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || state.remote.unpair()).await??;
    Ok(StatusCode::NO_CONTENT)
}
