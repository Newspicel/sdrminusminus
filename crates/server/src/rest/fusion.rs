use super::*;

#[utoipa::path(
    get, path = "/api/fusion/{node}",
    params(("node" = String, Path, description = "Triangulation node id")),
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
    delete, path = "/api/fusion/{node}",
    params(("node" = String, Path, description = "Triangulation node id")),
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
