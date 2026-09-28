use sdrmm_wire::{PatchGraph, WorkspaceState};

use crate::{AppState, array, df_fusion, fusion_routes, radar, survey};

pub(crate) fn reconcile_graph_hooks(
    state: &AppState,
    graph: &PatchGraph,
    bound: &[(String, u32)],
    saved: &WorkspaceState,
) -> Vec<(String, String)> {
    let mut refusals = array::reconcile(state, graph, bound, saved);
    refusals.extend(radar::reconcile(state, graph));
    refusals.extend(df_fusion::reconcile(state, graph));
    refusals.extend(fusion_routes::reconcile(state, graph));
    refusals.extend(survey::reconcile(state, graph));
    refusals
}
