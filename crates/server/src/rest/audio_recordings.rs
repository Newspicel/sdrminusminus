use super::*;

#[utoipa::path(
    get, path = "/api/audiorecordings",
    responses((
        status = 200,
        description = "The audio-recording library, read off the files themselves",
        body = AudioRecordingsResponse,
    )),
)]
pub(super) async fn list_audio_recordings(
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

pub(super) fn audio_info(path: &std::path::Path) -> Option<AudioRecordingInfo> {
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

pub(super) fn file_created_at(path: &std::path::Path) -> String {
    let at = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| jiff::Timestamp::from_second(since.as_secs() as i64).ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    at.to_string()
}

pub(super) fn audio_recording_path(
    state: &AppState,
    file: &str,
) -> Result<std::path::PathBuf, AppError> {
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
pub(super) async fn download_audio_recording(
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
pub(super) async fn delete_audio_recording(
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
