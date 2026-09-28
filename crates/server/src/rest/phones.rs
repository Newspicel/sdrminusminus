use sdrmm_wire::{
    CreateOfferRequest, PairRequest, PairResponse, PairingOffer, Phone, PhoneAccess,
    PhoneAccessStatus, PhoneSelf, PhonesResponse, RenamePhoneRequest,
};

use super::*;

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_phones))
        .routes(routes!(set_phone_access))
        .routes(routes!(create_pairing_offer, cancel_pairing_offer))
        .routes(routes!(pair_phone))
        .routes(routes!(get_phone_self, unpair_phone_self))
        .routes(routes!(rename_phone, revoke_phone))
}

#[utoipa::path(
    get, path = "/api/phones",
    responses((status = 200, description = "Paired phones, the live offer and phone access", body = PhonesResponse)),
)]
pub(super) async fn list_phones(
    State(_state): State<AppState>,
) -> Result<Json<PhonesResponse>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    put, path = "/api/phones/access",
    request_body = PhoneAccess,
    responses(
        (status = 200, description = "The phone listener as it now runs", body = PhoneAccessStatus),
        (status = 400, description = "Port 0 or the main port", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn set_phone_access(
    State(_state): State<AppState>,
    Json(_access): Json<PhoneAccess>,
) -> Result<Json<PhoneAccessStatus>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    post, path = "/api/phones/offers",
    request_body = CreateOfferRequest,
    responses(
        (status = 201, description = "A one-time pairing code", body = PairingOffer),
        (status = 400, description = "Name not valid", body = ApiError),
        (status = 409, description = "Turn on Allow phones", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_pairing_offer(
    State(_state): State<AppState>,
    Json(_request): Json<CreateOfferRequest>,
) -> Result<(StatusCode, Json<PairingOffer>), AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    delete, path = "/api/phones/offers",
    responses(
        (status = 204, description = "The offer is gone"),
        (status = 404, description = "No live offer", body = ApiError),
    ),
)]
pub(super) async fn cancel_pairing_offer(
    State(_state): State<AppState>,
) -> Result<StatusCode, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    post, path = "/api/phones/pair",
    request_body = PairRequest,
    responses(
        (status = 200, description = "The phone is paired", body = PairResponse),
        (status = 400, description = "Name not valid", body = ApiError),
        (status = 401, description = "Wrong code", body = ApiError),
        (status = 403, description = "Pair over HTTPS", body = ApiError),
        (status = 404, description = "No live offer or the code expired", body = ApiError),
        (status = 409, description = "Offer burned or protocol needed", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn pair_phone(
    State(_state): State<AppState>,
    Json(_request): Json<PairRequest>,
) -> Result<Json<PairResponse>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    get, path = "/api/phones/self",
    responses(
        (status = 200, description = "The calling phone", body = PhoneSelf),
        (status = 401, description = "Phone not paired", body = ApiError),
    ),
)]
pub(super) async fn get_phone_self(
    State(_state): State<AppState>,
) -> Result<Json<PhoneSelf>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    delete, path = "/api/phones/self",
    responses(
        (status = 204, description = "The calling phone is unpaired"),
        (status = 401, description = "Phone not paired", body = ApiError),
    ),
)]
pub(super) async fn unpair_phone_self(
    State(_state): State<AppState>,
) -> Result<StatusCode, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    patch, path = "/api/phones/{id}",
    params(("id" = String, Path, description = "Phone id")),
    request_body = RenamePhoneRequest,
    responses(
        (status = 200, description = "The renamed phone", body = Phone),
        (status = 400, description = "Name not valid", body = ApiError),
        (status = 404, description = "No phone with that id", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn rename_phone(
    State(_state): State<AppState>,
    Path(_id): Path<String>,
    Json(_request): Json<RenamePhoneRequest>,
) -> Result<Json<Phone>, AppError> {
    Err(AppError::not_built())
}

#[utoipa::path(
    delete, path = "/api/phones/{id}",
    params(("id" = String, Path, description = "Phone id")),
    responses(
        (status = 204, description = "The phone is revoked and its sockets close"),
        (status = 404, description = "No phone with that id", body = ApiError),
    ),
)]
pub(super) async fn revoke_phone(
    State(_state): State<AppState>,
    Path(_id): Path<String>,
) -> Result<StatusCode, AppError> {
    Err(AppError::not_built())
}
