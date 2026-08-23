use sdrmm_cps::{convert::fit, merge::merge, model, models};
use sdrmm_wire::cps::{
    CpsCodeplugDetail, CpsCodeplugRequest, CpsConvertRequest, CpsConvertResponse, CpsDevice,
    CpsDeviceRequest, CpsIdentifyRequest, CpsJob, CpsJobsResponse, CpsLibraryResponse,
    CpsMergeRequest, CpsPortsResponse, CpsReadRequest, CpsUser, CpsUserRequest, CpsWriteRequest,
    RadioIdent, RadioModelsResponse,
};

use super::*;
use crate::cps::CpsJobError;

impl From<CpsJobError> for AppError {
    fn from(err: CpsJobError) -> Self {
        match err {
            CpsJobError::Store(error) => error.into(),
            CpsJobError::NotFound(_) => Self::not_found(err.to_string()),
            CpsJobError::Busy | CpsJobError::Finished(_) => Self {
                status: StatusCode::CONFLICT,
                body: ApiError {
                    error: err.to_string(),
                    detail: None,
                },
            },
            CpsJobError::Unconfirmed => Self::bad_request(err.to_string()),
            CpsJobError::NoStoredImage(_) => Self::bad_request(err.to_string()),
            CpsJobError::Radio(error) if error.is_not_found() => Self::not_found(error.to_string()),
            CpsJobError::Radio(error) => Self {
                status: StatusCode::BAD_GATEWAY,
                body: ApiError {
                    error: error.to_string(),
                    detail: None,
                },
            },
        }
    }
}

fn unknown_model(id: &str) -> AppError {
    AppError::not_found(format!("no radio model with id {id}"))
}

#[utoipa::path(
    get, path = "/api/cps/models",
    responses((status = 200, description = "Radio models this build can program", body = RadioModelsResponse)),
)]
pub(super) async fn list_radio_models() -> Json<RadioModelsResponse> {
    Json(RadioModelsResponse {
        models: models().descriptors(),
    })
}

#[utoipa::path(
    get, path = "/api/cps/ports",
    responses((status = 200, description = "Serial ports a radio could be on", body = CpsPortsResponse)),
)]
pub(super) async fn list_cps_ports(
    State(state): State<AppState>,
) -> Result<Json<CpsPortsResponse>, AppError> {
    let hub = state.cps.clone();
    let (ports, ignored_ports) = tokio::task::spawn_blocking(move || hub.ports()).await??;
    Ok(Json(CpsPortsResponse {
        ports,
        ignored_ports,
    }))
}

#[utoipa::path(
    get, path = "/api/cps/library",
    responses((status = 200, description = "Stored operators, radios and codeplugs", body = CpsLibraryResponse)),
)]
pub(super) async fn get_cps_library(
    State(state): State<AppState>,
) -> Result<Json<CpsLibraryResponse>, AppError> {
    let store = state.store.clone();
    let library = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        Ok(CpsLibraryResponse {
            users: store.list_cps_users()?,
            devices: store.list_cps_devices()?,
            codeplugs: store.list_cps_codeplugs()?,
        })
    })
    .await??;
    Ok(Json(library))
}

#[utoipa::path(
    post, path = "/api/cps/users",
    request_body = CpsUserRequest,
    responses(
        (status = 200, description = "Operator stored", body = CreatedRowId),
        (status = 400, description = "Unusable field", body = ApiError),
        (status = 409, description = "That name is taken", body = ApiError),
    ),
)]
pub(super) async fn create_cps_user(
    State(state): State<AppState>,
    Json(req): Json<CpsUserRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || store.create_cps_user(&req)).await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    patch, path = "/api/cps/users/{id}",
    params(("id" = i64, Path, description = "Operator id")),
    request_body = CpsUserRequest,
    responses(
        (status = 200, description = "Operator updated", body = CpsUser),
        (status = 404, description = "Operator not found", body = ApiError),
    ),
)]
pub(super) async fn update_cps_user(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CpsUserRequest>,
) -> Result<Json<CpsUser>, AppError> {
    let store = state.store.clone();
    let user = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        store.update_cps_user(id, &req)?;
        Ok(store.cps_user(id)?)
    })
    .await??;
    Ok(Json(user))
}

#[utoipa::path(
    delete, path = "/api/cps/users/{id}",
    params(("id" = i64, Path, description = "Operator id")),
    responses(
        (status = 204, description = "Operator removed"),
        (status = 404, description = "Operator not found", body = ApiError),
    ),
)]
pub(super) async fn delete_cps_user(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.delete_cps_user(id)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/cps/devices",
    request_body = CpsDeviceRequest,
    responses(
        (status = 200, description = "Radio stored", body = CreatedRowId),
        (status = 400, description = "Unusable field", body = ApiError),
        (status = 404, description = "No such radio model", body = ApiError),
        (status = 409, description = "That name is taken", body = ApiError),
    ),
)]
pub(super) async fn create_cps_device(
    State(state): State<AppState>,
    Json(req): Json<CpsDeviceRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    if model(&req.model_id).is_none() {
        return Err(unknown_model(&req.model_id));
    }
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || store.create_cps_device(&req)).await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    patch, path = "/api/cps/devices/{id}",
    params(("id" = i64, Path, description = "Radio id")),
    request_body = CpsDeviceRequest,
    responses(
        (status = 200, description = "Radio updated", body = CpsDevice),
        (status = 404, description = "Radio not found", body = ApiError),
    ),
)]
pub(super) async fn update_cps_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CpsDeviceRequest>,
) -> Result<Json<CpsDevice>, AppError> {
    if model(&req.model_id).is_none() {
        return Err(unknown_model(&req.model_id));
    }
    let store = state.store.clone();
    let device = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        store.update_cps_device(id, &req)?;
        Ok(store.cps_device(id)?)
    })
    .await??;
    Ok(Json(device))
}

#[utoipa::path(
    delete, path = "/api/cps/devices/{id}",
    params(("id" = i64, Path, description = "Radio id")),
    responses(
        (status = 204, description = "Radio removed"),
        (status = 404, description = "Radio not found", body = ApiError),
    ),
)]
pub(super) async fn delete_cps_device(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.delete_cps_device(id)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/cps/codeplugs",
    request_body = CpsCodeplugRequest,
    responses(
        (status = 200, description = "Codeplug stored", body = CreatedRowId),
        (status = 404, description = "No such radio model", body = ApiError),
    ),
)]
pub(super) async fn create_cps_codeplug(
    State(state): State<AppState>,
    Json(req): Json<CpsCodeplugRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    if model(&req.model_id).is_none() {
        return Err(unknown_model(&req.model_id));
    }
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || store.store_cps_codeplug(&req, None)).await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    get, path = "/api/cps/codeplugs/{id}",
    params(("id" = i64, Path, description = "Codeplug id")),
    responses(
        (status = 200, description = "The stored codeplug", body = CpsCodeplugDetail),
        (status = 404, description = "Codeplug not found", body = ApiError),
    ),
)]
pub(super) async fn get_cps_codeplug(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CpsCodeplugDetail>, AppError> {
    let store = state.store.clone();
    let detail = tokio::task::spawn_blocking(move || store.cps_codeplug(id)).await??;
    Ok(Json(detail))
}

#[utoipa::path(
    patch, path = "/api/cps/codeplugs/{id}",
    params(("id" = i64, Path, description = "Codeplug id")),
    request_body = CpsCodeplugRequest,
    responses(
        (status = 200, description = "Codeplug updated", body = CpsCodeplugDetail),
        (status = 404, description = "Codeplug not found", body = ApiError),
    ),
)]
pub(super) async fn update_cps_codeplug(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CpsCodeplugRequest>,
) -> Result<Json<CpsCodeplugDetail>, AppError> {
    if model(&req.model_id).is_none() {
        return Err(unknown_model(&req.model_id));
    }
    let store = state.store.clone();
    let detail = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        store.update_cps_codeplug(id, &req)?;
        Ok(store.cps_codeplug(id)?)
    })
    .await??;
    Ok(Json(detail))
}

#[utoipa::path(
    delete, path = "/api/cps/codeplugs/{id}",
    params(("id" = i64, Path, description = "Codeplug id")),
    responses(
        (status = 204, description = "Codeplug removed"),
        (status = 404, description = "Codeplug not found", body = ApiError),
    ),
)]
pub(super) async fn delete_cps_codeplug(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || store.delete_cps_codeplug(id)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/cps/codeplugs/{id}/convert",
    params(("id" = i64, Path, description = "Codeplug id")),
    request_body = CpsConvertRequest,
    responses(
        (status = 200, description = "What the target radio can hold, and what it cannot", body = CpsConvertResponse),
        (status = 404, description = "Codeplug or model not found", body = ApiError),
    ),
)]
pub(super) async fn convert_cps_codeplug(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CpsConvertRequest>,
) -> Result<Json<CpsConvertResponse>, AppError> {
    let Some(target) = model(&req.target_model_id) else {
        return Err(unknown_model(&req.target_model_id));
    };
    let store = state.store.clone();
    let response = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        let source = store.cps_codeplug(id)?;
        let mut codeplug = source.codeplug;
        if let Some(user_id) = req.user_id {
            crate::cps::apply_user(&mut codeplug, &store.cps_user(user_id)?);
        }
        let (fitted, report) = fit(&codeplug, &req.target_model_id, &target.limits());
        let stored_id = if req.store {
            Some(store.store_cps_codeplug(
                &CpsCodeplugRequest {
                    name: req.name.clone().unwrap_or_else(|| {
                        format!("{} → {}", source.info.name, target.descriptor().model)
                    }),
                    model_id: req.target_model_id.clone(),
                    device_id: req.device_id,
                    user_id: req.user_id,
                    codeplug: fitted.clone(),
                },
                None,
            )?)
        } else {
            None
        };
        Ok(CpsConvertResponse {
            report,
            codeplug: fitted,
            stored_id,
        })
    })
    .await??;
    Ok(Json(response))
}

#[utoipa::path(
    post, path = "/api/cps/codeplugs/{id}/merge",
    params(("id" = i64, Path, description = "Codeplug the entries land in")),
    request_body = CpsMergeRequest,
    responses(
        (status = 200, description = "The merged codeplug", body = CpsConvertResponse),
        (status = 404, description = "Either codeplug was not found", body = ApiError),
    ),
)]
pub(super) async fn merge_cps_codeplug(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<CpsMergeRequest>,
) -> Result<Json<CpsConvertResponse>, AppError> {
    let store = state.store.clone();
    let response = tokio::task::spawn_blocking(move || -> Result<_, AppError> {
        let target = store.cps_codeplug(id)?;
        let source = store.cps_codeplug(req.source_id)?;
        let (merged, notes) = merge(&target.codeplug, &source.codeplug, req.mode, &req.parts);
        let Some(radio) = model(&target.info.model_id) else {
            return Err(unknown_model(&target.info.model_id));
        };
        let (fitted, mut report) = fit(&merged, &target.info.model_id, &radio.limits());
        report.issues.extend(notes);
        report.source_model = Some(source.info.model_id.clone());
        store.update_cps_codeplug(
            id,
            &CpsCodeplugRequest {
                name: target.info.name.clone(),
                model_id: target.info.model_id.clone(),
                device_id: target.info.device_id,
                user_id: target.info.user_id,
                codeplug: fitted.clone(),
            },
        )?;
        Ok(CpsConvertResponse {
            report,
            codeplug: fitted,
            stored_id: Some(id),
        })
    })
    .await??;
    Ok(Json(response))
}

#[utoipa::path(
    post, path = "/api/cps/identify",
    request_body = CpsIdentifyRequest,
    responses(
        (status = 200, description = "What the radio on that port says it is", body = RadioIdent),
        (status = 404, description = "No such radio model", body = ApiError),
        (status = 409, description = "Another transfer holds the port", body = ApiError),
        (status = 502, description = "The radio did not answer", body = ApiError),
    ),
)]
pub(super) async fn identify_radio(
    State(state): State<AppState>,
    Json(req): Json<CpsIdentifyRequest>,
) -> Result<Json<RadioIdent>, AppError> {
    let hub = state.cps.clone();
    let ident =
        tokio::task::spawn_blocking(move || hub.identify(&req.model_id, &req.port)).await??;
    Ok(Json(ident))
}

#[utoipa::path(
    post, path = "/api/cps/read",
    request_body = CpsReadRequest,
    responses(
        (status = 200, description = "The read is running; poll the job", body = CpsJob),
        (status = 404, description = "No such radio model", body = ApiError),
        (status = 409, description = "Another transfer holds the port", body = ApiError),
    ),
)]
pub(super) async fn read_radio(
    State(state): State<AppState>,
    Json(req): Json<CpsReadRequest>,
) -> Result<Json<CpsJob>, AppError> {
    let hub = state.cps.clone();
    let store = state.store.clone();
    let name = req
        .name
        .clone()
        .unwrap_or_else(|| format!("Read from {}", req.model_id));
    let job = hub.read(
        &store,
        &req.model_id,
        &req.port,
        &name,
        req.device_id,
        req.user_id,
    )?;
    Ok(Json(job))
}

#[utoipa::path(
    post, path = "/api/cps/write",
    request_body = CpsWriteRequest,
    responses(
        (status = 200, description = "The write is running; poll the job", body = CpsJob),
        (status = 400, description = "`confirm` was not set", body = ApiError),
        (status = 404, description = "Model or codeplug not found", body = ApiError),
        (status = 409, description = "Another transfer holds the port", body = ApiError),
    ),
)]
pub(super) async fn write_radio(
    State(state): State<AppState>,
    Json(req): Json<CpsWriteRequest>,
) -> Result<Json<CpsJob>, AppError> {
    let hub = state.cps.clone();
    let store = state.store.clone();
    let user = match req.user_id {
        Some(id) => Some(store.cps_user(id)?),
        None => None,
    };
    let job = hub.write(
        &store,
        &req.model_id,
        &req.port,
        req.codeplug_id,
        user,
        req.device_id,
        req.confirm,
        req.restore_image,
    )?;
    Ok(Json(job))
}

#[utoipa::path(
    get, path = "/api/cps/jobs",
    responses((status = 200, description = "Radio transfers this session ran", body = CpsJobsResponse)),
)]
pub(super) async fn list_cps_jobs(State(state): State<AppState>) -> Json<CpsJobsResponse> {
    Json(CpsJobsResponse {
        jobs: state.cps.jobs(),
    })
}

#[utoipa::path(
    get, path = "/api/cps/jobs/{id}",
    params(("id" = u64, Path, description = "Job id")),
    responses(
        (status = 200, description = "How the transfer is going", body = CpsJob),
        (status = 404, description = "Job not found", body = ApiError),
    ),
)]
pub(super) async fn get_cps_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<CpsJob>, AppError> {
    Ok(Json(state.cps.job(id)?))
}

#[utoipa::path(
    delete, path = "/api/cps/jobs/{id}",
    params(("id" = u64, Path, description = "Job id")),
    responses(
        (status = 200, description = "The transfer was asked to stop", body = CpsJob),
        (status = 404, description = "Job not found", body = ApiError),
        (status = 409, description = "The transfer already finished", body = ApiError),
    ),
)]
pub(super) async fn cancel_cps_job(
    State(state): State<AppState>,
    Path(id): Path<u64>,
) -> Result<Json<CpsJob>, AppError> {
    Ok(Json(state.cps.cancel(id)?))
}
