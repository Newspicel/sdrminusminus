use super::*;

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/scanner",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = ScanRequest,
    responses(
        (
            status = 200,
            description = "Scanner status: the initial state after `start`, the final state \
                           after `stop`, the state after `skip` lets go of a held frequency. \
                           Live progress arrives as the `ScannerUpdate` WS event, not as one \
                           state change per step",
            body = ScannerStatus,
        ),
        (status = 400, description = "Unusable scan settings, set not running, already \
                                      scanning, not scanning, or nothing held to skip",
                                      body = ApiError),
        (status = 404, description = "Device set or decoder not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn scan_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<ScanRequest>,
) -> Result<Json<ScannerStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<ScannerStatus, AppError> {
        match req.action {
            ScanAction::Start => {
                let settings = req.settings.ok_or_else(|| {
                    AppError::bad_request("starting a scan needs `settings`".to_string())
                })?;
                Ok(engine.start_scan(ds, settings)?)
            }
            ScanAction::Stop => Ok(engine.stop_scan(ds)?),
            ScanAction::Skip => Ok(engine.skip_scan(ds)?),
        }
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/hunt",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = HuntRequest,
    responses(
        (
            status = 200,
            description = "Hunt status: the initial state after `start`, the final state after \
                           `stop`. Readings arrive as the `HuntUpdate` WS event",
            body = HuntStatus,
        ),
        (status = 400, description = "Set not running, scanning, already hunting, or not \
                                      hunting", body = ApiError),
        (status = 404, description = "Device set or decoder not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn hunt_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<HuntRequest>,
) -> Result<Json<HuntStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<HuntStatus, AppError> {
        match req.action {
            HuntAction::Start => {
                let settings = req.settings.ok_or_else(|| {
                    AppError::bad_request("starting a hunt needs `settings`".to_string())
                })?;
                Ok(engine.start_hunt(ds, settings)?)
            }
            HuntAction::Stop => Ok(engine.stop_hunt(ds)?),
        }
    })
    .await??;
    Ok(Json(status))
}
