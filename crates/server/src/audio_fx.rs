use std::{
    collections::HashSet,
    sync::{Arc, Weak},
    time::Duration,
};

use sdrmm_engine::Engine;
use sdrmm_wire::{NodeBody, PatchGraph};

use crate::{
    recorders::{Hooks, Recorders},
    store::Store,
};

const SYNC_INTERVAL: Duration = Duration::from_millis(250);

pub(crate) async fn run(engine: Weak<Engine>, hooks: Arc<Hooks>) {
    let mut recorders = Recorders::default();
    loop {
        let Some(strong) = engine.upgrade() else {
            return;
        };
        let hooks = hooks.clone();
        let ticked = tokio::task::spawn_blocking(move || {
            if let Some(graph) = sync(&strong, &hooks.store) {
                recorders.reconcile(&strong, graph.as_ref(), &hooks);
            }
            recorders
        })
        .await;
        match ticked {
            Ok(next) => recorders = next,
            Err(error) => {
                tracing::error!(%error, "audio FX sync failed");
                return;
            }
        }
        tokio::time::sleep(SYNC_INTERVAL).await;
    }
}

pub(crate) fn sync(engine: &Engine, store: &Store) -> Option<Option<PatchGraph>> {
    match store.active_workspace() {
        Ok(Some(workspace)) => {
            apply(engine, &workspace.snapshot.graph);
            Some(Some(workspace.snapshot.graph))
        }
        Ok(None) => {
            engine.retain_audio_fx(&HashSet::new());
            Some(None)
        }
        Err(error) => {
            tracing::error!(%error, "could not load audio FX settings");
            None
        }
    }
}

fn apply(engine: &Engine, graph: &PatchGraph) {
    let mut present = HashSet::new();
    for node in &graph.nodes {
        let NodeBody::AudioFx(fx) = &node.body else {
            continue;
        };
        match engine.set_audio_fx(&node.id, fx.settings.clone()) {
            Ok(()) => {
                present.insert(node.id.clone());
            }
            Err(error) => tracing::warn!(node = %node.id, %error, "audio FX settings refused"),
        }
    }
    engine.retain_audio_fx(&present);
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AudioFxNode, AudioRoute, PatchNode, Position};

    use super::*;

    fn fx(id: &str) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body: NodeBody::AudioFx(AudioFxNode::default()),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    #[test]
    fn only_the_fx_nodes_in_the_patch_can_be_routed_through() {
        let engine = Engine::new(None);
        apply(
            &engine,
            &PatchGraph {
                nodes: vec![fx("kept")],
                edges: Vec::new(),
            },
        );
        let through = |node: &str| AudioRoute {
            fx: vec![node.to_owned()],
            ..AudioRoute::channel(0, 0)
        };
        let unknown = engine.subscribe_route_pcm(&through("gone")).unwrap_err();
        assert!(
            unknown.to_string().contains("not in the patch"),
            "{unknown}"
        );
        let known = engine.subscribe_route_pcm(&through("kept")).unwrap_err();
        assert!(known.is_not_found(), "{known}");
    }
}
