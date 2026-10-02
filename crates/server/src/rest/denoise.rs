use sdrmm_wire::{DenoiseModel, DenoiseModelsResponse};

use super::*;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_denoise_models))
        .routes(routes!(download_denoise_model, delete_denoise_model))
}

fn parse(name: &str) -> Result<DenoiseModel, AppError> {
    DenoiseModel::from_name(name).ok_or_else(|| AppError::not_found(format!("no model {name}")))
}

#[utoipa::path(
    get, path = "/api/denoise-models",
    responses((status = 200, description = "Every DPDFNet model and whether it is here", body = DenoiseModelsResponse)),
)]
pub(super) async fn list_denoise_models(
    State(state): State<AppState>,
) -> Json<DenoiseModelsResponse> {
    Json(DenoiseModelsResponse {
        models: state.denoise.statuses(state.engine.denoise_models()),
    })
}

#[utoipa::path(
    post, path = "/api/denoise-models/{model}",
    params(("model" = String, Path, description = "Model name")),
    responses(
        (status = 202, description = "Download started"),
        (status = 404, description = "No such model", body = ApiError),
        (status = 409, description = "Already downloading", body = ApiError),
        (status = 503, description = "No data directory to keep models in", body = ApiError),
    ),
)]
pub(super) async fn download_denoise_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let model = parse(&name)?;
    if state.engine.denoise_models().path(model).is_none() {
        return Err(AppError::unavailable("no data directory to keep models in"));
    }
    if !state.denoise.start(&state.engine, model) {
        return Err(AppError::conflict(format!("{name} is already downloading")));
    }
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    delete, path = "/api/denoise-models/{model}",
    params(("model" = String, Path, description = "Model name")),
    responses(
        (status = 204, description = "Model removed"),
        (status = 404, description = "No such model", body = ApiError),
    ),
)]
pub(super) async fn delete_denoise_model(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, AppError> {
    let model = parse(&name)?;
    state.denoise.forget(model);
    state
        .engine
        .denoise_models()
        .remove(model)
        .map_err(|error| AppError::internal(error.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}
