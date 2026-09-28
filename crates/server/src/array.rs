use sdrmm_wire::{PatchGraph, WorkspaceState};

use crate::AppState;

#[derive(Default)]
pub(crate) struct ArrayHub;

pub(crate) fn reconcile(
    _state: &AppState,
    _graph: &PatchGraph,
    _bound: &[(String, u32)],
    _saved: &WorkspaceState,
) -> Vec<(String, String)> {
    Vec::new()
}

pub(crate) fn start_pump(_state: &AppState) {}
