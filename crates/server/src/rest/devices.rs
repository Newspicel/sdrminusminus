use super::*;

#[utoipa::path(
    get, path = "/api/state",
    responses((status = 200, description = "Full state snapshot", body = StateSnapshot)),
)]
pub(super) async fn get_state(State(state): State<AppState>) -> Json<StateSnapshot> {
    Json(state.engine.snapshot())
}

#[utoipa::path(
    get, path = "/api/devices",
    responses((status = 200, description = "Discovered devices", body = DevicesResponse)),
)]
pub(super) async fn get_devices(
    State(state): State<AppState>,
) -> Result<Json<DevicesResponse>, AppError> {
    let engine = state.engine.clone();
    let devices = tokio::task::spawn_blocking(move || engine.probe_devices()).await?;
    Ok(Json(DevicesResponse { devices }))
}

#[utoipa::path(
    get, path = "/api/position/nmea-devices",
    responses(
        (status = 200, description = "Serial devices available to NMEA GPS nodes", body = NmeaDevicesResponse),
        (status = 500, description = "Serial device discovery failed", body = ApiError),
    ),
)]
pub(super) async fn get_nmea_devices() -> Result<Json<NmeaDevicesResponse>, AppError> {
    let devices = tokio::task::spawn_blocking(crate::gps::nmea_devices)
        .await?
        .map_err(AppError::internal)?;
    Ok(Json(devices))
}

#[utoipa::path(
    post, path = "/api/devicesets",
    request_body = CreateDeviceSetRequest,
    responses(
        (status = 200, description = "Device set created", body = CreatedId),
        (status = 400, description = "Unusable device", body = ApiError),
        (status = 404, description = "Device not found", body = ApiError),
        (status = 409, description = "Device already in use, here or by another program", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_device_set(
    State(state): State<AppState>,
    Json(req): Json<CreateDeviceSetRequest>,
) -> Result<Json<CreatedId>, AppError> {
    let engine = state.engine.clone();
    let id =
        tokio::task::spawn_blocking(move || engine.create_device_set(&req.device_id)).await??;
    Ok(Json(CreatedId { id }))
}

#[utoipa::path(
    post, path = "/api/devices/serial",
    request_body = WriteSerialRequest,
    responses(
        (status = 200, description = "Serial written, takes effect after a replug", body = WrittenSerial),
        (status = 400, description = "This radio cannot take that serial", body = ApiError),
        (status = 404, description = "Device not found", body = ApiError),
        (status = 409, description = "Device open here or in another program", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn write_serial(
    State(state): State<AppState>,
    Json(req): Json<WriteSerialRequest>,
) -> Result<Json<WrittenSerial>, AppError> {
    let engine = state.engine.clone();
    let serial = tokio::task::spawn_blocking(move || {
        engine.write_serial(&req.device_id, req.serial.as_deref())
    })
    .await??;
    Ok(Json(WrittenSerial { serial }))
}

#[utoipa::path(
    delete, path = "/api/devicesets/{ds}",
    params(("ds" = u32, Path, description = "Device set id")),
    responses(
        (status = 204, description = "Device set removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
    ),
)]
pub(super) async fn delete_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.remove_device_set(ds)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch, path = "/api/devicesets/{ds}/device",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = DeviceSettings,
    responses(
        (status = 204, description = "Settings applied"),
        (status = 400, description = "Unsupported setting", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn patch_device(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(settings): Json<DeviceSettings>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let edit = workspace::begin_edit(&state, ds, None);
        let calibrates = settings.ppm.is_some() || settings.offset_hz.is_some();
        state.engine.patch_device(ds, settings)?;
        if calibrates {
            calibration::remember_live(&state.engine, &state.store, ds);
        }
        if let Some(edit) = edit {
            workspace::finish_edit(&state, edit);
        }
        crate::placement::settle_active(&state);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = CreateChannelRequest,
    responses(
        (status = 200, description = "Channel created", body = CreatedId),
        (status = 400, description = "Invalid channel settings", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_channel(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<CreateChannelRequest>,
) -> Result<Json<CreatedId>, AppError> {
    let engine = state.engine.clone();
    let id = tokio::task::spawn_blocking(move || engine.add_channel(ds, req.stream, req.settings))
        .await??;
    Ok(Json(CreatedId { id }))
}

#[utoipa::path(
    patch, path = "/api/devicesets/{ds}/channels/{ch}",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "Channel id"),
    ),
    request_body = ChannelSettings,
    responses(
        (status = 204, description = "Settings applied"),
        (status = 400, description = "Invalid channel settings", body = ApiError),
        (status = 404, description = "Device set or channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn patch_channel(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(settings): Json<ChannelSettings>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let edit = workspace::begin_edit(&state, ds, Some(ch));
        state.engine.patch_channel(ds, ch, settings)?;
        if let Some(edit) = edit {
            workspace::finish_edit(&state, edit);
        }
        crate::placement::settle_active(&state);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete, path = "/api/devicesets/{ds}/channels/{ch}",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "Channel id"),
    ),
    responses(
        (status = 204, description = "Channel removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Device set or channel not found", body = ApiError),
    ),
)]
pub(super) async fn delete_channel(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.remove_channel(ds, ch)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/api/channeltypes",
    responses((status = 200, description = "Available channel types", body = ChannelTypesResponse)),
)]
pub(super) async fn get_channel_types(State(state): State<AppState>) -> Json<ChannelTypesResponse> {
    Json(ChannelTypesResponse {
        types: state.engine.channel_types(),
        facets: sdrmm_wire::event_facets(),
    })
}

#[utoipa::path(
    get, path = "/api/saved-radios",
    responses((status = 200, description = "Saved network radios", body = Vec<SavedRadio>)),
)]
pub(super) async fn list_saved_radios(
    State(state): State<AppState>,
) -> Result<Json<Vec<SavedRadio>>, AppError> {
    let store = state.store.clone();
    let radios = tokio::task::spawn_blocking(move || store.list_saved_radios()).await??;
    Ok(Json(radios))
}

#[utoipa::path(
    post, path = "/api/saved-radios",
    request_body = SaveRadioRequest,
    responses(
        (status = 200, description = "Radio saved, or its label updated", body = CreatedRowId),
        (status = 400, description = "Blank device id or label", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn save_radio(
    State(state): State<AppState>,
    Json(req): Json<SaveRadioRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let req = SaveRadioRequest {
        device_id: req.device_id.trim().to_string(),
        label: req.label.trim().to_string(),
    };
    if req.device_id.is_empty() || req.label.is_empty() {
        return Err(AppError::bad_request(
            "a saved radio needs a device id and a label",
        ));
    }
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let id = store.save_radio(&req)?;
        engine.emit_scope(StateScope::SavedRadios);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    delete, path = "/api/saved-radios/{id}",
    params(("id" = i64, Path, description = "Saved radio id")),
    responses(
        (status = 204, description = "Saved radio removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Saved radio not found", body = ApiError),
    ),
)]
pub(super) async fn delete_saved_radio(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        store.delete_saved_radio(id)?;
        engine.emit_scope(StateScope::SavedRadios);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}
