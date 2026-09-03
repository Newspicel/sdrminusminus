use super::*;

#[utoipa::path(
    get, path = "/api/recordings",
    responses((
        status = 200,
        description = "The recording library, reconciled with the SigMF pairs on disk",
        body = RecordingsResponse,
    )),
)]
pub(super) async fn list_recordings(
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
    put, path = "/api/recordings/{id}/annotation",
    params(("id" = i64, Path, description = "Recording id")),
    request_body = RecordingAnnotation,
    responses(
        (
            status = 200,
            description = "The annotated recording. Name, tags and note replace what the \
                           recording carried; all three live in its SigMF metadata, so they \
                           travel with a downloaded archive",
            body = RecordingInfo,
        ),
        (
            status = 400,
            description = "More tags, or a longer tag, name or note than a recording holds",
            body = ApiError,
        ),
        (status = 404, description = "Recording not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn annotate_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(annotation): Json<RecordingAnnotation>,
) -> Result<Json<RecordingInfo>, AppError> {
    let annotation = annotation
        .normalized()
        .map_err(|err| AppError::bad_request(err.to_string()))?;
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    let info = tokio::task::spawn_blocking(move || -> Result<RecordingInfo, AppError> {
        let _gate = lock_gate(&gate);
        let stem = store.recording_stem(id)?;
        let dir = engine
            .recordings_dir()
            .ok_or_else(|| AppError::not_found(format!("recording {id} not found")))?;
        sdrmm_recorder::annotate(
            &dir.join(&stem),
            annotation.name.as_deref(),
            &annotation.tags,
            annotation.note.as_deref(),
        )
        .map_err(|err| annotate_error(id, err))?;
        reconcile_recordings(dir, &store)?;
        let info = store
            .list_recordings(dir)?
            .into_iter()
            .find(|recording| recording.id == id)
            .ok_or_else(|| AppError::not_found(format!("recording {id} not found")))?;
        engine.emit_scope(StateScope::Recordings);
        Ok(info)
    })
    .await??;
    Ok(Json(info))
}

fn annotate_error(id: i64, err: sdrmm_recorder::SigmfError) -> AppError {
    match err {
        sdrmm_recorder::SigmfError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => {
            AppError::not_found(format!("recording {id} not found"))
        }
        other => AppError::internal(format!("annotate recording {id}: {other}")),
    }
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
pub(super) async fn download_recording(
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
        let stem = store.recording_stem(id)?;
        let dir = engine
            .recordings_dir()
            .ok_or_else(|| AppError::not_found(format!("recording {id} not found")))?;
        Export::open(&dir.join(&stem), kind).map_err(|err| export_error(id, err))
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

pub(super) fn export_error(id: i64, err: sdrmm_recorder::SigmfError) -> AppError {
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

pub(super) fn byte_stream(
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
pub(super) async fn delete_recording(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let gate = state.recordings_gate.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _gate = lock_gate(&gate);
        let stem = store.recording_stem(id)?;
        if let Some(dir) = engine.recordings_dir() {
            let pair = dir.join(&stem);
            for path in [meta_path(&pair), data_path(&pair)] {
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
