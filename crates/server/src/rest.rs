//! REST handlers (PLAN §5). Each is a `#[utoipa::path]` so the OpenAPI — and therefore the
//! generated TypeScript client — is produced from these signatures, never hand-written.

use axum::{
    extract::{
        FromRequest, FromRequestParts, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use sdrmm_engine::EngineError;
use sdrmm_recorder::{SigmfMeta, SigmfReader, data_path, meta_path, scan_stems};
use sdrmm_wire::{
    ApiError, ApplyPresetRequest, Bookmark, ChannelSettings, ChannelTypesResponse, ClientCommand,
    CreateBookmarkRequest, CreateChannelRequest, CreateDeviceSetRequest, CreatePresetRequest,
    CreatedId, CreatedRowId, DeviceSettings, DevicesResponse, PresetInfo, PresetSnapshot,
    RecordAction, RecordRequest, RecordingStatus, RecordingsResponse, ServerEvent, StateScope,
    StateSnapshot,
};
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppState,
    store::{RecordingRow, Store, StoreError},
};

/// The `PresetSnapshot` schema version this build writes and applies.
pub(crate) const PRESET_VERSION: u32 = 1;

/// Typed REST error → `(status, ApiError)` (PLAN §5). Declaring these in each path's responses
/// is what gives the generated client a typed `error` branch.
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

/// `axum::Json` with its rejection remapped onto the [`ApiError`] contract: a malformed body
/// must produce the same JSON error shape the documented 4xx responses promise, never axum's
/// plain-text default.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(AppError))]
pub(crate) struct Json<T>(pub T);

impl<T: serde::Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

/// `axum::extract::Path` with the rejection remapped like [`Json`].
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(AppError))]
pub(crate) struct Path<T>(pub T);

/// Keeps axum's status split (400 syntax / 415 content type / 422 schema mismatch); only the
/// body shape changes.
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

impl From<EngineError> for AppError {
    fn from(err: EngineError) -> Self {
        let status = if err.is_not_found() {
            StatusCode::NOT_FOUND
        } else if err.is_bad_request() {
            StatusCode::BAD_REQUEST
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
            | StoreError::RecordingNotFound(_) => StatusCode::NOT_FOUND,
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

/// A panicked/cancelled `spawn_blocking` task. Handlers below run every engine call that
/// reaches real hardware (USB I/O, thread joins) and every store call (SQLite blocks) on the
/// blocking pool so a slow device never stalls the tokio workers; only the pure in-memory
/// reads (`get_state`, `get_channel_types`) stay direct.
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
    post, path = "/api/devicesets",
    request_body = CreateDeviceSetRequest,
    responses(
        (status = 200, description = "Device set created", body = CreatedId),
        (status = 400, description = "Unknown or unusable device", body = ApiError),
        (status = 404, description = "Device not found", body = ApiError),
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
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.patch_device(ds, settings)).await??;
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
    let id = tokio::task::spawn_blocking(move || engine.add_channel(ds, req.settings)).await??;
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
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.patch_channel(ds, ch, settings)).await??;
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
        (status = 404, description = "Device set not found", body = ApiError),
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
        let snap = engine.snapshot();
        let set = snap
            .device_sets
            .iter()
            .find(|s| s.id == req.device_set)
            .ok_or(EngineError::DeviceSetNotFound(req.device_set))?;
        let snapshot = PresetSnapshot {
            version: PRESET_VERSION,
            device_id: set.device.id(),
            settings: set.settings.clone(),
            channels: set.channels.iter().map(|c| c.settings.clone()).collect(),
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
    request_body = ApplyPresetRequest,
    responses(
        (status = 204, description = "Preset applied"),
        (
            status = 400,
            description = "Preset rejected by the target device; `detail` reports what state \
                           a partial application left behind",
            body = ApiError,
        ),
        (status = 404, description = "Preset or device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
async fn apply_preset(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<ApplyPresetRequest>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let snapshot = store.preset_snapshot(id)?;
        if snapshot.version != PRESET_VERSION {
            return Err(AppError::bad_request(format!(
                "preset {id} has unsupported snapshot version {}",
                snapshot.version
            )));
        }
        let ds = req.device_set;
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
        // Applying to different hardware than the preset was taken from is allowed on
        // purpose (a bookmarkable configuration, not a device binding); the engine rejects
        // loudly whatever the device can't do.
        //
        // Remove the existing channels FIRST: patch_device validates a new sample rate
        // against the currently hosted channels, which would wrongly veto a valid preset
        // on behalf of channels this apply is about to delete anyway. The three engine
        // calls cannot be atomic (each does real device I/O), so a mid-sequence failure
        // reports exactly what was left behind in `detail`; the engine's own StateChanged
        // events keep clients converged on that state.
        let total_existing = existing.len();
        for (done, ch) in existing.into_iter().enumerate() {
            engine.remove_channel(ds, ch).map_err(|e| {
                AppError::from(e).with_detail(format!(
                    "preset partially applied: removed {done} of {total_existing} existing \
                     channels, device settings untouched, no preset channels added"
                ))
            })?;
        }
        engine.patch_device(ds, snapshot.settings).map_err(|e| {
            AppError::from(e).with_detail(format!(
                "preset partially applied: all {total_existing} existing channels removed, \
                 device settings untouched, no preset channels added"
            ))
        })?;
        let total_new = snapshot.channels.len();
        for (done, settings) in snapshot.channels.into_iter().enumerate() {
            engine.add_channel(ds, settings).map_err(|e| {
                AppError::from(e).with_detail(format!(
                    "preset partially applied: existing channels removed, device settings \
                     applied, {done} of {total_new} preset channels added"
                ))
            })?;
        }
        Ok(())
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
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
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let status = tokio::task::spawn_blocking(move || -> Result<RecordingStatus, AppError> {
        match req.action {
            RecordAction::Start => {
                engine.start_recording(ds)?;
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
                // Final counts (and any truncation fault) go back in-band; the indexed row
                // reaches clients through the Recordings scope + `GET /api/recordings`, which
                // also covers a pair that faulted before finalizing and thus has no row.
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
    Ok(Json(status))
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
        // Files before row: if removal fails the recording stays listed and the delete can
        // be retried. A set replaying the pair faults via the probe-vanish path — accepted,
        // honest (PLAN §16 M2 hotplug contract).
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

fn lock_gate(gate: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
    gate.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Disk → index reconciliation (PLAN §11: the files are the source of truth, the database is
/// an index): upsert a row per finalized pair, prune rows whose pair vanished. Pairs that
/// cannot be read (foreign datatype, torn meta, no sample rate) are skipped — and therefore
/// delisted — since they cannot be played either. Callers hold the recordings gate.
fn reconcile_recordings(dir: &std::path::Path, store: &Store) -> Result<(), AppError> {
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
        // Sample count from the data file, not the meta: a crash-truncated pair is honest
        // about what is actually replayable.
        let samples = reader.total_samples();
        let meta = reader.meta();
        // `core:sample_rate` is optional in SigMF, but playback (and duration) need one.
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

/// Foreign SigMF files may omit `core:datetime`; fall back to the data file's mtime so the
/// row still carries a usable timestamp.
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

/// OpenAPI metadata plus the schemas no path references — the WS message enums and the stored
/// preset blob — which must be force-registered as components (PLAN §4) to appear in the
/// generated TypeScript.
#[derive(OpenApi)]
#[openapi(
    info(title = "sdr-- API", version = env!("CARGO_PKG_VERSION")),
    components(schemas(ServerEvent, ClientCommand, PresetSnapshot)),
)]
struct ApiDoc;

/// The REST surface as a utoipa-axum router; `split_for_parts` yields the axum `Router` and the
/// merged `OpenApi` (PLAN §4: same service layer feeds REST and the spec).
pub(crate) fn openapi_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(get_state))
        .routes(routes!(get_devices))
        .routes(routes!(get_channel_types))
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
        .routes(routes!(list_recordings))
        .routes(routes!(delete_recording))
}
