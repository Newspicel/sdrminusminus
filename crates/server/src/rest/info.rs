use super::*;

#[utoipa::path(
    get, path = "/api/patch/catalog",
    responses((
        status = 200,
        description = "The node palette: every node kind this build offers, its category and \
                       its ports. The canvas renders its \"add node\" menu and enforces its \
                       drag-time connection rules from this, so a new node type needs no \
                       frontend table",
        body = PatchCatalog,
    )),
)]
pub(super) async fn get_patch_catalog() -> Json<PatchCatalog> {
    Json(PatchCatalog::build())
}

#[utoipa::path(
    get, path = "/api/bandplan/regions",
    responses((
        status = 200,
        description = "Selectable band-plan regions and the one to use with no stored \
                       preference. Static: the tables ship with the binary",
        body = BandRegionsResponse,
    )),
)]
pub(super) async fn list_band_regions() -> Json<BandRegionsResponse> {
    Json(crate::bandplan::regions())
}

#[utoipa::path(
    get, path = "/api/bandplan/regions/{region}",
    params(("region" = String, Path, description = "Region id from /api/bandplan/regions")),
    responses(
        (
            status = 200,
            description = "The region's allocations, already layered most-specific-wins, as one \
                           lane per view: the regulatory stack merged into one, and each amateur \
                           band plan as an overlay. Clipping it to a scope's window and \
                           searching it are client-side arithmetic over this document",
            body = BandPlan,
        ),
        (status = 404, description = "No such region", body = ApiError),
    ),
)]
pub(super) async fn get_band_plan(Path(region): Path<String>) -> Result<Json<BandPlan>, AppError> {
    crate::bandplan::plan(&region)
        .map(Json)
        .ok_or_else(|| AppError::not_found(format!("no band plan for region {region}")))
}

#[utoipa::path(
    get, path = "/api/bandplan/locate",
    params(LocateQuery),
    responses(
        (
            status = 200,
            description = "The region a coordinate falls in. Coarse by construction — bounding \
                           boxes over the national footprints and an approximation of the ITU \
                           lines — so `approximate` says when only the ITU region could be \
                           decided and the operator should confirm it",
            body = BandRegionMatch,
        ),
        (status = 400, description = "Coordinate out of range", body = ApiError),
    ),
)]
pub(super) async fn locate_band_region(
    Query(at): Query<LocateQuery>,
) -> Result<Json<BandRegionMatch>, AppError> {
    if !(-90.0..=90.0).contains(&at.lat) || !(-180.0..=180.0).contains(&at.lon) {
        return Err(AppError::bad_request(format!(
            "lat must be -90..90 and lon -180..180, got {}, {}",
            at.lat, at.lon
        )));
    }
    Ok(Json(crate::bandplan::locate(at.lat, at.lon)))
}

#[utoipa::path(
    get, path = "/api/clients",
    responses((
        status = 200,
        description = "WebSocket clients connected right now, including the caller's own \
                       socket. Invalidated by the `clients` scope, never polled",
        body = ClientsResponse,
    )),
)]
pub(super) async fn get_clients(State(state): State<AppState>) -> Json<ClientsResponse> {
    Json(ClientsResponse {
        clients: state.clients.load(std::sync::atomic::Ordering::Relaxed),
    })
}

#[utoipa::path(
    get, path = "/api/occupancy",
    params(
        ("min_samples" = Option<u64>, Query,
         description = "Drop buckets observed fewer times than this. A duty cycle from three \
                        sightings is not a measurement; the default keeps that out of the report"),
    ),
    responses((
        status = 200,
        description = "How much of the time each slice of spectrum has carried a signal, busiest \
                       first. Accumulated from the spectrum tap of every running receiver against \
                       absolute frequency, so a scan and a retune both add to the same picture",
        body = OccupancyReport,
    )),
)]
pub(super) async fn get_occupancy(
    State(state): State<AppState>,
    Query(query): Query<OccupancyQuery>,
) -> Json<OccupancyReport> {
    let occupancy = state
        .engine
        .occupancy()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Json(occupancy.report(query.min_samples.unwrap_or(DEFAULT_MIN_SAMPLES)))
}

pub(super) const DEFAULT_MIN_SAMPLES: u64 = 30;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub(super) struct OccupancyQuery {
    min_samples: Option<u64>,
}

#[utoipa::path(
    get, path = "/api/ionosonde",
    responses((
        status = 200,
        description = "The ionosonde network's current MUF(3000 km) per sounding site, cached \
                       for fifteen minutes — the interval the upstream map is rebuilt on. \
                       A server with no route to the feed answers the same shape with an empty \
                       station list and the reason in `error`, so the propagation map degrades \
                       to what this receiver measured on its own",
        body = IonosondeReport,
    )),
)]
pub(super) async fn get_ionosonde(State(state): State<AppState>) -> Json<IonosondeReport> {
    Json(state.ionosonde.report().await)
}

#[utoipa::path(
    get, path = "/api/auth",
    responses((
        status = 200,
        description = "Whether this server requires the shared token. Answered without \
                       authentication so a client can prompt before its first real request",
        body = AuthInfo,
    )),
)]
pub(super) async fn get_auth(State(state): State<AppState>) -> Json<AuthInfo> {
    Json(AuthInfo {
        token_required: state.auth.required(),
    })
}

#[utoipa::path(
    get, path = "/api/doctor",
    responses((
        status = 200,
        description = "Environment diagnostics: compiled backends, devices found, USB \
                       permissions and storage paths (the same report `sdrmm --doctor` prints)",
        body = DoctorReport,
    )),
)]
pub(super) async fn get_doctor(
    State(state): State<AppState>,
) -> Result<Json<DoctorReport>, AppError> {
    let engine = state.engine.clone();
    let db_path = state.db_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        crate::doctor::report(
            engine.registry(),
            db_path.as_deref(),
            engine.recordings_dir(),
        )
    })
    .await?;
    Ok(Json(report))
}

#[utoipa::path(
    get, path = "/api/tools",
    responses((
        status = 200,
        description = "Every tool this build offers. Tools stand beside the receiver: they own \
                       no device set and no channel, and a build without a tool's hardware \
                       support simply does not list it",
        body = ToolsResponse,
    )),
)]
pub(super) async fn list_tools(State(state): State<AppState>) -> Json<ToolsResponse> {
    Json(ToolsResponse {
        tools: state.tools.descriptors(),
    })
}

#[utoipa::path(
    post, path = "/api/tools/run",
    request_body = ToolRequest,
    responses(
        (
            status = 200,
            description = "The tool's answer, tagged with the same tool id the request carried",
            body = ToolResponse,
        ),
        (status = 400, description = "The tool refused the request", body = ApiError),
        (status = 404, description = "No such tool in this build", body = ApiError),
        (status = 503, description = "The tool's hardware is not attached", body = ApiError),
    ),
)]
pub(super) async fn run_tool(
    State(state): State<AppState>,
    Json(request): Json<ToolRequest>,
) -> Result<Json<ToolResponse>, AppError> {
    let tools = state.tools.clone();
    let response = tokio::task::spawn_blocking(move || tools.run(request)).await??;
    Ok(Json(response))
}

#[utoipa::path(
    get, path = "/api/about",
    responses((
        status = 200,
        description = "This build, its license, and every third-party component it distributes",
        body = AboutResponse,
    )),
)]
pub(super) async fn get_about(State(state): State<AppState>) -> Json<AboutResponse> {
    Json(AboutResponse {
        lan_addresses: crate::notices::lan_addresses(),
        routing: state.routing.configured(),
        offline_basemap: crate::basemap::basemap_path(&state).is_some(),
        ..crate::notices::about()
    })
}

#[utoipa::path(
    get, path = "/api/about/licenses/{id}",
    params(("id" = String, Path, description = "Content id from an attribution's `texts`")),
    responses(
        (status = 200, description = "The full license text", body = LicenseTextResponse),
        (status = 404, description = "No component ships a text with that id", body = ApiError),
    ),
)]
pub(super) async fn get_license_text(
    Path(id): Path<String>,
) -> Result<Json<LicenseTextResponse>, AppError> {
    crate::notices::license_text(&id)
        .map(Json)
        .ok_or_else(|| AppError {
            status: StatusCode::NOT_FOUND,
            body: ApiError {
                error: "unknown license text".to_string(),
                detail: Some(format!("no component ships a license text with id `{id}`")),
            },
        })
}
