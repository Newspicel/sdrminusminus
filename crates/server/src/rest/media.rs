use super::*;

#[utoipa::path(
    get, path = "/api/calls",
    responses((status = 200, description = "Completed temporary voice calls", body = VoiceCallsResponse)),
)]
pub(super) async fn list_calls(State(state): State<AppState>) -> Json<VoiceCallsResponse> {
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
pub(super) async fn call_audio(
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
pub(super) async fn list_images(State(state): State<AppState>) -> Json<CapturedImagesResponse> {
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
pub(super) async fn captured_image(
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
