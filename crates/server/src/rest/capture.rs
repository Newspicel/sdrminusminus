use super::*;

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels/{ch}/network-export",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "Channel id"),
    ),
    request_body = ChannelNetworkExportRequest,
    responses(
        (
            status = 200,
            description = "Live status after start or final counters after stop",
            body = NetworkExportStatus,
        ),
        (
            status = 400,
            description = "Invalid destination, inactive export, or conflicting owner",
            body = ApiError,
        ),
        (status = 404, description = "Device set or channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn network_export_channel(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(req): Json<ChannelNetworkExportRequest>,
) -> Result<Json<NetworkExportStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || match req.action {
        NetworkExportAction::Start => engine
            .start_channel_network_export(ds, ch, req.node, req.settings)
            .map_err(AppError::from),
        NetworkExportAction::Stop => engine
            .stop_channel_network_export(ds, ch, &req.node)
            .map_err(AppError::from),
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/time-machine",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = TimeMachineRequest,
    responses(
        (
            status = 200,
            description = "History status after the action: `arm` starts the rolling buffer, \
                           `capture` writes the buffered past into a SigMF pair and keeps \
                           appending, `stop` finalizes that pair, `disarm` releases the buffer",
            body = TimeMachineStatus,
        ),
        (
            status = 400,
            description = "Cannot hold history: no recordings directory, set not running, a \
                           window that does not fit in memory, or an action the current state \
                           has nothing to do with",
            body = ApiError,
        ),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn time_machine_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<TimeMachineRequest>,
) -> Result<Json<TimeMachineStatus>, AppError> {
    let gps_state = state.clone();
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<TimeMachineStatus, AppError> {
        let finalizes = matches!(
            req.action,
            TimeMachineAction::Stop | TimeMachineAction::Disarm
        );
        let status =
            engine.control_time_machine(ds, req.node, req.stream, req.action, req.settings)?;
        if finalizes && let Some(dir) = engine.recordings_dir() {
            {
                let _gate = lock_gate(&gate);
                reconcile_recordings(dir, &store)?;
            }
            engine.emit_scope(StateScope::Recordings);
        }
        Ok(status)
    })
    .await??;
    gps_state.gps.route_current(&gps_state);
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/network-export",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = NetworkExportRequest,
    responses(
        (status = 200, description = "Live status after start or final counters after stop", body = NetworkExportStatus),
        (status = 400, description = "Invalid destination, inactive export, or conflicting owner", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn network_export_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<NetworkExportRequest>,
) -> Result<Json<NetworkExportStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<NetworkExportStatus, AppError> {
        match req.action {
            NetworkExportAction::Start => engine
                .start_network_export(ds, req.node, req.stream, req.settings)
                .map_err(AppError::from),
            NetworkExportAction::Stop => engine
                .stop_network_export(ds, &req.node)
                .map_err(AppError::from),
        }
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/playback",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = PlaybackRequest,
    responses(
        (status = 200, description = "The transport after the request", body = PlaybackStatus),
        (
            status = 400,
            description = "The set's device is a radio, not a recording",
            body = ApiError,
        ),
        (status = 404, description = "Device set not found", body = ApiError),
    ),
)]
pub(super) async fn control_playback(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(request): Json<PlaybackRequest>,
) -> Result<Json<PlaybackStatus>, AppError> {
    Ok(Json(state.engine.control_playback(ds, &request)?))
}
