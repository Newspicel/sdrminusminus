use super::*;

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels/{ch}/scanner",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "The decoder the scan drives"),
    ),
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
        (status = 400, description = "Unusable scan settings, set not running, decoder already \
                                      scanning or hunted, not scanning, or nothing held to skip",
                                      body = ApiError),
        (status = 404, description = "Device set or decoder not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn scan_channel(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(req): Json<ScanRequest>,
) -> Result<Json<ScannerStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<ScannerStatus, AppError> {
        match req.action {
            ScanAction::Start => {
                let settings = req.settings.ok_or_else(|| {
                    AppError::bad_request("starting a scan needs `settings`".to_string())
                })?;
                Ok(engine.start_scan(
                    ds,
                    ScanSettings {
                        channel: ch,
                        ..settings
                    },
                )?)
            }
            ScanAction::Stop => Ok(engine.stop_scan(ds, ch)?),
            ScanAction::Skip => Ok(engine.skip_scan(ds, ch)?),
        }
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels/{ch}/hunt",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "The decoder being hunted"),
    ),
    request_body = HuntRequest,
    responses(
        (
            status = 200,
            description = "Hunt status after `start`, `stop` or `sweep`. Readings arrive as the \
                           `HuntUpdate` WS event",
            body = HuntStatus,
        ),
        (status = 204, description = "`mark` sent one bearing event"),
        (status = 400, description = "Set not running, scanning, already hunting, or not \
                                      hunting", body = ApiError),
        (status = 404, description = "Device set or decoder not found", body = ApiError),
        (status = 409, description = "`No heading`, `No position`, `Not running` or `No hunt \
                                      node`", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn hunt_channel(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(req): Json<HuntRequest>,
) -> Result<Response, AppError> {
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || -> Result<Response, AppError> {
        match req.action {
            HuntAction::Start => {
                let settings = req.settings.ok_or_else(|| {
                    AppError::bad_request("starting a hunt needs `settings`".to_string())
                })?;
                let status = engine.start_hunt(
                    ds,
                    HuntSettings {
                        channel: ch,
                        ..settings
                    },
                )?;
                Ok(Json(status).into_response())
            }
            HuntAction::Sweep => Ok(Json(engine.sweep_hunt(ds, ch, req.settings)?).into_response()),
            HuntAction::Stop => Ok(Json(engine.stop_hunt(ds, ch)?).into_response()),
            HuntAction::Mark => {
                engine.hunt_mark(ds, ch)?;
                Ok(StatusCode::NO_CONTENT.into_response())
            }
        }
    })
    .await?
}
