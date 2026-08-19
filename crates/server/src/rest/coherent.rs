use super::*;

#[utoipa::path(
    post, path = "/api/coherent/{node}/calibrate",
    params(("node" = String, Path, description = "Patch node id of the coherent processor")),
    responses(
        (status = 200, description = "The calibration will be solved again from scratch"),
        (status = 404, description = "No coherent node of that name is running", body = ApiError),
        (status = 400, description = "The radio cannot calibrate", body = ApiError),
    ),
)]
pub(super) async fn calibrate_coherent(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<StatusCode, AppError> {
    let binding = state
        .coherent
        .binding(&node)
        .ok_or_else(|| AppError::not_found(format!("no coherent node {node} is running")))?;
    let engine = state.engine.clone();
    tokio::task::spawn_blocking(move || engine.recalibrate_coherent(binding.device_set)).await??;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get, path = "/api/coherent/{node}/fusion",
    params(("node" = String, Path, description = "Patch node id of the direction finder")),
    responses(
        (status = 200, description = "Where the bearings so far say the transmitter is", body = DfFusionState),
        (status = 404, description = "Nothing has been fused for that node", body = ApiError),
    ),
)]
pub(super) async fn get_fusion(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> Result<Json<DfFusionState>, AppError> {
    state
        .fusion
        .state(&node)
        .map(Json)
        .ok_or_else(|| AppError::not_found(format!("no bearings have been fused for {node}")))
}

#[utoipa::path(
    delete, path = "/api/coherent/{node}/fusion",
    params(("node" = String, Path, description = "Patch node id of the direction finder")),
    responses((status = 204, description = "The grid is empty again")),
)]
pub(super) async fn reset_fusion(
    State(state): State<AppState>,
    Path(node): Path<String>,
) -> StatusCode {
    state.fusion.reset(&node);
    state.engine.emit_event(ServerEvent::DfFusionUpdate {
        node: node.clone(),
        state: Box::new(state.fusion.state(&node).unwrap_or_default()),
    });
    StatusCode::NO_CONTENT
}

#[utoipa::path(
    post, path = "/api/df/bearings",
    request_body = BearingSubmission,
    params(("node" = Option<String>, Query, description = "Which direction finder's grid to feed")),
    responses(
        (status = 200, description = "Where every station's bearings now cross", body = DfFusionState),
        (status = 400, description = "The report is not a usable bearing", body = ApiError),
        (status = 404, description = "No direction finder is running to fuse it into", body = ApiError),
    ),
)]
pub(super) async fn ingest_bearing(
    State(state): State<AppState>,
    Query(query): Query<BearingQuery>,
    Json(submission): Json<BearingSubmission>,
) -> Result<Json<DfFusionState>, AppError> {
    let report = submission
        .into_report()
        .filter(BearingReport::valid)
        .ok_or_else(|| {
            AppError::bad_request(
                "a bearing report needs a station, a place on the globe and a confidence in 0..1"
                    .to_owned(),
            )
        })?;
    let node = match query.node {
        Some(node) => node,
        None => state
            .coherent
            .nodes()
            .into_iter()
            .find(|(_, binding)| binding.kind == "df")
            .map(|(node, _)| node)
            .ok_or_else(|| {
                AppError::not_found(
                    "no direction finder is running to fuse a bearing into".to_owned(),
                )
            })?,
    };
    let at = report
        .time
        .clone()
        .unwrap_or_else(|| format!("{:.9}", jiff::Timestamp::now()));
    let fused = state.fusion.ingest(&node, &report, &at);
    state.engine.emit_event(ServerEvent::DfFusionUpdate {
        node,
        state: Box::new(fused.clone()),
    });
    Ok(Json(fused))
}

#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
pub(super) struct BearingQuery {
    pub(super) node: Option<String>,
}

#[utoipa::path(
    post, path = "/api/routing/route",
    request_body = RouteRequest,
    responses(
        (status = 200, description = "A drivable route between the two points", body = Route),
        (status = 400, description = "Not a leg this build will ask for", body = ApiError),
        (status = 502, description = "The routing service refused or could not be reached", body = ApiError),
        (status = 503, description = "No routing backend is configured", body = ApiError),
    ),
)]
pub(super) async fn get_route(
    State(state): State<AppState>,
    Json(request): Json<RouteRequest>,
) -> Result<Json<Route>, AppError> {
    crate::routing::route(&state.routing, &request)
        .await
        .map(Json)
        .map_err(|error| {
            let status = match error {
                crate::routing::RoutingError::NotConfigured => StatusCode::SERVICE_UNAVAILABLE,
                crate::routing::RoutingError::BadRequest(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::BAD_GATEWAY,
            };
            AppError {
                status,
                body: ApiError {
                    error: error.to_string(),
                    detail: None,
                },
            }
        })
}
