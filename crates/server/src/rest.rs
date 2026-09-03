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
    DeviceInfo, DeviceSettings, DevicesResponse, DfFusionState, DoctorReport, ExportFormat,
    HuntAction, HuntRequest, HuntStatus, IonosondeReport, LicenseTextResponse, LocateQuery,
    NetworkExportAction, NetworkExportRequest, NetworkExportStatus, NmeaDevicesResponse, NodeBody,
    OccupancyReport, PRESET_SNAPSHOT_VERSION, PatchApplyReport, PatchBinding, PatchCatalog,
    PatchRefusal, PlaybackRequest, PlaybackStatus, PresetDevice, PresetInfo, PresetSnapshot,
    RecordAction, RecordRequest, RecordingAnnotation, RecordingDownloadQuery, RecordingFormat,
    RecordingInfo, RecordingStatus, RecordingsResponse, Route, RouteRequest, ScanAction,
    ScanRequest, ScanSessionRequest, ScanSessionStatus, ScannerStatus, ServerEvent, StateScope,
    StateSnapshot, TemplateInfo, TemplatesResponse, TimeMachineAction, TimeMachineRequest,
    TimeMachineStatus, ToolRequest, ToolResponse, ToolsResponse, UpdateWorkspaceRequest,
    VoiceCallsResponse, WorkspaceDetail, WorkspaceExport, WorkspaceInfo, WorkspaceSnapshot,
    WorkspaceState, WorkspacesResponse,
};
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

mod audio_recordings;
mod capture;
mod coherent;
mod cps;
mod decoderlog;
mod devices;
mod info;
mod media;
mod presets;
mod recordings;
mod scanning;
mod workspaces;

use audio_recordings::*;
use capture::*;
use coherent::*;
use cps::*;
use decoderlog::*;
use devices::*;
use info::*;
use media::*;
pub(crate) use media::{call_audio_path, captured_image_path};
use presets::*;
use recordings::*;
use scanning::*;
use workspaces::*;

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
            | StoreError::WorkspaceNotFound(_)
            | StoreError::CpsUserNotFound(_)
            | StoreError::CpsDeviceNotFound(_)
            | StoreError::CpsCodeplugNotFound(_) => StatusCode::NOT_FOUND,
            StoreError::Timestamp(_)
            | StoreError::Sources(_)
            | StoreError::WorkspaceLayout(_)
            | StoreError::CpsField(_) => StatusCode::BAD_REQUEST,
            StoreError::WorkspaceNameTaken(_)
            | StoreError::WorkspaceConflict { .. }
            | StoreError::CpsNameTaken
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
            name: meta.global.name.clone(),
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
            tags: meta.global.tags.clone(),
            note: meta.global.description.clone(),
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
        .routes(routes!(annotate_recording))
        .routes(routes!(download_recording))
        .routes(routes!(list_decoder_log, clear_decoder_log))
        .routes(routes!(export_decoder_log))
        .routes(routes!(scan_device_set))
        .routes(routes!(scan_session))
        .routes(routes!(hunt_device_set))
        .routes(routes!(list_templates))
        .routes(routes!(apply_template))
        .routes(routes!(list_workspaces, create_workspace))
        .routes(routes!(import_workspace))
        .routes(routes!(export_workspace))
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
        .routes(routes!(list_radio_models))
        .routes(routes!(list_cps_ports))
        .routes(routes!(get_cps_library))
        .routes(routes!(create_cps_user))
        .routes(routes!(update_cps_user, delete_cps_user))
        .routes(routes!(create_cps_device))
        .routes(routes!(update_cps_device, delete_cps_device))
        .routes(routes!(create_cps_codeplug))
        .routes(routes!(
            get_cps_codeplug,
            update_cps_codeplug,
            delete_cps_codeplug
        ))
        .routes(routes!(convert_cps_codeplug))
        .routes(routes!(merge_cps_codeplug))
        .routes(routes!(identify_radio))
        .routes(routes!(read_radio))
        .routes(routes!(write_radio))
        .routes(routes!(list_cps_jobs))
        .routes(routes!(get_cps_job, cancel_cps_job))
        .routes(routes!(list_tools))
        .routes(routes!(run_tool))
        .routes(routes!(calibrate_coherent))
        .routes(routes!(get_fusion, reset_fusion))
        .routes(routes!(get_route))
        .routes(routes!(get_about))
        .routes(routes!(get_license_text))
}
