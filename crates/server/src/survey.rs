use sdrmm_wire::PatchGraph;

use crate::AppState;

mod measure;

#[derive(Default)]
pub(crate) struct SurveyHub;

pub(crate) fn reconcile(_state: &AppState, _graph: &PatchGraph) -> Vec<(String, String)> {
    Vec::new()
}

pub(crate) fn start(_state: &AppState) {}
