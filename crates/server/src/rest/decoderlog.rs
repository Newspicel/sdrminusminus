use super::*;

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
pub(super) async fn list_decoder_log(
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
pub(super) async fn clear_decoder_log(
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
pub(super) async fn export_decoder_log(
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

pub(super) fn csv_export(entries: &[DecoderLogEntry]) -> Result<String, serde_json::Error> {
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

pub(super) fn csv_field(value: &str) -> String {
    if value.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}
