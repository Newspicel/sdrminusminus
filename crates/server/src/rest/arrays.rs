use super::*;

/// Puts what the store holds in front of the driver, which is the only thing that reads it.
pub(crate) fn reload(state: &AppState) {
    match state.store.list_arrays() {
        Ok(definitions) => state.engine.arrays().replace(definitions),
        Err(error) => tracing::warn!(%error, "could not read the stored arrays"),
    }
}

#[utoipa::path(
    get, path = "/api/arrays",
    responses((status = 200, description = "Every array the operator has described", body = ArraysResponse)),
)]
pub(super) async fn list_arrays(
    State(state): State<AppState>,
) -> Result<Json<ArraysResponse>, AppError> {
    let store = state.store.clone();
    let arrays = tokio::task::spawn_blocking(move || store.list_arrays()).await??;
    Ok(Json(ArraysResponse { arrays }))
}

#[utoipa::path(
    put, path = "/api/arrays/{key}",
    params(("key" = String, Path, description = "The array's own key, which names its device id")),
    request_body = ArrayDefinition,
    responses(
        (status = 200, description = "The array is described and will be probed", body = ArrayDefinition),
        (status = 400, description = "Not a usable array", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn put_array(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(definition): Json<ArrayDefinition>,
) -> Result<Json<ArrayDefinition>, AppError> {
    let definition = ArrayDefinition { key, ..definition };
    if !definition.valid() {
        return Err(AppError::bad_request(
            "an array needs a plain key, two or more distinct member radios and a shared clock"
                .to_owned(),
        ));
    }
    let store = state.store.clone();
    let stored = definition.clone();
    tokio::task::spawn_blocking(move || store.put_array(&stored)).await??;
    reload(&state);
    state.engine.emit_scope(StateScope::Devices);
    Ok(Json(definition))
}

#[utoipa::path(
    delete, path = "/api/arrays/{key}",
    params(("key" = String, Path, description = "The array's own key")),
    responses(
        (status = 204, description = "The array is no longer described"),
        (status = 404, description = "No array of that name", body = ApiError),
    ),
)]
pub(super) async fn delete_array(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, AppError> {
    let store = state.store.clone();
    let removed = {
        let key = key.clone();
        tokio::task::spawn_blocking(move || store.delete_array(&key)).await??
    };
    if !removed {
        return Err(AppError::not_found(format!("no array named {key}")));
    }
    reload(&state);
    state.engine.emit_scope(StateScope::Devices);
    Ok(StatusCode::NO_CONTENT)
}
