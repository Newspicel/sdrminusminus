use std::time::Duration;

use axum::Extension;
use jiff::Timestamp;
use sdrmm_wire::{
    CreateOfferRequest, PairRequest, PairResponse, PairingOffer, Phone, PhoneAccess,
    PhoneAccessStatus, PhoneSelf, PhonesResponse, RenamePhoneRequest, phone::PAIR_FAILURE_DELAY_MS,
};

use super::*;
use crate::{auth::Identity, phones::PairError};

const NO_OFFER: &str = "No pairing code is open";

pub(super) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(list_phones))
        .routes(routes!(set_phone_access))
        .routes(routes!(create_pairing_offer, cancel_pairing_offer))
        .routes(routes!(pair_phone))
        .routes(routes!(get_phone_self, unpair_phone_self))
        .routes(routes!(rename_phone, revoke_phone))
}

fn not_allowed() -> AppError {
    AppError::new(
        StatusCode::FORBIDDEN,
        ErrorCode::Auth,
        "Not allowed".to_owned(),
    )
}

fn operator(identity: &Identity) -> Result<(), AppError> {
    if identity.administers() {
        Ok(())
    } else {
        Err(not_allowed())
    }
}

impl From<PairError> for AppError {
    fn from(error: PairError) -> Self {
        if let PairError::Store(store) = error {
            return store.into();
        }
        let (status, code) = match &error {
            PairError::Store(_) => (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Storage),
            PairError::NoOffer | PairError::Expired => (StatusCode::NOT_FOUND, ErrorCode::NotFound),
            PairError::WrongCode { .. } => (StatusCode::UNAUTHORIZED, ErrorCode::Auth),
            PairError::Burned | PairError::Protocol { .. } | PairError::NoEndpoint => {
                (StatusCode::CONFLICT, ErrorCode::Conflict)
            }
            PairError::Name => (StatusCode::BAD_REQUEST, ErrorCode::Request),
            PairError::Random(_) => (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal),
        };
        Self::new(status, code, error.to_string())
    }
}

fn active_graph(state: &AppState) -> Result<Option<PatchGraph>, StoreError> {
    Ok(state
        .store
        .active_workspace()?
        .map(|active| active.snapshot.graph))
}

#[utoipa::path(
    get, path = "/api/phones",
    responses(
        (status = 200, description = "Paired phones, the live offer and phone access", body = PhonesResponse),
        (status = 403, description = "Not allowed", body = ApiError),
    ),
)]
pub(super) async fn list_phones(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<PhonesResponse>, AppError> {
    operator(&identity)?;
    let access = state.gate.status();
    let endpoint = access.endpoint.clone();
    let app = state.clone();
    let (phones, offer) = tokio::task::spawn_blocking(move || -> Result<_, StoreError> {
        let graph = active_graph(&app)?;
        let phones = app.phones.list(graph.as_ref())?;
        let offer =
            app.phones
                .offer_status(endpoint.as_ref(), &app.server_name, Timestamp::now())?;
        Ok((phones, offer))
    })
    .await??;
    Ok(Json(PhonesResponse {
        phones,
        offer,
        access,
    }))
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
        (status = 403, description = "Not allowed", body = ApiError),
        (status = 409, description = "Turn on Allow phones", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_pairing_offer(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Json(request): Json<CreateOfferRequest>,
) -> Result<(StatusCode, Json<PairingOffer>), AppError> {
    operator(&identity)?;
    let endpoint = state.gate.endpoint().ok_or(PairError::NoEndpoint)?;
    let app = state.clone();
    let offer = tokio::task::spawn_blocking(move || {
        app.phones.create_offer(
            request.name.as_deref(),
            &endpoint,
            &app.server_name,
            Timestamp::now(),
        )
    })
    .await??;
    state.engine.emit_scope(StateScope::Phones);
    Ok((StatusCode::CREATED, Json(offer)))
}

#[utoipa::path(
    delete, path = "/api/phones/offers",
    responses(
        (status = 204, description = "The offer is gone"),
        (status = 403, description = "Not allowed", body = ApiError),
        (status = 404, description = "No live offer", body = ApiError),
    ),
)]
pub(super) async fn cancel_pairing_offer(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<StatusCode, AppError> {
    operator(&identity)?;
    let app = state.clone();
    if !tokio::task::spawn_blocking(move || app.phones.cancel_offer()).await?? {
        return Err(AppError::not_found(NO_OFFER.to_owned()));
    }
    state.engine.emit_scope(StateScope::Phones);
    Ok(StatusCode::NO_CONTENT)
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
    State(state): State<AppState>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>, AppError> {
    let app = state.clone();
    let paired = tokio::task::spawn_blocking(move || {
        app.phones
            .pair(&request, &app.server_id, &app.server_name, Timestamp::now())
            .map(|paired| with_gps_nodes(&app, paired))
    })
    .await?;
    match paired {
        Ok(response) => {
            state.engine.emit_scope(StateScope::Phones);
            Ok(Json(response))
        }
        Err(error @ (PairError::WrongCode { .. } | PairError::Burned)) => {
            state.engine.emit_scope(StateScope::Phones);
            tokio::time::sleep(Duration::from_millis(PAIR_FAILURE_DELAY_MS)).await;
            Err(error.into())
        }
        Err(error) => Err(error.into()),
    }
}

fn with_gps_nodes(state: &AppState, mut paired: PairResponse) -> PairResponse {
    match active_graph(state).and_then(|graph| state.phones.one(&paired.phone.id, graph.as_ref())) {
        Ok(phone) => paired.phone = phone,
        Err(error) => tracing::warn!(%error, "a paired phone is shown without its GPS nodes"),
    }
    paired
}

#[utoipa::path(
    get, path = "/api/phones/self",
    responses(
        (status = 200, description = "The calling phone", body = PhoneSelf),
        (status = 401, description = "Phone not paired", body = ApiError),
        (status = 403, description = "Not a phone", body = ApiError),
    ),
)]
pub(super) async fn get_phone_self(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<Json<PhoneSelf>, AppError> {
    let id = identity.phone().ok_or_else(not_allowed)?.to_owned();
    let app = state.clone();
    let phone = tokio::task::spawn_blocking(move || {
        let graph = active_graph(&app)?;
        app.phones.one(&id, graph.as_ref())
    })
    .await??;
    Ok(Json(PhoneSelf {
        phone,
        server_id: state.server_id.to_string(),
        server_name: state.server_name.to_string(),
    }))
}

#[utoipa::path(
    delete, path = "/api/phones/self",
    responses(
        (status = 204, description = "The calling phone is unpaired"),
        (status = 401, description = "Phone not paired", body = ApiError),
        (status = 403, description = "Not a phone", body = ApiError),
    ),
)]
pub(super) async fn unpair_phone_self(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
) -> Result<StatusCode, AppError> {
    let id = identity.phone().ok_or_else(not_allowed)?.to_owned();
    revoke(&state, id).await
}

#[utoipa::path(
    patch, path = "/api/phones/{id}",
    params(("id" = String, Path, description = "Phone id")),
    request_body = RenamePhoneRequest,
    responses(
        (status = 200, description = "The renamed phone", body = Phone),
        (status = 400, description = "Name not valid", body = ApiError),
        (status = 403, description = "Not allowed", body = ApiError),
        (status = 404, description = "No phone with that id", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn rename_phone(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
    Json(request): Json<RenamePhoneRequest>,
) -> Result<Json<Phone>, AppError> {
    operator(&identity)?;
    let app = state.clone();
    let phone = tokio::task::spawn_blocking(move || {
        let graph = active_graph(&app)?;
        app.phones.rename(&id, &request.name, graph.as_ref())
    })
    .await??;
    state.engine.emit_scope(StateScope::Phones);
    Ok(Json(phone))
}

#[utoipa::path(
    delete, path = "/api/phones/{id}",
    params(("id" = String, Path, description = "Phone id")),
    responses(
        (status = 204, description = "The phone is revoked and its sockets close"),
        (status = 403, description = "Not allowed", body = ApiError),
        (status = 404, description = "No phone with that id", body = ApiError),
    ),
)]
pub(super) async fn revoke_phone(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    operator(&identity)?;
    revoke(&state, id).await
}

async fn revoke(state: &AppState, id: String) -> Result<StatusCode, AppError> {
    let app = state.clone();
    tokio::task::spawn_blocking(move || app.phones.revoke(&app, &id)).await??;
    Ok(StatusCode::NO_CONTENT)
}
