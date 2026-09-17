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

#[utoipa::path(
    get, path = "/api/audiorecordings/{file}",
    params(
        ("file" = String, Path, description = "Audio recording file name, extension included"),
        ("Range" = Option<String>, Header, description = "A `bytes=` window of the file"),
    ),
    responses(
        (
            status = 200,
            description = "The recording as a WAV to play where it is asked for, rather than a \
                           copy to save",
            content((String = "audio/wav")),
        ),
        (
            status = 206,
            description = "The requested window of the file, which is what a media element asks \
                           for when it seeks",
            content((String = "audio/wav")),
        ),
        (status = 404, description = "Audio recording not found", body = ApiError),
        (status = 416, description = "The requested window is past the end of the file"),
    ),
)]
pub(super) async fn play_audio_recording(
    State(state): State<AppState>,
    Path(file): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<Response, AppError> {
    let name = file.clone();
    let (mut handle, len) =
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

    let asked = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let window = byte_window(asked, len);
    if window == Window::Unsatisfiable {
        return Ok((
            StatusCode::RANGE_NOT_SATISFIABLE,
            [(header::CONTENT_RANGE, format!("bytes */{len}"))],
        )
            .into_response());
    }

    let (status, span) = match window {
        Window::Part(first, last) => {
            std::io::Seek::seek(&mut handle, std::io::SeekFrom::Start(first))
                .map_err(|err| AppError::internal(format!("seek {name}: {err}")))?;
            (StatusCode::PARTIAL_CONTENT, last - first + 1)
        }
        _ => (StatusCode::OK, len),
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "audio/wav")
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{name}\""),
        )
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, span);
    if let Window::Part(first, last) = window {
        response = response.header(header::CONTENT_RANGE, format!("bytes {first}-{last}/{len}"));
    }
    response
        .body(Body::from_stream(byte_stream(std::io::Read::take(
            handle, span,
        ))))
        .map_err(|err| AppError::internal(format!("serve {name}: {err}")))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Window {
    Whole,
    Part(u64, u64),
    Unsatisfiable,
}

fn byte_window(header: Option<&str>, len: u64) -> Window {
    let Some(spec) = header.map(str::trim).and_then(|h| h.strip_prefix("bytes=")) else {
        return Window::Whole;
    };
    let spec = spec.trim();
    if spec.contains(',') {
        return Window::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Window::Whole;
    };
    let Some(end) = len.checked_sub(1) else {
        return Window::Unsatisfiable;
    };
    match (first.trim(), last.trim()) {
        ("", "") => Window::Whole,
        ("", suffix) => match suffix.parse::<u64>() {
            Ok(0) => Window::Unsatisfiable,
            Ok(want) => Window::Part(len.saturating_sub(want), end),
            Err(_) => Window::Whole,
        },
        (start, "") => match start.parse::<u64>() {
            Ok(start) if start <= end => Window::Part(start, end),
            Ok(_) => Window::Unsatisfiable,
            Err(_) => Window::Whole,
        },
        (start, stop) => match (start.parse::<u64>(), stop.parse::<u64>()) {
            (Ok(start), Ok(stop)) if start > stop => Window::Whole,
            (Ok(start), Ok(stop)) if start <= end => Window::Part(start, stop.min(end)),
            (Ok(_), Ok(_)) => Window::Unsatisfiable,
            _ => Window::Whole,
        },
    }
}

#[utoipa::path(
    post, path = "/api/audiorecordings/{file}/reveal",
    params(("file" = String, Path, description = "Audio recording file name, extension included")),
    responses(
        (status = 204, description = "The file is selected in the machine's file manager"),
        (
            status = 404,
            description = "Audio recording not found, or this server has no file manager",
            body = ApiError,
        ),
    ),
)]
pub(super) async fn reveal_audio_recording(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<StatusCode, AppError> {
    tokio::task::spawn_blocking(move || -> Result<StatusCode, AppError> {
        let path = audio_recording_path(&state, &file)?;
        reveal_path(&state, &path)
    })
    .await?
}
