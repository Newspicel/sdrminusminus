use axum::{
    body::Body,
    extract::{
        FromRequest, FromRequestParts, State,
        rejection::{JsonRejection, PathRejection, QueryRejection},
    },
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use sdrmm_engine::EngineError;
use sdrmm_recorder::{
    AUDIO_SUFFIX, Export, ExportKind, SigmfMeta, SigmfReader, data_path, meta_path,
    read_audio_info, scan_audio, scan_stems,
};
use sdrmm_tools::ToolError;
use sdrmm_wire::{
    AboutResponse, ApiError, ApplyTemplateRequest, AudioRecordingInfo, AudioRecordingStatus,
    AudioRecordingsResponse, AuthInfo, BandPlan, BandRegionMatch, BandRegionsResponse, Bookmark,
    CapturedImagesResponse, ChannelNetworkExportRequest, ChannelRecordRequest, ChannelSettings,
    ChannelTypesResponse, ClientCommand, ClientsResponse, CreateBookmarkRequest,
    CreateChannelRequest, CreateDeviceSetRequest, CreatePresetRequest, CreateWorkspaceRequest,
    CreatedId, CreatedRowId, DecoderLogEntry, DecoderLogQuery, DecoderLogResponse, DeletedCount,
    DeviceInfo, DeviceSettings, DevicesResponse, DoctorReport, ExportFormat, HuntAction,
    HuntRequest, HuntStatus, IonosondeReport, LicenseTextResponse, LocateQuery,
    NetworkExportAction, NetworkExportRequest, NetworkExportStatus, NmeaDevicesResponse, NodeBody,
    OccupancyReport, PRESET_SNAPSHOT_VERSION, PatchApplyReport, PatchBinding, PatchCatalog,
    PatchRefusal, PlaybackRequest, PlaybackStatus, PresetDevice, PresetInfo, PresetSnapshot,
    RecordAction, RecordRequest, RecordingDownloadQuery, RecordingFormat, RecordingStatus,
    RecordingsResponse, ScanAction, ScanRequest, ScanSessionRequest, ScanSessionStatus,
    ScannerStatus, ServerEvent, StateScope, StateSnapshot, TemplateInfo, TemplatesResponse,
    TimeMachineAction, TimeMachineRequest, TimeMachineStatus, ToolRequest, ToolResponse,
    ToolsResponse, UpdateWorkspaceRequest, VoiceCallsResponse, WorkspaceDetail, WorkspaceInfo,
    WorkspaceSnapshot, WorkspaceState, WorkspacesResponse,
};
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    store::{RecordingRow, SteppedWorkspace, Store, StoreError},
    workspace,
};

#[derive(Debug)]
pub(crate) struct AppError {
    status: StatusCode,
    body: ApiError,
}

impl AppError {
    fn bad_request(message: String) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            body: ApiError {
                error: message,
                detail: None,
            },
        }
    }

    fn not_found(message: String) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            body: ApiError {
                error: message,
                detail: None,
            },
        }
    }

    fn internal(message: String) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiError {
                error: message,
                detail: None,
            },
        }
    }

    fn with_detail(mut self, detail: String) -> Self {
        self.body.detail = Some(detail);
        self
    }
}

#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub(crate) struct Json<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(AppError))]
pub(crate) struct Path<T>(pub T);

#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(AppError))]
pub(crate) struct Query<T>(pub T);

impl From<JsonRejection> for AppError {
    fn from(rej: JsonRejection) -> Self {
        Self {
            status: rej.status(),
            body: ApiError {
                error: "invalid request body".to_string(),
                detail: Some(rej.body_text()),
            },
        }
    }
}

impl From<PathRejection> for AppError {
    fn from(rej: PathRejection) -> Self {
        Self {
            status: rej.status(),
            body: ApiError {
                error: "invalid path parameter".to_string(),
                detail: Some(rej.body_text()),
            },
        }
    }
}

impl From<QueryRejection> for AppError {
    fn from(rej: QueryRejection) -> Self {
        Self {
            status: rej.status(),
            body: ApiError {
                error: "invalid query parameter".to_string(),
                detail: Some(rej.body_text()),
            },
        }
    }
}

impl From<EngineError> for AppError {
    fn from(err: EngineError) -> Self {
        let status = if err.is_not_found() {
            StatusCode::NOT_FOUND
        } else if err.is_bad_request() {
            StatusCode::BAD_REQUEST
        } else if err.is_conflict() {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        Self {
            status,
            body: ApiError {
                error: err.to_string(),
                detail: None,
            },
        }
    }
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        let status = match err {
            StoreError::PresetNotFound(_)
            | StoreError::BookmarkNotFound(_)
            | StoreError::RecordingNotFound(_)
            | StoreError::WorkspaceNotFound(_) => StatusCode::NOT_FOUND,
            StoreError::Timestamp(_) | StoreError::Sources(_) | StoreError::WorkspaceLayout(_) => {
                StatusCode::BAD_REQUEST
            }
            StoreError::WorkspaceNameTaken(_)
            | StoreError::WorkspaceConflict { .. }
            | StoreError::WorkspaceHistoryEnd { .. } => StatusCode::CONFLICT,
            StoreError::Db(_) | StoreError::Corrupt(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            body: ApiError {
                error: err.to_string(),
                detail: None,
            },
        }
    }
}

impl From<ToolError> for AppError {
    fn from(err: ToolError) -> Self {
        let status = if err.is_not_found() {
            StatusCode::NOT_FOUND
        } else if err.is_bad_request() {
            StatusCode::BAD_REQUEST
        } else if err.is_unavailable() {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        Self {
            status,
            body: ApiError {
                error: err.to_string(),
                detail: None,
            },
        }
    }
}

impl From<tokio::task::JoinError> for AppError {
    fn from(err: tokio::task::JoinError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            body: ApiError {
                error: "engine task failed".to_string(),
                detail: Some(err.to_string()),
            },
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

#[utoipa::path(
    get, path = "/api/state",
    responses((status = 200, description = "Full state snapshot", body = StateSnapshot)),
)]
async fn get_state(State(state): State<AppState>) -> Json<StateSnapshot> {
    Json(state.engine.snapshot())
}

#[utoipa::path(
    get, path = "/api/devices",
    responses((status = 200, description = "Discovered devices", body = DevicesResponse)),
)]
async fn get_devices(State(state): State<AppState>) -> Result<Json<DevicesResponse>, AppError> {
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
async fn get_nmea_devices() -> Result<Json<NmeaDevicesResponse>, AppError> {
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
async fn create_device_set(
    State(state): State<AppState>,
    Json(req): Json<CreateDeviceSetRequest>,
) -> Result<Json<CreatedId>, AppError> {
    let engine = state.engine.clone();
    let id =
        tokio::task::spawn_blocking(move || engine.create_device_set(&req.device_id)).await??;
    Ok(Json(CreatedId { id }))
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
async fn delete_device_set(
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
async fn patch_device(
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
        state.engine.patch_device(ds, settings)?;
        if let Some(edit) = edit {
            workspace::finish_edit(&state, edit);
        }
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
async fn create_channel(
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
async fn patch_channel(
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
async fn delete_channel(
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
async fn get_channel_types(State(state): State<AppState>) -> Json<ChannelTypesResponse> {
    Json(ChannelTypesResponse {
        types: state.engine.channel_types(),
    })
}

#[utoipa::path(
    get, path = "/api/calls",
    responses((status = 200, description = "Completed temporary voice calls", body = VoiceCallsResponse)),
)]
async fn list_calls(State(state): State<AppState>) -> Json<VoiceCallsResponse> {
    Json(VoiceCallsResponse {
        calls: state.calls.list(),
    })
}

#[utoipa::path(
    get, path = "/api/calls/{id}/audio",
    params(("id" = u64, Path, description = "Call id")),
    responses(
        (status = 200, description = "Call audio as mono 48 kHz PCM", content_type = "audio/wav"),
        (status = 404, description = "Call or clear audio not found", body = ApiError),
    ),
)]
async fn call_audio(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let audio = state
        .calls
        .audio(id)
        .ok_or_else(|| AppError::not_found(format!("audio for call {id} not found")))?;
    let headers = [
        (header::CONTENT_TYPE, "audio/wav".to_owned()),
        (header::CONTENT_LENGTH, audio.len().to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"call-{id}.wav\""),
        ),
        (header::CACHE_CONTROL, "private, max-age=3600".to_owned()),
    ];
    Ok((headers, Body::from(audio)).into_response())
}

pub(crate) fn call_audio_path(id: u64) -> String {
    format!("/api/calls/{id}/audio")
}

#[utoipa::path(
    get, path = "/api/images",
    responses((status = 200, description = "Pictures captured from scanning modes", body = CapturedImagesResponse)),
)]
async fn list_images(State(state): State<AppState>) -> Json<CapturedImagesResponse> {
    Json(CapturedImagesResponse {
        images: state.images.list(),
    })
}

#[utoipa::path(
    get, path = "/api/images/{id}/png",
    params(("id" = u64, Path, description = "Captured picture id")),
    responses(
        (status = 200, description = "The captured picture", content_type = "image/png"),
        (status = 404, description = "Picture not found", body = ApiError),
    ),
)]
async fn captured_image(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Response, AppError> {
    let png = state
        .images
        .png(id)
        .ok_or_else(|| AppError::not_found(format!("picture {id} not found")))?;
    let headers = [
        (header::CONTENT_TYPE, "image/png".to_owned()),
        (header::CONTENT_LENGTH, png.len().to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"picture-{id}.png\""),
        ),
        (header::CACHE_CONTROL, "private, max-age=3600".to_owned()),
    ];
    Ok((headers, Body::from(png)).into_response())
}

pub(crate) fn captured_image_path(id: u64) -> String {
    format!("/api/images/{id}/png")
}

#[utoipa::path(
    get, path = "/api/presets",
    responses((status = 200, description = "Stored presets", body = Vec<PresetInfo>)),
)]
async fn list_presets(State(state): State<AppState>) -> Result<Json<Vec<PresetInfo>>, AppError> {
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
async fn create_preset(
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
async fn apply_preset(
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

fn apply_configuration(
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
async fn delete_preset(
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
async fn list_bookmarks(State(state): State<AppState>) -> Result<Json<Vec<Bookmark>>, AppError> {
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
async fn create_bookmark(
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
async fn delete_bookmark(
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

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/record",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = RecordRequest,
    responses(
        (
            status = 200,
            description = "Recording status: live after `start`; final counts after `stop`, \
                           where `error` reports a truncated recording and the finalized pair \
                           appears in `GET /api/recordings`",
            body = RecordingStatus,
        ),
        (status = 400, description = "Cannot record: no recordings directory, set not \
                                      running, already recording, or not recording", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn record_device_set(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(req): Json<RecordRequest>,
) -> Result<Json<RecordingStatus>, AppError> {
    let gps_state = state.clone();
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<RecordingStatus, AppError> {
        match req.action {
            RecordAction::Start => {
                engine.start_recording(ds, req.stream)?;
                engine
                    .snapshot()
                    .device_sets
                    .into_iter()
                    .find(|s| s.id == ds)
                    .and_then(|s| s.recording)
                    .ok_or_else(|| {
                        AppError::internal(
                            "recording vanished before its first status snapshot".to_string(),
                        )
                    })
            }
            RecordAction::Stop => {
                let finalized = engine.stop_recording(ds)?;
                if let Some(dir) = engine.recordings_dir() {
                    {
                        let _gate = lock_gate(&gate);
                        reconcile_recordings(dir, &store)?;
                    }
                    engine.emit_scope(StateScope::Recordings);
                }
                let file = finalized
                    .stem
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map_or_else(|| finalized.stem.display().to_string(), str::to_string);
                Ok(RecordingStatus {
                    file,
                    stream: finalized.stream,
                    started_at: finalized.started_at,
                    samples: finalized.samples,
                    bytes: finalized.bytes,
                    overruns: finalized.overruns,
                    error: finalized.error,
                })
            }
        }
    })
    .await??;
    gps_state.gps.route_current(&gps_state);
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels/{ch}/record",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "Channel id"),
    ),
    request_body = ChannelRecordRequest,
    responses(
        (
            status = 200,
            description = "Recording status: live after `start`; final counts after `stop`, \
                           where `error` reports a recording that was cut short and the \
                           finished file appears in `GET /api/audiorecordings`",
            body = AudioRecordingStatus,
        ),
        (
            status = 400,
            description = "Cannot record: no recordings directory, set not running, channel \
                           already recording, not recording, or a channel with no audio",
            body = ApiError,
        ),
        (status = 404, description = "Device set or channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn record_channel_audio(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(req): Json<ChannelRecordRequest>,
) -> Result<Json<AudioRecordingStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || match req.action {
        RecordAction::Start => engine
            .start_channel_recording(ds, ch)
            .map_err(AppError::from),
        RecordAction::Stop => engine
            .stop_channel_recording(ds, ch)
            .map_err(AppError::from),
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/channels/{ch}/baseband",
    params(
        ("ds" = u32, Path, description = "Device set id"),
        ("ch" = u32, Path, description = "Channel id"),
    ),
    request_body = ChannelRecordRequest,
    responses(
        (
            status = 200,
            description = "Baseband recording status: live after `start`; final counts after \
                           `stop`, where the finished SigMF pair appears in \
                           `GET /api/recordings`",
            body = RecordingStatus,
        ),
        (
            status = 400,
            description = "Cannot record: no recordings directory, set not running, or this \
                           channel's baseband is already recording",
            body = ApiError,
        ),
        (status = 404, description = "Device set or channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn record_channel_baseband(
    State(state): State<AppState>,
    Path((ds, ch)): Path<(u32, u32)>,
    Json(req): Json<ChannelRecordRequest>,
) -> Result<Json<RecordingStatus>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<RecordingStatus, AppError> {
        match req.action {
            RecordAction::Start => engine
                .start_channel_baseband_recording(ds, ch)
                .map_err(AppError::from),
            RecordAction::Stop => {
                let status = engine.stop_channel_baseband_recording(ds, ch)?;
                if let Some(dir) = engine.recordings_dir() {
                    {
                        let _gate = lock_gate(&gate);
                        reconcile_recordings(dir, &store)?;
                    }
                    engine.emit_scope(StateScope::Recordings);
                }
                Ok(status)
            }
        }
    })
    .await??;
    Ok(Json(status))
}

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
async fn network_export_channel(
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
async fn time_machine_device_set(
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
    get, path = "/api/audiorecordings",
    responses((
        status = 200,
        description = "The audio-recording library, read off the files themselves",
        body = AudioRecordingsResponse,
    )),
)]
async fn list_audio_recordings(
    State(state): State<AppState>,
) -> Result<Json<AudioRecordingsResponse>, AppError> {
    let engine = state.engine.clone();
    let recordings = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        let Some(dir) = engine.audio_recordings_dir() else {
            return Ok(Vec::new());
        };
        let files = scan_audio(&dir)
            .map_err(|err| AppError::internal(format!("scan {}: {err}", dir.display())))?;
        Ok(files.iter().filter_map(|path| audio_info(path)).collect())
    })
    .await??;
    Ok(Json(AudioRecordingsResponse { recordings }))
}

fn audio_info(path: &std::path::Path) -> Option<AudioRecordingInfo> {
    let file = path.file_name().and_then(|name| name.to_str())?.to_owned();
    let info = match read_audio_info(path) {
        Ok(info) => info,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "skipping unreadable audio recording");
            return None;
        }
    };
    Some(AudioRecordingInfo {
        file,
        channels: info.channels,
        sample_rate: info.sample_rate,
        frames: info.frames,
        bytes: info.bytes,
        duration_s: info.duration_s(),
        created_at: file_created_at(path),
    })
}

fn file_created_at(path: &std::path::Path) -> String {
    let at = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| jiff::Timestamp::from_second(since.as_secs() as i64).ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    at.to_string()
}

fn audio_recording_path(state: &AppState, file: &str) -> Result<std::path::PathBuf, AppError> {
    let missing = || AppError::not_found(format!("audio recording `{file}` not found"));
    let dir = state.engine.audio_recordings_dir().ok_or_else(missing)?;
    let plain = !file.is_empty()
        && file.ends_with(AUDIO_SUFFIX)
        && !file.contains(['/', '\\'])
        && !file.contains("..");
    if !plain {
        return Err(missing());
    }
    let path = dir.join(file);
    if path.is_file() {
        Ok(path)
    } else {
        Err(missing())
    }
}

#[utoipa::path(
    get, path = "/api/audiorecordings/{file}/download",
    params(("file" = String, Path, description = "Audio recording file name, extension included")),
    responses(
        (
            status = 200,
            description = "The recording as a WAV, streamed with an exact `Content-Length`",
            content((String = "audio/wav")),
        ),
        (status = 404, description = "Audio recording not found", body = ApiError),
    ),
)]
async fn download_audio_recording(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<Response, AppError> {
    let name = file.clone();
    let (handle, len) =
        tokio::task::spawn_blocking(move || -> Result<(std::fs::File, u64), AppError> {
            let path = audio_recording_path(&state, &file)?;
            let handle = std::fs::File::open(&path)
                .map_err(|err| AppError::internal(format!("open {}: {err}", path.display())))?;
            let len = handle
                .metadata()
                .map_err(|err| AppError::internal(format!("stat {}: {err}", path.display())))?
                .len();
            Ok((handle, len))
        })
        .await??;
    Ok((
        [
            (header::CONTENT_TYPE, "audio/wav".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{name}\""),
            ),
        ],
        Body::from_stream(byte_stream(std::io::Read::take(handle, len))),
    )
        .into_response())
}

#[utoipa::path(
    delete, path = "/api/audiorecordings/{file}",
    params(("file" = String, Path, description = "Audio recording file name, extension included")),
    responses(
        (status = 204, description = "Audio recording removed"),
        (status = 404, description = "Audio recording not found", body = ApiError),
    ),
)]
async fn delete_audio_recording(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let path = audio_recording_path(&state, &file)?;
        std::fs::remove_file(&path)
            .map_err(|err| AppError::internal(format!("delete {}: {err}", path.display())))?;
        engine.emit_scope(StateScope::Recordings);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
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
async fn network_export_device_set(
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
async fn control_playback(
    State(state): State<AppState>,
    Path(ds): Path<u32>,
    Json(request): Json<PlaybackRequest>,
) -> Result<Json<PlaybackStatus>, AppError> {
    Ok(Json(state.engine.control_playback(ds, &request)?))
}

#[utoipa::path(
    get, path = "/api/recordings",
    responses((
        status = 200,
        description = "The recording library, reconciled with the SigMF pairs on disk",
        body = RecordingsResponse,
    )),
)]
async fn list_recordings(
    State(state): State<AppState>,
) -> Result<Json<RecordingsResponse>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let recordings = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        let Some(dir) = engine.recordings_dir() else {
            return Ok(Vec::new());
        };
        let _gate = lock_gate(&gate);
        reconcile_recordings(dir, &store)?;
        Ok(store.list_recordings(dir)?)
    })
    .await??;
    Ok(Json(RecordingsResponse { recordings }))
}

#[utoipa::path(
    get, path = "/api/recordings/{id}/download",
    params(("id" = i64, Path, description = "Recording id"), RecordingDownloadQuery),
    responses(
        (
            status = 200,
            description = "The recording as a downloadable file, streamed with an exact \
                           `Content-Length`",
            content(
                (String = "application/x-tar"),
                (String = "audio/wav"),
            ),
        ),
        (
            status = 400,
            description = "Unknown format, or a recording the requested container cannot \
                           express (a WAV needs a sample rate and cf32 samples)",
            body = ApiError,
        ),
        (status = 404, description = "Recording not found", body = ApiError),
    ),
)]
async fn download_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<RecordingDownloadQuery>,
) -> Result<Response, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let kind = match query.format {
        RecordingFormat::Sigmf => ExportKind::SigmfArchive,
        RecordingFormat::Wav => ExportKind::Wav,
    };
    let export = tokio::task::spawn_blocking(move || -> Result<Export, AppError> {
        let _gate = lock_gate(&gate);
        let name = store.recording_stem(id)?;
        let dir = engine
            .recordings_dir()
            .ok_or_else(|| AppError::not_found(format!("recording {id} not found")))?;
        Export::open(&dir.join(&name), kind).map_err(|err| export_error(id, err))
    })
    .await??;

    let content_length = export.byte_len();
    let headers = [
        (header::CONTENT_TYPE, export.content_type().to_string()),
        (header::CONTENT_LENGTH, content_length.to_string()),
        (
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", export.file_name()),
        ),
    ];
    Ok((headers, Body::from_stream(byte_stream(export))).into_response())
}

fn export_error(id: i64, err: sdrmm_recorder::SigmfError) -> AppError {
    match err {
        sdrmm_recorder::SigmfError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            AppError::not_found(format!("recording {id} not found"))
        }
        sdrmm_recorder::SigmfError::Unexportable { .. } => AppError::bad_request(err.to_string())
            .with_detail(
                "download it as `format=sigmf`, which carries any recording verbatim".to_string(),
            ),
        other => AppError::internal(format!("export recording {id}: {other}")),
    }
}

fn byte_stream(
    mut source: impl std::io::Read + Send + 'static,
) -> impl futures::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send + 'static {
    const CHUNK: usize = 256 * 1024;

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        loop {
            let mut chunk = vec![0u8; CHUNK];
            let read = match source.read(&mut chunk) {
                Ok(0) => return,
                Ok(read) => read,
                Err(err) => {
                    let _ = tx.blocking_send(Err(err));
                    return;
                }
            };
            chunk.truncate(read);
            if tx.blocking_send(Ok(chunk)).is_err() {
                return;
            }
        }
    });
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|chunk| (chunk, rx))
    })
}

#[utoipa::path(
    delete, path = "/api/recordings/{id}",
    params(("id" = i64, Path, description = "Recording id")),
    responses(
        (status = 204, description = "Recording removed: SigMF pair and index row"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Recording not found", body = ApiError),
    ),
)]
async fn delete_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _gate = lock_gate(&gate);
        let name = store.recording_stem(id)?;
        if let Some(dir) = engine.recordings_dir() {
            let stem = dir.join(&name);
            for path in [meta_path(&stem), data_path(&stem)] {
                if let Err(err) = std::fs::remove_file(&path)
                    && err.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(AppError::internal(format!(
                        "delete {}: {err}",
                        path.display()
                    )));
                }
            }
        }
        store.delete_recording(id)?;
        engine.emit_scope(StateScope::Recordings);
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/api/decoderlog",
    params(DecoderLogQuery),
    responses(
        (
            status = 200,
            description = "Stored decodes, newest first, with the total the filter matches \
                           and the frames lost on the way to the log",
            body = DecoderLogResponse,
        ),
        (status = 400, description = "Malformed filter (`since`/`until`, `limit`)", body = ApiError),
    ),
)]
async fn list_decoder_log(
    State(state): State<AppState>,
    Query(filter): Query<DecoderLogQuery>,
) -> Result<Json<DecoderLogResponse>, AppError> {
    let store = state.store.clone();
    let (entries, total) =
        tokio::task::spawn_blocking(move || store.query_decoder_log(&filter)).await??;
    Ok(Json(DecoderLogResponse {
        entries,
        total,
        dropped: state.decoder_log_dropped() + state.engine.decoded_dropped(),
    }))
}

#[utoipa::path(
    delete, path = "/api/decoderlog",
    params(DecoderLogQuery),
    responses(
        (status = 200, description = "Entries removed", body = DeletedCount),
        (status = 400, description = "Malformed filter (`since`/`until`, `limit`)", body = ApiError),
    ),
)]
async fn clear_decoder_log(
    State(state): State<AppState>,
    Query(filter): Query<DecoderLogQuery>,
) -> Result<Json<DeletedCount>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let deleted = tokio::task::spawn_blocking(move || -> Result<u64, AppError> {
        let deleted = store.delete_decoder_log(&filter)?;
        engine.emit_scope(StateScope::DecoderLog);
        Ok(deleted)
    })
    .await??;
    Ok(Json(DeletedCount { deleted }))
}

#[utoipa::path(
    get, path = "/api/decoderlog/export/{format}",
    params(("format" = ExportFormat, Path, description = "Export encoding"), DecoderLogQuery),
    responses(
        (
            status = 200,
            description = "The matching entries as a downloadable file, capped at the \
                           server's export limit; `limit` is ignored",
            content(
                (String = "text/csv"),
                (Vec<DecoderLogEntry> = "application/json"),
            ),
        ),
        (status = 400, description = "Unknown format or malformed filter", body = ApiError),
    ),
)]
async fn export_decoder_log(
    State(state): State<AppState>,
    Path(format): Path<ExportFormat>,
    Query(filter): Query<DecoderLogQuery>,
) -> Result<Response, AppError> {
    let store = state.store.clone();
    let entries = tokio::task::spawn_blocking(move || store.export_decoder_log(&filter)).await??;
    let (content_type, body) = match format {
        ExportFormat::Csv => ("text/csv; charset=utf-8", csv_export(&entries)),
        ExportFormat::Json => ("application/json", serde_json::to_string(&entries)),
    };
    let body = body
        .map_err(|err| AppError::internal(format!("serializing the decoder-log export: {err}")))?;
    let extension = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
    };
    let stamp = jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ");
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"decoderlog-{stamp}.{extension}\""),
            ),
        ],
        body,
    )
        .into_response())
}

fn csv_export(entries: &[DecoderLogEntry]) -> Result<String, serde_json::Error> {
    let mut out = String::from("at,device_set,channel,kind,freq_hz,station,summary,event\r\n");
    for entry in entries {
        let event = serde_json::to_string(&entry.event)?;
        out.push_str(&csv_field(&entry.at));
        out.push(',');
        out.push_str(&entry.device_set.to_string());
        out.push(',');
        out.push_str(&entry.channel.to_string());
        out.push(',');
        out.push_str(&csv_field(&entry.kind));
        out.push(',');
        out.push_str(&entry.freq_hz.to_string());
        out.push(',');
        out.push_str(&csv_field(entry.station.as_deref().unwrap_or_default()));
        out.push(',');
        out.push_str(&csv_field(&entry.summary));
        out.push(',');
        out.push_str(&csv_field(&event));
        out.push_str("\r\n");
    }
    Ok(out)
}

fn csv_field(value: &str) -> String {
    if value.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub(crate) fn lock_gate(gate: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
    gate.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn reconcile_recordings(dir: &std::path::Path, store: &Store) -> Result<(), AppError> {
    let stems = scan_stems(dir)
        .map_err(|err| AppError::internal(format!("scan {}: {err}", dir.display())))?;
    let mut kept = Vec::with_capacity(stems.len());
    for stem in &stems {
        let Some(name) = stem.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let reader = match SigmfReader::open(stem) {
            Ok(reader) => reader,
            Err(err) => {
                tracing::warn!(stem = %stem.display(), error = %err, "skipping unreadable recording");
                continue;
            }
        };
        let samples = reader.total_samples();
        let meta = reader.meta();
        let Some(sample_rate) = meta.global.sample_rate else {
            tracing::warn!(stem = %stem.display(), "skipping recording without a core:sample_rate");
            continue;
        };
        store.upsert_recording(&RecordingRow {
            stem: name.to_string(),
            created_at: recording_created_at(stem, meta),
            device_label: meta.global.hw.clone().unwrap_or_default(),
            center_hz: meta
                .captures
                .first()
                .and_then(|c| c.frequency)
                .unwrap_or_default(),
            sample_rate,
            samples,
            bytes: samples * sdrmm_recorder::BYTES_PER_SAMPLE,
        })?;
        kept.push(name.to_string());
    }
    store.prune_recordings(&kept)?;
    Ok(())
}

fn recording_created_at(stem: &std::path::Path, meta: &SigmfMeta) -> String {
    meta.captures
        .first()
        .and_then(|c| c.datetime.clone())
        .or_else(|| {
            std::fs::metadata(data_path(stem))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| jiff::Timestamp::try_from(t).ok())
                .map(|ts| ts.to_string())
        })
        .unwrap_or_default()
}

#[utoipa::path(
    post, path = "/api/devicesets/{ds}/scanner",
    params(("ds" = u32, Path, description = "Device set id")),
    request_body = ScanRequest,
    responses(
        (
            status = 200,
            description = "Scanner status: the initial state after `start`, the final state \
                           after `stop`. Live progress arrives as the `ScannerUpdate` WS \
                           event, not as one state change per step",
            body = ScannerStatus,
        ),
        (status = 400, description = "Unusable scan settings, set not running, already \
                                      scanning, or not scanning", body = ApiError),
        (status = 404, description = "Device set or hold channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn scan_device_set(
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
        (status = 400, description = "Unusable hunt settings, set not running, scanning, \
                                      already hunting, or not hunting", body = ApiError),
        (status = 404, description = "Device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn hunt_device_set(
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

#[utoipa::path(
    post, path = "/api/scanner",
    request_body = ScanSessionRequest,
    responses(
        (
            status = 200,
            description = "Every device set in the scan and the state it started or ended in.                            Live progress arrives as one `ScannerUpdate` WS event per set",
            body = ScanSessionStatus,
        ),
        (status = 400, description = "Unusable scan settings, a set that is not running,                                       already scanning, or no scan to stop", body = ApiError),
        (status = 404, description = "Device set or hold channel not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn scan_session(
    State(state): State<AppState>,
    Json(req): Json<ScanSessionRequest>,
) -> Result<Json<ScanSessionStatus>, AppError> {
    let engine = state.engine.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<ScanSessionStatus, AppError> {
        match req.action {
            ScanAction::Start => {
                let settings = req.settings.ok_or_else(|| {
                    AppError::bad_request("starting a scan needs `settings`".to_string())
                })?;
                Ok(engine.start_scan_session(&req.device_sets, settings)?)
            }
            ScanAction::Stop => Ok(engine.stop_scan_session()?),
        }
    })
    .await??;
    Ok(Json(status))
}

#[utoipa::path(
    get, path = "/api/templates",
    responses((
        status = 200,
        description = "Built-in workspace templates (read-only; presets are the writable kind)",
        body = TemplatesResponse,
    )),
)]
async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<TemplatesResponse>, AppError> {
    let engine = state.engine.clone();
    let probed = tokio::task::spawn_blocking(move || engine.probe_devices()).await?;
    let templates = crate::templates::all()
        .iter()
        .map(|template| TemplateInfo {
            supported_devices: probed
                .iter()
                .filter(|device| {
                    device
                        .profile
                        .as_ref()
                        .is_none_or(|profile| template.unmet_by(profile).is_none())
                })
                .map(DeviceInfo::id)
                .collect(),
            ..template.clone()
        })
        .collect();
    Ok(Json(TemplatesResponse { templates }))
}

#[utoipa::path(
    post, path = "/api/templates/{id}/apply",
    params(("id" = String, Path, description = "Template id")),
    request_body = ApplyTemplateRequest,
    responses(
        (status = 204, description = "Template applied"),
        (
            status = 400,
            description = "Template rejected by the target device (usually out of its tuning \
                           range); `detail` reports what a partial application left behind",
            body = ApiError,
        ),
        (status = 404, description = "Template or device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn apply_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApplyTemplateRequest>,
) -> Result<StatusCode, AppError> {
    let template = crate::templates::get(&id)
        .ok_or_else(|| AppError::not_found(format!("template {id} not found")))?;
    let engine = state.engine.clone();
    let store = state.store.clone();
    let settings = DeviceSettings {
        center_hz: Some(template.center_hz),
        sample_rate: Some(template.sample_rate),
        ..DeviceSettings::default()
    };
    let channels = template.channels.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let open = engine.snapshot();
        if let Some(set) = open.device_sets.iter().find(|set| set.id == req.device_set)
            && let Some(reason) = template.unmet_by(&set.capabilities.profile())
        {
            return Err(AppError::bad_request(format!(
                "{} cannot run this template: {reason}",
                set.device.label
            )));
        }
        apply_configuration(&engine, req.device_set, settings, channels, "template")?;
        apply_template_patch(&engine, &store, template, req.device_set)
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

fn apply_template_patch(
    engine: &sdrmm_engine::Engine,
    store: &Store,
    template: &TemplateInfo,
    device_set: u32,
) -> Result<(), AppError> {
    let Some(patch) = &template.patch else {
        return Ok(());
    };
    let Some(mut active) = store.active_workspace()? else {
        return Ok(());
    };
    let device = engine
        .snapshot()
        .device_sets
        .iter()
        .find(|set| set.id == device_set)
        .map(|set| sdrmm_wire::DeviceRef::from_info(&set.device));
    active.snapshot.merge_patch(
        patch,
        &format!("template:{}:", template.id),
        device.as_ref(),
    );
    let update = UpdateWorkspaceRequest {
        revision: active.info.revision,
        name: None,
        snapshot: Some(active.snapshot),
    };
    match store.update_workspace(active.info.id, &update) {
        Ok(_) => {
            engine.emit_scope(StateScope::Workspaces);
            Ok(())
        }
        Err(StoreError::WorkspaceConflict { .. }) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn first_binding(app: &AppState, workspace: i64, node: &str, device_set: u32) -> bool {
    app.restored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((workspace, node.to_string(), device_set))
}

fn note_restore(app: &AppState, node: &str, restored: bool) {
    let mut unrestored = app
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    unrestored.retain(|held| held != node);
    if !restored {
        unrestored.push(node.to_string());
    }
}

fn forget_closed_bindings(app: &AppState, state: &StateSnapshot) {
    app.restored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(_, _, set)| state.device_sets.iter().any(|live| live.id == *set));
}

fn bring_up(
    app: &AppState,
    workspace: i64,
    snapshot: &WorkspaceSnapshot,
    saved: &WorkspaceState,
) -> Result<PatchApplyReport, AppError> {
    let engine = &app.engine;
    let mut report = PatchApplyReport::default();
    let mut state = engine.snapshot();
    forget_closed_bindings(app, &state);

    for (node, device_set) in workspace::bind_devices(&snapshot.graph, &state) {
        if first_binding(app, workspace, &node, device_set) {
            match workspace::restore_device(engine, device_set, &node, saved) {
                Ok(()) => note_restore(app, &node, true),
                Err(reason) => {
                    note_restore(app, &node, false);
                    report.refused.push(PatchRefusal {
                        node: node.clone(),
                        reason,
                    });
                }
            }
        }
        report.bound.push(PatchBinding { node, device_set });
    }

    let mut attached: Option<Vec<DeviceInfo>> = None;
    for node in snapshot.graph.device_nodes() {
        let NodeBody::Device(device) = &node.body else {
            continue;
        };
        let Some(reference) = &device.device else {
            continue;
        };
        if report.bound.iter().any(|bound| bound.node == node.id) {
            continue;
        }
        let devices = attached.get_or_insert_with(|| engine.probe_devices());
        let open = devices
            .iter()
            .filter(|info| reference.matches(info))
            .find(|info| {
                !state
                    .device_sets
                    .iter()
                    .any(|set| set.device.id() == info.id())
            })
            .map(DeviceInfo::id);
        match open {
            Some(device_id) => match engine.create_device_set(&device_id) {
                Ok(id) => {
                    report.opened += 1;
                    first_binding(app, workspace, &node.id, id);
                    match workspace::restore_device(engine, id, &node.id, saved) {
                        Ok(()) => note_restore(app, &node.id, true),
                        Err(reason) => {
                            note_restore(app, &node.id, false);
                            report.refused.push(PatchRefusal {
                                node: node.id.clone(),
                                reason,
                            });
                        }
                    }
                    report.bound.push(PatchBinding {
                        node: node.id.clone(),
                        device_set: id,
                    });
                    state = engine.snapshot();
                }
                Err(err) => report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: err.to_string(),
                }),
            },
            None => report.absent.push(node.id.clone()),
        }
    }

    for binding in &report.bound {
        let Some(set) = state
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        let mut live: Vec<(&str, u32)> = set
            .channels
            .iter()
            .map(|channel| (channel.settings.params.type_id(), channel.stream))
            .collect();
        for (node, stream) in snapshot.graph.channels_of(&binding.node) {
            let NodeBody::Channel(channel) = &node.body else {
                continue;
            };
            if let Some(at) = live.iter().position(|(type_id, live_stream)| {
                *type_id == channel.channel_type && *live_stream == stream
            }) {
                live.remove(at);
                continue;
            }
            let Some(settings) =
                workspace::channel_settings(&node.id, &channel.channel_type, saved)
            else {
                report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: format!("this build has no channel type {:?}", channel.channel_type),
                });
                continue;
            };
            if let Err(err) = engine.add_channel(set.id, stream, settings) {
                report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: err.to_string(),
                });
            } else {
                report.created += 1;
            }
        }
    }
    Ok(report)
}

#[utoipa::path(
    get, path = "/api/workspaces",
    responses((
        status = 200,
        description = "Stored workspaces and which one is active. Layouts are not included — \
                       fetch one workspace for that",
        body = WorkspacesResponse,
    )),
)]
async fn list_workspaces(
    State(state): State<AppState>,
) -> Result<Json<WorkspacesResponse>, AppError> {
    let store = state.store.clone();
    let workspaces = tokio::task::spawn_blocking(move || store.list_workspaces()).await??;
    Ok(Json(workspaces))
}

#[utoipa::path(
    post, path = "/api/workspaces",
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace stored", body = CreatedRowId),
        (status = 400, description = "Layout rejected", body = ApiError),
        (status = 409, description = "A workspace of that name already exists", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn create_workspace(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let snapshot = req.snapshot.unwrap_or_else(WorkspaceSnapshot::empty);
        let id = store.create_workspace(&req.name, &snapshot)?;
        engine.emit_scope(StateScope::Workspaces);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    get, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "The workspace and its layout", body = WorkspaceDetail),
        (status = 404, description = "Workspace not found", body = ApiError),
        (
            status = 500,
            description = "The stored layout no longer parses — the row is left intact so a \
                           newer build can still read it",
            body = ApiError,
        ),
    ),
)]
async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    let store = state.store.clone();
    let workspace = tokio::task::spawn_blocking(move || store.workspace(id)).await??;
    Ok(Json(workspace))
}

#[utoipa::path(
    put, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    request_body = UpdateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceInfo),
        (status = 400, description = "Layout rejected", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (
            status = 409,
            description = "Another client wrote first (stale revision) or the name is taken; \
                           reload and reapply",
            body = ApiError,
        ),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn update_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> Result<Json<WorkspaceInfo>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let info = tokio::task::spawn_blocking(move || -> Result<WorkspaceInfo, AppError> {
        let info = store.update_workspace(id, &req)?;
        engine.emit_scope(StateScope::Workspaces);
        Ok(info)
    })
    .await??;
    reconcile_gps(state).await?;
    Ok(Json(info))
}

#[utoipa::path(
    delete, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 204, description = "Workspace removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
async fn delete_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let gps_state = state.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.store.active_workspace_id()?;
        let after = state.store.delete_workspace(id)?;
        if let Some(promoted) = after.filter(|promoted| Some(*promoted) != before) {
            let detail = state.store.workspace(promoted)?;
            let saved = state.store.workspace_state(promoted)?;
            workspace::reconcile(&state, &detail.snapshot.graph, &saved);
        }
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(())
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/activate",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 204,
            description = "Workspace activated for every client. The hardware is reconciled to \
                           it: radios this workspace does not name are closed, channels it does \
                           not draw are dropped, and the radios it keeps are put back where it \
                           was left. Apply opens the rest",
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
async fn activate_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let gps_state = state.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = workspace::save_active(&state) {
            tracing::warn!(%err, "could not save the outgoing workspace before the switch");
        }
        state.store.activate_workspace(id)?;
        let detail = state.store.workspace(id)?;
        let saved = state.store.workspace_state(id)?;
        let report = workspace::reconcile(&state, &detail.snapshot.graph, &saved);
        tracing::info!(
            workspace = id,
            closed = report.closed,
            channels = report.dropped_channels,
            scans = report.stopped_scans,
            "activated"
        );
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(())
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/undo",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 200,
            description = "The workspace as it was before its last change, with the history it \
                           can still walk. The step is stored, so every client is told to reload \
                           it — one workspace, one history, whichever browser pressed undo",
            body = WorkspaceDetail,
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (status = 409, description = "Nothing left to undo", body = ApiError),
    ),
)]
async fn undo_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    step_history(state, id, Store::undo_workspace).await
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/redo",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "The workspace an undo had stepped out of", body = WorkspaceDetail),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (status = 409, description = "Nothing left to redo", body = ApiError),
    ),
)]
async fn redo_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    step_history(state, id, Store::redo_workspace).await
}

async fn step_history(
    state: AppState,
    id: i64,
    step: fn(&Store, i64) -> Result<SteppedWorkspace, StoreError>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    let gps_state = state.clone();
    let detail = tokio::task::spawn_blocking(move || -> Result<WorkspaceDetail, AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.store.workspace(id)?;
        let stepped = step(&state.store, id)?;
        let detail = stepped.detail;
        if state.store.active_workspace_id()? == Some(id) {
            if !before.snapshot.graph.same_topology(&detail.snapshot.graph) {
                let saved = state.store.workspace_state(id)?;
                workspace::reconcile(&state, &detail.snapshot.graph, &saved);
                let report = bring_up(&state, id, &detail.snapshot, &saved)?;
                for refusal in &report.refused {
                    tracing::warn!(
                        workspace = id,
                        node = refusal.node,
                        reason = refusal.reason,
                        "a node could not be restored by the history step"
                    );
                }
            }
            if let Some(settings) = &stepped.settings {
                workspace::restore_settings(&state, &detail.snapshot.graph, settings);
            }
        }
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(detail)
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(Json(detail))
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/apply",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 200,
            description = "The workspace was brought up: radios opened, channels added, and what \
                           could not be satisfied. Additive and idempotent — nothing is closed \
                           or deleted, so calling it twice changes nothing",
            body = PatchApplyReport,
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
async fn apply_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PatchApplyReport>, AppError> {
    let gps_state = state.clone();
    let report = tokio::task::spawn_blocking(move || -> Result<PatchApplyReport, AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workspace = state.store.workspace(id)?;
        let saved = state.store.workspace_state(id)?;
        bring_up(&state, id, &workspace.snapshot, &saved)
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(Json(report))
}

async fn reconcile_gps(state: AppState) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || state.gps.reconcile(&state)).await?;
    Ok(())
}

#[utoipa::path(
    get, path = "/api/patch/catalog",
    responses((
        status = 200,
        description = "The node palette: every node kind this build offers, its category and \
                       its ports. The canvas renders its \"add node\" menu and enforces its \
                       drag-time connection rules from this, so a new node type needs no \
                       frontend table",
        body = PatchCatalog,
    )),
)]
async fn get_patch_catalog() -> Json<PatchCatalog> {
    Json(PatchCatalog::build())
}

#[utoipa::path(
    get, path = "/api/bandplan/regions",
    responses((
        status = 200,
        description = "Selectable band-plan regions and the one to use with no stored \
                       preference. Static: the tables ship with the binary",
        body = BandRegionsResponse,
    )),
)]
async fn list_band_regions() -> Json<BandRegionsResponse> {
    Json(crate::bandplan::regions())
}

#[utoipa::path(
    get, path = "/api/bandplan/regions/{region}",
    params(("region" = String, Path, description = "Region id from /api/bandplan/regions")),
    responses(
        (
            status = 200,
            description = "The region's allocations, already layered most-specific-wins, as one \
                           lane per view: the regulatory stack merged into one, and each amateur \
                           band plan as an overlay. Clipping it to a scope's window and \
                           searching it are client-side arithmetic over this document",
            body = BandPlan,
        ),
        (status = 404, description = "No such region", body = ApiError),
    ),
)]
async fn get_band_plan(Path(region): Path<String>) -> Result<Json<BandPlan>, AppError> {
    crate::bandplan::plan(&region)
        .map(Json)
        .ok_or_else(|| AppError::not_found(format!("no band plan for region {region}")))
}

#[utoipa::path(
    get, path = "/api/bandplan/locate",
    params(LocateQuery),
    responses(
        (
            status = 200,
            description = "The region a coordinate falls in. Coarse by construction — bounding \
                           boxes over the national footprints and an approximation of the ITU \
                           lines — so `approximate` says when only the ITU region could be \
                           decided and the operator should confirm it",
            body = BandRegionMatch,
        ),
        (status = 400, description = "Coordinate out of range", body = ApiError),
    ),
)]
async fn locate_band_region(
    Query(at): Query<LocateQuery>,
) -> Result<Json<BandRegionMatch>, AppError> {
    if !(-90.0..=90.0).contains(&at.lat) || !(-180.0..=180.0).contains(&at.lon) {
        return Err(AppError::bad_request(format!(
            "lat must be -90..90 and lon -180..180, got {}, {}",
            at.lat, at.lon
        )));
    }
    Ok(Json(crate::bandplan::locate(at.lat, at.lon)))
}

#[utoipa::path(
    get, path = "/api/clients",
    responses((
        status = 200,
        description = "WebSocket clients connected right now, including the caller's own \
                       socket. Invalidated by the `clients` scope, never polled",
        body = ClientsResponse,
    )),
)]
async fn get_clients(State(state): State<AppState>) -> Json<ClientsResponse> {
    Json(ClientsResponse {
        clients: state.clients.load(std::sync::atomic::Ordering::Relaxed),
    })
}

#[utoipa::path(
    get, path = "/api/occupancy",
    params(
        ("min_samples" = Option<u64>, Query,
         description = "Drop buckets observed fewer times than this. A duty cycle from three \
                        sightings is not a measurement; the default keeps that out of the report"),
    ),
    responses((
        status = 200,
        description = "How much of the time each slice of spectrum has carried a signal, busiest \
                       first. Accumulated from the spectrum tap of every running receiver against \
                       absolute frequency, so a scan and a retune both add to the same picture",
        body = OccupancyReport,
    )),
)]
async fn get_occupancy(
    State(state): State<AppState>,
    Query(query): Query<OccupancyQuery>,
) -> Json<OccupancyReport> {
    let occupancy = state
        .engine
        .occupancy()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Json(occupancy.report(query.min_samples.unwrap_or(DEFAULT_MIN_SAMPLES)))
}

const DEFAULT_MIN_SAMPLES: u64 = 30;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
struct OccupancyQuery {
    min_samples: Option<u64>,
}

#[utoipa::path(
    get, path = "/api/ionosonde",
    responses((
        status = 200,
        description = "The ionosonde network's current MUF(3000 km) per sounding site, cached \
                       for fifteen minutes — the interval the upstream map is rebuilt on. \
                       A server with no route to the feed answers the same shape with an empty \
                       station list and the reason in `error`, so the propagation map degrades \
                       to what this receiver measured on its own",
        body = IonosondeReport,
    )),
)]
async fn get_ionosonde(State(state): State<AppState>) -> Json<IonosondeReport> {
    Json(state.ionosonde.report().await)
}

#[utoipa::path(
    get, path = "/api/auth",
    responses((
        status = 200,
        description = "Whether this server requires the shared token. Answered without \
                       authentication so a client can prompt before its first real request",
        body = AuthInfo,
    )),
)]
async fn get_auth(State(state): State<AppState>) -> Json<AuthInfo> {
    Json(AuthInfo {
        token_required: state.auth.required(),
    })
}

#[utoipa::path(
    get, path = "/api/doctor",
    responses((
        status = 200,
        description = "Environment diagnostics: compiled backends, devices found, USB \
                       permissions and storage paths (the same report `sdrmm --doctor` prints)",
        body = DoctorReport,
    )),
)]
async fn get_doctor(State(state): State<AppState>) -> Result<Json<DoctorReport>, AppError> {
    let engine = state.engine.clone();
    let db_path = state.db_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::doctor::report(
            engine.registry(),
            db_path.as_deref(),
            engine.recordings_dir(),
        )
    })
    .await?;
    Ok(Json(report))
}

#[utoipa::path(
    get, path = "/api/tools",
    responses((
        status = 200,
        description = "Every tool this build offers. Tools stand beside the receiver: they own \
                       no device set and no channel, and a build without a tool's hardware \
                       support simply does not list it",
        body = ToolsResponse,
    )),
)]
async fn list_tools(State(state): State<AppState>) -> Json<ToolsResponse> {
    Json(ToolsResponse {
        tools: state.tools.descriptors(),
    })
}

#[utoipa::path(
    post, path = "/api/tools/run",
    request_body = ToolRequest,
    responses(
        (
            status = 200,
            description = "The tool's answer, tagged with the same tool id the request carried",
            body = ToolResponse,
        ),
        (status = 400, description = "The tool refused the request", body = ApiError),
        (status = 404, description = "No such tool in this build", body = ApiError),
        (status = 503, description = "The tool's hardware is not attached", body = ApiError),
    ),
)]
async fn run_tool(
    State(state): State<AppState>,
    Json(request): Json<ToolRequest>,
) -> Result<Json<ToolResponse>, AppError> {
    let tools = state.tools.clone();
    let response = tokio::task::spawn_blocking(move || tools.run(request)).await??;
    Ok(Json(response))
}

#[utoipa::path(
    get, path = "/api/about",
    responses((
        status = 200,
        description = "This build, its license, and every third-party component it distributes",
        body = AboutResponse,
    )),
)]
async fn get_about() -> Json<AboutResponse> {
    Json(crate::notices::about())
}

#[utoipa::path(
    get, path = "/api/about/licenses/{id}",
    params(("id" = String, Path, description = "Content id from an attribution's `texts`")),
    responses(
        (status = 200, description = "The full license text", body = LicenseTextResponse),
        (status = 404, description = "No component ships a text with that id", body = ApiError),
    ),
)]
async fn get_license_text(Path(id): Path<String>) -> Result<Json<LicenseTextResponse>, AppError> {
    crate::notices::license_text(&id)
        .map(Json)
        .ok_or_else(|| AppError {
            status: StatusCode::NOT_FOUND,
            body: ApiError {
                error: "unknown license text".to_string(),
                detail: Some(format!("no component ships a license text with id `{id}`")),
            },
        })
}

#[derive(OpenApi)]
#[openapi(
    info(title = "sdr-- API", version = env!("CARGO_PKG_VERSION")),
    components(schemas(
        ServerEvent,
        ClientCommand,
        PresetSnapshot,
        ExportFormat,
        RecordingFormat,
        TemplateInfo,
        ScannerStatus,
    )),
)]
struct ApiDoc;

pub(crate) fn openapi_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(get_state))
        .routes(routes!(get_devices))
        .routes(routes!(get_nmea_devices))
        .routes(routes!(get_channel_types))
        .routes(routes!(list_calls))
        .routes(routes!(call_audio))
        .routes(routes!(list_images))
        .routes(routes!(captured_image))
        .routes(routes!(create_device_set))
        .routes(routes!(delete_device_set))
        .routes(routes!(patch_device))
        .routes(routes!(create_channel))
        .routes(routes!(patch_channel, delete_channel))
        .routes(routes!(list_presets, create_preset))
        .routes(routes!(apply_preset))
        .routes(routes!(delete_preset))
        .routes(routes!(list_bookmarks, create_bookmark))
        .routes(routes!(delete_bookmark))
        .routes(routes!(record_device_set))
        .routes(routes!(record_channel_audio))
        .routes(routes!(record_channel_baseband))
        .routes(routes!(network_export_channel))
        .routes(routes!(time_machine_device_set))
        .routes(routes!(list_audio_recordings))
        .routes(routes!(download_audio_recording))
        .routes(routes!(delete_audio_recording))
        .routes(routes!(network_export_device_set))
        .routes(routes!(control_playback))
        .routes(routes!(list_recordings))
        .routes(routes!(delete_recording))
        .routes(routes!(download_recording))
        .routes(routes!(list_decoder_log, clear_decoder_log))
        .routes(routes!(export_decoder_log))
        .routes(routes!(scan_device_set))
        .routes(routes!(scan_session))
        .routes(routes!(hunt_device_set))
        .routes(routes!(list_templates))
        .routes(routes!(apply_template))
        .routes(routes!(list_workspaces, create_workspace))
        .routes(routes!(get_workspace, update_workspace, delete_workspace))
        .routes(routes!(activate_workspace))
        .routes(routes!(apply_workspace))
        .routes(routes!(undo_workspace))
        .routes(routes!(redo_workspace))
        .routes(routes!(get_patch_catalog))
        .routes(routes!(list_band_regions))
        .routes(routes!(get_band_plan))
        .routes(routes!(locate_band_region))
        .routes(routes!(get_auth))
        .routes(routes!(get_clients))
        .routes(routes!(get_occupancy))
        .routes(routes!(get_ionosonde))
        .routes(routes!(get_doctor))
        .routes(routes!(list_tools))
        .routes(routes!(run_tool))
        .routes(routes!(get_about))
        .routes(routes!(get_license_text))
}
