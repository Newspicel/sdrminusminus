use std::{collections::HashMap, sync::Arc, time::Duration};

use sdrmm_engine::{Engine, TrunkSystem};
use sdrmm_wire::{NodeBody, ServerEvent, StateScope};
use tokio::sync::{broadcast::error::RecvError, watch};

use crate::Store;

const DEBOUNCE: Duration = Duration::from_millis(250);

const REFRESH: Duration = Duration::from_secs(30);

pub(crate) type Retentions = Arc<HashMap<String, Duration>>;

pub(crate) async fn watch_patch(
    engine: std::sync::Weak<Engine>,
    store: Arc<Store>,
    retentions: watch::Sender<Retentions>,
) {
    let Some(strong) = engine.upgrade() else {
        return;
    };
    let mut events = strong.subscribe_events();
    drop(strong);
    let mut configured: Option<Vec<TrunkSystem>> = None;
    loop {
        let Some(strong) = engine.upgrade() else {
            return;
        };
        let store = store.clone();
        let resolved = {
            let engine = strong.clone();
            tokio::task::spawn_blocking(move || resolve(&store, &engine)).await
        };
        match resolved {
            Ok((systems, retention)) => {
                if configured.as_ref() != Some(&systems) {
                    strong.configure_trunking(systems.clone());
                    configured = Some(systems);
                }
                let _ = retentions.send(retention);
            }
            Err(error) => tracing::warn!(%error, "could not resolve the trunk systems"),
        }
        drop(strong);
        loop {
            match tokio::time::timeout(REFRESH, events.recv()).await {
                Ok(Ok(event)) if !touches_binding(&event) => continue,
                Ok(Err(RecvError::Closed)) => return,
                Ok(_) | Err(_) => break,
            }
        }
        tokio::time::sleep(DEBOUNCE).await;
    }
}

fn touches_binding(event: &ServerEvent) -> bool {
    matches!(
        event,
        ServerEvent::StateChanged {
            scope: StateScope::All | StateScope::DeviceSet(_) | StateScope::Workspaces
        }
    )
}

fn resolve(store: &Store, engine: &Engine) -> (Vec<TrunkSystem>, Retentions) {
    let Ok(Some(workspace)) = store.active_workspace() else {
        return (Vec::new(), Retentions::default());
    };
    let graph = &workspace.snapshot.graph;
    let state = engine.snapshot();
    let live: HashMap<String, (u32, u32)> = crate::workspace::bind(graph, &state)
        .into_iter()
        .flat_map(|binding| {
            let device_set = binding.device_set;
            binding
                .channels
                .into_iter()
                .map(move |(node, channel)| (node, (device_set, channel)))
        })
        .collect();
    let mut systems = Vec::new();
    let mut retentions = HashMap::new();
    for node in &graph.nodes {
        let NodeBody::DmrTrunk(settings) = &node.body else {
            continue;
        };
        systems.push(TrunkSystem {
            node: node.id.clone(),
            protocol: settings.protocol,
            carriers: graph
                .sources_of(&node.id, "events")
                .filter_map(|source| live.get(source).copied())
                .collect(),
        });
        if settings.retention_seconds > 0 {
            retentions.insert(
                node.id.clone(),
                Duration::from_secs(u64::from(settings.retention_seconds)),
            );
        }
    }
    (systems, Arc::new(retentions))
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::{
        DmrTrunkNode, DmrTrunkProtocol, PatchEdge, PatchNode, PortRef, Position,
        UpdateWorkspaceRequest, WorkspaceSnapshot,
    };

    use super::*;

    fn trunk_node(retention_seconds: u32) -> PatchNode {
        PatchNode {
            id: "trunk".to_owned(),
            body: NodeBody::DmrTrunk(DmrTrunkNode {
                protocol: DmrTrunkProtocol::TierThree,
                retention_seconds,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    #[test]
    fn a_trunk_node_with_no_live_carrier_still_reports_itself() {
        let store = Store::open(None).expect("in-memory store");
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let id = store
            .create_workspace("w", &WorkspaceSnapshot::empty())
            .expect("workspace");
        store.activate_workspace(id).expect("activate");
        let mut detail = store.workspace(id).expect("detail");
        detail.snapshot.graph.nodes.push(trunk_node(0));
        store
            .update_workspace(
                id,
                &UpdateWorkspaceRequest {
                    revision: detail.info.revision,
                    name: None,
                    snapshot: Some(detail.snapshot),
                },
            )
            .expect("store the patch");

        let (systems, retentions) = resolve(&store, &engine);
        assert_eq!(systems.len(), 1);
        assert!(systems[0].carriers.is_empty());
        assert!(
            retentions.is_empty(),
            "zero retention must not buffer calls"
        );
    }

    #[test]
    fn only_the_input_side_of_the_events_port_names_a_carrier() {
        let mut graph = sdrmm_wire::PatchGraph::default();
        graph.nodes.push(trunk_node(60));
        graph.edges.push(PatchEdge {
            from: PortRef {
                node: "trunk".to_owned(),
                port: "events".to_owned(),
            },
            to: PortRef {
                node: "log".to_owned(),
                port: "events".to_owned(),
            },
        });
        assert_eq!(graph.sources_of("trunk", "events").count(), 0);
    }
}
