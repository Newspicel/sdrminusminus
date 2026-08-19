use super::*;

#[utoipa::path(
    get, path = "/api/presets",
    responses((status = 200, description = "Stored presets", body = Vec<PresetInfo>)),
)]
pub(super) async fn list_presets(
    State(state): State<AppState>,
) -> Result<Json<Vec<PresetInfo>>, AppError> {
    let store = state.store.clone();
    let presets = tokio::task::spawn_blocking(move || store.list_presets()).await??;
    Ok(Json(presets))
}

#[utoipa::path(
    post, path = "/api/presets",
    request_body = CreatePresetRequest,
    responses(
        (status = 200, description = "Preset stored", body = CreatedRowId),
        (
            status = 400,
            description = "No active workspace, or none of its device nodes is on a live radio",
            body = ApiError,
        ),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_preset(
    State(state): State<AppState>,
    Json(req): Json<CreatePresetRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let active = store
            .active_workspace()?
            .ok_or_else(|| AppError::bad_request("no active workspace to snapshot".to_owned()))?;
        let live = engine.snapshot();
        let devices: Vec<PresetDevice> = workspace::bind(&active.snapshot.graph, &live)
            .into_iter()
            .filter_map(|binding| {
                let set = live
                    .device_sets
                    .iter()
                    .find(|set| set.id == binding.device_set)?;
                Some(PresetDevice {
                    node: binding.node,
                    device_id: set.device.id(),
                    settings: set.settings.clone(),
                    channels: set.channels.iter().map(|c| c.settings.clone()).collect(),
                })
            })
            .collect();
        if devices.is_empty() {
            return Err(AppError::bad_request(
                "no radio on this workspace is open, so there is nothing to save".to_owned(),
            ));
        }
        let snapshot = PresetSnapshot {
            version: PRESET_SNAPSHOT_VERSION,
            devices,
        };
        let id = store.create_preset(&req.name, &snapshot)?;
        engine.emit_scope(StateScope::Presets);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    post, path = "/api/presets/{id}/apply",
    params(("id" = i64, Path, description = "Preset id")),
    responses(
        (status = 204, description = "Preset applied"),
        (
            status = 400,
            description = "Preset rejected by a target radio, or none of its radios is on this \
                           workspace; `detail` reports what state a partial application left \
                           behind",
            body = ApiError,
        ),
        (status = 404, description = "Preset not found", body = ApiError),
    ),
)]
pub(super) async fn apply_preset(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let snapshot = store.preset_snapshot(id)?;
        if snapshot.version != PRESET_SNAPSHOT_VERSION {
            return Err(AppError::bad_request(format!(
                "preset {id} has unsupported snapshot version {} (this build applies \
                 {PRESET_SNAPSHOT_VERSION})",
                snapshot.version
            )));
        }
        let active = store.active_workspace()?.ok_or_else(|| {
            AppError::bad_request("no active workspace to apply a preset to".to_owned())
        })?;
        let live = engine.snapshot();
        let bindings = workspace::bind(&active.snapshot.graph, &live);

        let mut targets: Vec<(u32, PresetDevice)> = Vec::new();
        for device in snapshot.devices {
            let free = |set: u32| !targets.iter().any(|(taken, _)| *taken == set);
            let found = bindings
                .iter()
                .find(|binding| binding.node == device.node && free(binding.device_set))
                .or_else(|| {
                    bindings.iter().find(|binding| {
                        free(binding.device_set)
                            && live.device_sets.iter().any(|set| {
                                set.id == binding.device_set && set.device.id() == device.device_id
                            })
                    })
                });
            if let Some(binding) = found {
                targets.push((binding.device_set, device));
            }
        }
        if targets.is_empty() {
            return Err(AppError::bad_request(format!(
                "preset {id} names no radio this workspace has open"
            )));
        }

        let total = targets.len();
        for (done, (device_set, device)) in targets.into_iter().enumerate() {
            apply_configuration(
                &engine,
                device_set,
                device.settings,
                device.channels,
                "preset",
            )
            .map_err(|err| {
                let within = err
                    .body
                    .detail
                    .as_deref()
                    .map_or(String::new(), |detail| format!("; {detail}"));
                err.with_detail(format!(
                    "{done} of {total} radios in the preset were configured{within}"
                ))
            })?;
        }
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn apply_configuration(
    engine: &sdrmm_engine::Engine,
    ds: u32,
    settings: DeviceSettings,
    channels: Vec<ChannelSettings>,
    what: &str,
) -> Result<(), AppError> {
    engine.validate_configuration(ds, &settings, &channels)?;
    let existing: Vec<u32> = engine
        .snapshot()
        .device_sets
        .iter()
        .find(|s| s.id == ds)
        .ok_or(EngineError::DeviceSetNotFound(ds))?
        .channels
        .iter()
        .map(|c| c.id)
        .collect();
    let total_existing = existing.len();
    for (done, ch) in existing.into_iter().enumerate() {
        engine.remove_channel(ds, ch).map_err(|e| {
            AppError::from(e).with_detail(format!(
                "{what} partially applied: removed {done} of {total_existing} existing \
                 channels, device settings untouched, no {what} channels added"
            ))
        })?;
    }
    engine.patch_device(ds, settings).map_err(|e| {
        AppError::from(e).with_detail(format!(
            "{what} partially applied: all {total_existing} existing channels removed, \
             device settings untouched, no {what} channels added"
        ))
    })?;
    let total_new = channels.len();
    for (done, settings) in channels.into_iter().enumerate() {
        engine.add_channel(ds, 0, settings).map_err(|e| {
            AppError::from(e).with_detail(format!(
                "{what} partially applied: existing channels removed, device settings \
                 applied, {done} of {total_new} {what} channels added"
            ))
        })?;
    }
    Ok(())
}

#[utoipa::path(
    delete, path = "/api/presets/{id}",
    params(("id" = i64, Path, description = "Preset id")),
    responses(
        (status = 204, description = "Preset removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Preset not found", body = ApiError),
    ),
)]
pub(super) async fn delete_preset(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        store.delete_preset(id)?;
        engine.emit_scope(StateScope::Presets);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/api/bookmarks",
    responses((status = 200, description = "Stored bookmarks", body = Vec<Bookmark>)),
)]
pub(super) async fn list_bookmarks(
    State(state): State<AppState>,
) -> Result<Json<Vec<Bookmark>>, AppError> {
    let store = state.store.clone();
    let bookmarks = tokio::task::spawn_blocking(move || store.list_bookmarks()).await??;
    Ok(Json(bookmarks))
}

#[utoipa::path(
    post, path = "/api/bookmarks",
    request_body = CreateBookmarkRequest,
    responses(
        (status = 200, description = "Bookmark stored", body = CreatedRowId),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_bookmark(
    State(state): State<AppState>,
    Json(req): Json<CreateBookmarkRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let id = store.create_bookmark(&req)?;
        engine.emit_scope(StateScope::Bookmarks);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    delete, path = "/api/bookmarks/{id}",
    params(("id" = i64, Path, description = "Bookmark id")),
    responses(
        (status = 204, description = "Bookmark removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Bookmark not found", body = ApiError),
    ),
)]
pub(super) async fn delete_bookmark(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        store.delete_bookmark(id)?;
        engine.emit_scope(StateScope::Bookmarks);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}
