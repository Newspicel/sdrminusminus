use super::*;
use crate::workspace::Restored;

#[utoipa::path(
    get, path = "/api/templates",
    responses((
        status = 200,
        description = "Built-in workspace templates (read-only; presets are the writable kind)",
        body = TemplatesResponse,
    )),
)]
pub(super) async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<TemplatesResponse>, AppError> {
    let engine = state.engine.clone();
    let probed = tokio::task::spawn_blocking(move || engine.probe_devices()).await?;
    let templates = crate::templates::all()
        .iter()
        .map(|template| TemplateInfo {
            supported_devices: probed
                .iter()
                .filter(|device| {
                    device
                        .profile
                        .as_ref()
                        .is_none_or(|profile| template.unmet_by(profile).is_none())
                })
                .map(DeviceInfo::id)
                .collect(),
            ..template.clone()
        })
        .collect();
    Ok(Json(TemplatesResponse { templates }))
}

#[utoipa::path(
    post, path = "/api/templates/{id}/apply",
    params(("id" = String, Path, description = "Template id")),
    request_body = ApplyTemplateRequest,
    responses(
        (status = 204, description = "Template applied"),
        (
            status = 400,
            description = "Template rejected by the target device (usually out of its tuning \
                           range); `detail` reports what a partial application left behind",
            body = ApiError,
        ),
        (status = 404, description = "Template or device set not found", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn apply_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ApplyTemplateRequest>,
) -> Result<StatusCode, AppError> {
    let template = crate::templates::get(&id)
        .ok_or_else(|| AppError::not_found(format!("template {id} not found")))?;
    let engine = state.engine.clone();
    let store = state.store.clone();
    let settings = DeviceSettings {
        center_hz: Some(template.center_hz),
        sample_rate: Some(template.sample_rate),
        ..DeviceSettings::default()
    };
    let channels = template.channels.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let open = engine.snapshot();
        if let Some(set) = open.device_sets.iter().find(|set| set.id == req.device_set)
            && let Some(reason) = template.unmet_by(&set.capabilities.profile())
        {
            return Err(AppError::bad_request(format!(
                "{} cannot run this template: {reason}",
                set.device.label
            )));
        }
        apply_configuration(&engine, req.device_set, settings, channels, "template")?;
        apply_template_patch(&engine, &store, template, req.device_set)
    })
    .await??;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) fn apply_template_patch(
    engine: &sdrmm_engine::Engine,
    store: &Store,
    template: &TemplateInfo,
    device_set: u32,
) -> Result<(), AppError> {
    let Some(patch) = &template.patch else {
        return Ok(());
    };
    let Some(mut active) = store.active_workspace()? else {
        return Ok(());
    };
    let device = engine
        .snapshot()
        .device_sets
        .iter()
        .find(|set| set.id == device_set)
        .map(|set| sdrmm_wire::DeviceRef::from_info(&set.device));
    active.snapshot.merge_patch(
        patch,
        &format!("template:{}:", template.id),
        device.as_ref(),
    );
    let update = UpdateWorkspaceRequest {
        revision: active.info.revision,
        name: None,
        snapshot: Some(active.snapshot),
    };
    match store.update_workspace(active.info.id, &update) {
        Ok(_) => {
            engine.emit_scope(StateScope::Workspaces);
            Ok(())
        }
        Err(StoreError::WorkspaceConflict { .. }) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

pub(super) fn first_binding(app: &AppState, workspace: i64, node: &str, device_set: u32) -> bool {
    app.restored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((workspace, node.to_string(), device_set))
}

pub(super) fn note_restore(app: &AppState, node: &str, restored: bool) {
    let mut unrestored = app
        .unrestored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    unrestored.retain(|held| held != node);
    if !restored {
        unrestored.push(node.to_string());
    }
}

pub(super) fn forget_closed_bindings(app: &AppState, state: &StateSnapshot) {
    app.restored
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|(_, _, set)| state.device_sets.iter().any(|live| live.id == *set));
}

pub(super) fn bring_up(
    app: &AppState,
    workspace: i64,
    snapshot: &WorkspaceSnapshot,
    saved: &WorkspaceState,
) -> Result<PatchApplyReport, AppError> {
    let engine = &app.engine;
    let mut report = PatchApplyReport::default();
    workspace::describe_arrays(engine, &snapshot.graph);
    workspace::release_array_members(engine, &snapshot.graph);
    let mut state = engine.snapshot();
    forget_closed_bindings(app, &state);

    for (node, device_set) in workspace::bind_devices(&snapshot.graph, &state) {
        if first_binding(app, workspace, &node, device_set) {
            match workspace::restore_device(engine, device_set, &node, saved) {
                Ok(whole) => note_restore(app, &node, whole == Restored::Whole),
                Err(reason) => {
                    note_restore(app, &node, false);
                    report.refused.push(PatchRefusal {
                        node: node.clone(),
                        reason,
                    });
                }
            }
        }
        report.bound.push(PatchBinding { node, device_set });
    }

    let mut attached: Option<Vec<DeviceInfo>> = None;
    for node in snapshot.graph.device_nodes() {
        if snapshot.graph.array_holding(&node.id).is_some() {
            continue;
        }
        let Some(reference) = node.body.device_ref(&node.id) else {
            continue;
        };
        let reference = &reference;
        if report.bound.iter().any(|bound| bound.node == node.id) {
            continue;
        }
        let devices = attached.get_or_insert_with(|| engine.probe_devices());
        let open = devices
            .iter()
            .filter(|info| reference.matches(info))
            .find(|info| {
                !state
                    .device_sets
                    .iter()
                    .any(|set| set.device.id() == info.id())
            })
            .map(DeviceInfo::id);
        match open {
            Some(device_id) => match engine.create_device_set(&device_id) {
                Ok(id) => {
                    report.opened += 1;
                    first_binding(app, workspace, &node.id, id);
                    match workspace::restore_device(engine, id, &node.id, saved) {
                        Ok(whole) => note_restore(app, &node.id, whole == Restored::Whole),
                        Err(reason) => {
                            note_restore(app, &node.id, false);
                            report.refused.push(PatchRefusal {
                                node: node.id.clone(),
                                reason,
                            });
                        }
                    }
                    report.bound.push(PatchBinding {
                        node: node.id.clone(),
                        device_set: id,
                    });
                    state = engine.snapshot();
                }
                Err(err) => report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: err.to_string(),
                }),
            },
            None => report.absent.push(node.id.clone()),
        }
    }

    for binding in &report.bound {
        let Some(set) = state
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        let mut live: Vec<(&str, u32)> = set
            .channels
            .iter()
            .map(|channel| (channel.settings.params.type_id(), channel.stream))
            .collect();
        for (node, stream) in snapshot.graph.channels_of(&binding.node) {
            let NodeBody::Channel(channel) = &node.body else {
                continue;
            };
            if let Some(at) = live.iter().position(|(type_id, live_stream)| {
                *type_id == channel.channel_type && *live_stream == stream
            }) {
                live.remove(at);
                continue;
            }
            let Some(settings) =
                workspace::channel_settings(&node.id, &channel.channel_type, saved)
            else {
                report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: format!("this build has no channel type {:?}", channel.channel_type),
                });
                continue;
            };
            if let Err(err) = engine.add_channel(set.id, stream, settings) {
                report.refused.push(PatchRefusal {
                    node: node.id.clone(),
                    reason: err.to_string(),
                });
            } else {
                report.created += 1;
            }
        }
    }
    let bound: Vec<(String, u32)> = report
        .bound
        .iter()
        .map(|binding| (binding.node.clone(), binding.device_set))
        .collect();
    for (node, reason) in crate::coherent::apply(app, &snapshot.graph, &bound) {
        report.refused.push(PatchRefusal { node, reason });
    }
    let live = engine.snapshot();
    for (node, device_set, stream) in crate::coherent::beam_channels(app, &snapshot.graph, &live) {
        let Some(patch) = snapshot.graph.node(&node) else {
            continue;
        };
        let NodeBody::Channel(channel) = &patch.body else {
            continue;
        };
        let already = live
            .device_sets
            .iter()
            .find(|set| set.id == device_set)
            .is_some_and(|set| {
                set.channels.iter().any(|existing| {
                    existing.stream == stream
                        && existing.settings.params.type_id() == channel.channel_type
                })
            });
        if already {
            continue;
        }
        let Some(settings) = workspace::channel_settings(&node, &channel.channel_type, saved)
        else {
            report.refused.push(PatchRefusal {
                node: node.clone(),
                reason: format!("this build has no channel type {:?}", channel.channel_type),
            });
            continue;
        };
        match engine.add_channel(device_set, stream, settings) {
            Ok(_) => report.created += 1,
            Err(err) => report.refused.push(PatchRefusal {
                node,
                reason: err.to_string(),
            }),
        }
    }
    Ok(report)
}

#[utoipa::path(
    get, path = "/api/workspaces",
    responses((
        status = 200,
        description = "Stored workspaces and which one is active. Layouts are not included — \
                       fetch one workspace for that",
        body = WorkspacesResponse,
    )),
)]
pub(super) async fn list_workspaces(
    State(state): State<AppState>,
) -> Result<Json<WorkspacesResponse>, AppError> {
    let store = state.store.clone();
    let workspaces = tokio::task::spawn_blocking(move || store.list_workspaces()).await??;
    Ok(Json(workspaces))
}

#[utoipa::path(
    post, path = "/api/workspaces",
    request_body = CreateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace stored", body = CreatedRowId),
        (status = 400, description = "Layout rejected", body = ApiError),
        (status = 409, description = "A workspace of that name already exists", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn create_workspace(
    State(state): State<AppState>,
    Json(req): Json<CreateWorkspaceRequest>,
) -> Result<Json<CreatedRowId>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let snapshot = req.snapshot.unwrap_or_else(WorkspaceSnapshot::empty);
        let id = store.create_workspace(&req.name, &snapshot)?;
        engine.emit_scope(StateScope::Workspaces);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    get, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "The workspace and its layout", body = WorkspaceDetail),
        (status = 404, description = "Workspace not found", body = ApiError),
        (
            status = 500,
            description = "The stored layout no longer parses — the row is left intact so a \
                           newer build can still read it",
            body = ApiError,
        ),
    ),
)]
pub(super) async fn get_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    let store = state.store.clone();
    let workspace = tokio::task::spawn_blocking(move || store.workspace(id)).await??;
    Ok(Json(workspace))
}

#[utoipa::path(
    get, path = "/api/workspaces/{id}/export",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 200,
            description = "The workspace as a portable document: its name, the patch and rack it \
                           draws, and the tuning each node was left on. Nothing server-local \
                           travels — no id, revision or history — so importing it makes a new \
                           workspace rather than overwriting one",
            body = WorkspaceExport,
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (
            status = 500,
            description = "The stored layout no longer parses — the row is left intact so a \
                           newer build can still read it",
            body = ApiError,
        ),
    ),
)]
pub(super) async fn export_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Response, AppError> {
    let store = state.store.clone();
    let export = tokio::task::spawn_blocking(move || store.export_workspace(id)).await??;
    let filename = export_filename(&export.name, id);
    let body = serde_json::to_string_pretty(&export)
        .map_err(|err| AppError::internal(format!("serializing the workspace export: {err}")))?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

pub(super) fn export_filename(name: &str, id: i64) -> String {
    let mut slug = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("workspace-{id}.json")
    } else {
        format!("workspace-{slug}.json")
    }
}

#[utoipa::path(
    post, path = "/api/workspaces/import",
    request_body = WorkspaceExport,
    responses(
        (
            status = 200,
            description = "Imported as a new workspace, keeping the one it was exported from. A \
                           name already in use gains a copy number; the radios it names are \
                           opened by activating and applying it, and the ones this machine does \
                           not have are reported absent",
            body = CreatedRowId,
        ),
        (
            status = 400,
            description = "Not a workspace document this build can read, or its layout is \
                           rejected",
            body = ApiError,
        ),
        (status = 409, description = "No free name is left for this one", body = ApiError),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn import_workspace(
    State(state): State<AppState>,
    Json(export): Json<WorkspaceExport>,
) -> Result<Json<CreatedRowId>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let id = tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
        let id = store.import_workspace(&export)?;
        engine.emit_scope(StateScope::Workspaces);
        Ok(id)
    })
    .await??;
    Ok(Json(CreatedRowId { id }))
}

#[utoipa::path(
    put, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    request_body = UpdateWorkspaceRequest,
    responses(
        (status = 200, description = "Workspace updated", body = WorkspaceInfo),
        (status = 400, description = "Layout rejected", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (
            status = 409,
            description = "Another client wrote first (stale revision) or the name is taken; \
                           reload and reapply",
            body = ApiError,
        ),
        (status = 422, description = "Malformed request body", body = ApiError),
    ),
)]
pub(super) async fn update_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<UpdateWorkspaceRequest>,
) -> Result<Json<WorkspaceInfo>, AppError> {
    let engine = state.engine.clone();
    let store = state.store.clone();
    let info = tokio::task::spawn_blocking(move || -> Result<WorkspaceInfo, AppError> {
        let info = store.update_workspace(id, &req)?;
        engine.emit_scope(StateScope::Workspaces);
        Ok(info)
    })
    .await??;
    reconcile_gps(state).await?;
    Ok(Json(info))
}

#[utoipa::path(
    delete, path = "/api/workspaces/{id}",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 204, description = "Workspace removed"),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
pub(super) async fn delete_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let gps_state = state.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.store.active_workspace_id()?;
        let after = state.store.delete_workspace(id)?;
        if let Some(promoted) = after.filter(|promoted| Some(*promoted) != before) {
            let detail = state.store.workspace(promoted)?;
            let saved = state.store.workspace_state(promoted)?;
            workspace::reconcile(&state, &detail.snapshot.graph, &saved);
        }
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(())
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/activate",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 204,
            description = "Workspace activated for every client. The hardware is reconciled to \
                           it: radios this workspace does not name are closed, channels it does \
                           not draw are dropped, and the radios it keeps are put back where it \
                           was left. Apply opens the rest",
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
pub(super) async fn activate_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let gps_state = state.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(err) = workspace::save_active(&state) {
            tracing::warn!(%err, "could not save the outgoing workspace before the switch");
        }
        state.store.activate_workspace(id)?;
        let detail = state.store.workspace(id)?;
        let saved = state.store.workspace_state(id)?;
        let report = workspace::reconcile(&state, &detail.snapshot.graph, &saved);
        tracing::info!(
            workspace = id,
            closed = report.closed,
            channels = report.dropped_channels,
            scans = report.stopped_scans,
            "activated"
        );
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(())
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/undo",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 200,
            description = "The workspace as it was before its last change, with the history it \
                           can still walk. The step is stored, so every client is told to reload \
                           it — one workspace, one history, whichever browser pressed undo",
            body = WorkspaceDetail,
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (status = 409, description = "Nothing left to undo", body = ApiError),
    ),
)]
pub(super) async fn undo_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    step_history(state, id, Store::undo_workspace).await
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/redo",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (status = 200, description = "The workspace an undo had stepped out of", body = WorkspaceDetail),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
        (status = 409, description = "Nothing left to redo", body = ApiError),
    ),
)]
pub(super) async fn redo_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    step_history(state, id, Store::redo_workspace).await
}

pub(super) async fn step_history(
    state: AppState,
    id: i64,
    step: fn(&Store, i64) -> Result<SteppedWorkspace, StoreError>,
) -> Result<Json<WorkspaceDetail>, AppError> {
    let gps_state = state.clone();
    let detail = tokio::task::spawn_blocking(move || -> Result<WorkspaceDetail, AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = state.store.workspace(id)?;
        let stepped = step(&state.store, id)?;
        let detail = stepped.detail;
        if state.store.active_workspace_id()? == Some(id) {
            if !before.snapshot.graph.same_topology(&detail.snapshot.graph) {
                let saved = state.store.workspace_state(id)?;
                workspace::reconcile(&state, &detail.snapshot.graph, &saved);
                let report = bring_up(&state, id, &detail.snapshot, &saved)?;
                for refusal in &report.refused {
                    tracing::warn!(
                        workspace = id,
                        node = refusal.node,
                        reason = refusal.reason,
                        "a node could not be restored by the history step"
                    );
                }
            }
            if let Some(settings) = &stepped.settings {
                workspace::restore_settings(&state, &detail.snapshot.graph, settings);
            }
        }
        state.engine.emit_scope(StateScope::Workspaces);
        Ok(detail)
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(Json(detail))
}

#[utoipa::path(
    post, path = "/api/workspaces/{id}/apply",
    params(("id" = i64, Path, description = "Workspace id")),
    responses(
        (
            status = 200,
            description = "The workspace was brought up: radios opened, channels added, and what \
                           could not be satisfied. Additive and idempotent — nothing is closed \
                           or deleted, so calling it twice changes nothing",
            body = PatchApplyReport,
        ),
        (status = 400, description = "Invalid path parameter", body = ApiError),
        (status = 404, description = "Workspace not found", body = ApiError),
    ),
)]
pub(super) async fn apply_workspace(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<PatchApplyReport>, AppError> {
    let gps_state = state.clone();
    let report = tokio::task::spawn_blocking(move || -> Result<PatchApplyReport, AppError> {
        let _serialized = state
            .apply_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let workspace = state.store.workspace(id)?;
        let saved = state.store.workspace_state(id)?;
        bring_up(&state, id, &workspace.snapshot, &saved)
    })
    .await??;
    reconcile_gps(gps_state).await?;
    Ok(Json(report))
}

pub(super) async fn reconcile_gps(state: AppState) -> Result<(), AppError> {
    tokio::task::spawn_blocking(move || state.gps.reconcile(&state)).await?;
    Ok(())
}
