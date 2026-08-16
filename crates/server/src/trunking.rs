use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use sdrmm_engine::{Engine, TrunkSystem};
use sdrmm_wire::{NodeBody, ServerEvent, StateScope};
use tokio::sync::{broadcast::error::RecvError, watch};

use crate::Store;

const DEBOUNCE: Duration = Duration::from_millis(250);

const REFRESH: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallBinding {
    pub node: String,
    pub device_set: u32,
    pub channel: u32,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct CallPolicy {
    pub trunk_systems: HashSet<String>,
    pub channels: Vec<CallBinding>,
}

pub(crate) type Recording = Arc<CallPolicy>;

pub(crate) async fn watch_patch(
    engine: std::sync::Weak<Engine>,
    store: Arc<Store>,
    recording: watch::Sender<Recording>,
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
            Ok((systems, policy)) => {
                if configured.as_ref() != Some(&systems) {
                    strong.configure_trunking(systems.clone());
                    configured = Some(systems);
                }
                let _ = recording.send(policy);
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

fn resolve(store: &Store, engine: &Engine) -> (Vec<TrunkSystem>, Recording) {
    let Ok(Some(workspace)) = store.active_workspace() else {
        return (Vec::new(), Recording::default());
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
    let mut policy = CallPolicy::default();
    for node in &graph.nodes {
        match &node.body {
            NodeBody::DmrTrunk(settings) => {
                systems.push(TrunkSystem {
                    node: node.id.clone(),
                    protocol: settings.protocol,
                    carriers: graph
                        .sources_of(&node.id, "events")
                        .filter_map(|source| live.get(source).copied())
                        .collect(),
                });
                if settings.record_calls {
                    policy.trunk_systems.insert(node.id.clone());
                }
            }
            NodeBody::Channel(settings) if settings.record_calls => {
                let Some(&(device_set, channel)) = live.get(&node.id) else {
                    continue;
                };
                policy.channels.push(CallBinding {
                    node: node.id.clone(),
                    device_set,
                    channel,
                });
            }
            _ => {}
        }
    }
    (systems, Arc::new(policy))
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::{
        ChannelNode, ChannelParams, ChannelSettings, DeviceRef, DmrTrunkNode, DmrTrunkProtocol,
        PatchEdge, PatchNode, PortRef, Position, UpdateWorkspaceRequest, WorkspaceSnapshot,
    };

    use super::*;

    fn channel_node(id: &str, channel_type: &str, record_calls: bool) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body: NodeBody::Channel(ChannelNode {
                channel_type: channel_type.to_owned(),
                record_calls,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn live_channel_workspace(store: &Store, engine: &Engine, node: PatchNode) {
        let set = engine
            .create_device_set("virtual:siggen")
            .expect("open the virtual radio");
        let NodeBody::Channel(channel) = &node.body else {
            panic!("the fixture wires a channel node");
        };
        engine
            .add_channel(
                set,
                0,
                ChannelSettings {
                    offset_hz: 0.0,
                    squelch_db: None,
                    squelch_auto_db: None,
                    params: ChannelParams::default_for(&channel.channel_type)
                        .expect("a known channel type"),
                    audio: Default::default(),
                },
            )
            .expect("add channel");
        let mut snapshot = WorkspaceSnapshot::starter();
        let node_id = node.id.clone();
        snapshot.graph.nodes.push(node);
        snapshot.graph.edges.push(PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: PortRef {
                node: node_id,
                port: "iq".to_owned(),
            },
        });
        let NodeBody::Device(device) = &mut snapshot
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "device")
            .expect("the starter draws a radio")
            .body
        else {
            panic!("the starter's radio is a device node");
        };
        device.device = Some(DeviceRef {
            backend: "virtual".to_owned(),
            serial: None,
            key: Some("siggen".to_owned()),
        });
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");
    }

    fn trunk_node(record_calls: bool) -> PatchNode {
        PatchNode {
            id: "trunk".to_owned(),
            body: NodeBody::DmrTrunk(DmrTrunkNode {
                protocol: DmrTrunkProtocol::TierThree,
                record_calls,
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
        detail.snapshot.graph.nodes.push(trunk_node(false));
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

        let (systems, policy) = resolve(&store, &engine);
        assert_eq!(systems.len(), 1);
        assert!(systems[0].carriers.is_empty());
        assert!(
            policy.trunk_systems.is_empty(),
            "a system told not to record must not buffer calls"
        );
    }

    #[test]
    fn a_plain_channel_that_keeps_calls_binds_without_any_trunk_system() {
        let store = Store::open(None).expect("in-memory store");
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        live_channel_workspace(&store, &engine, channel_node("dmr", "dmr", true));

        let (systems, policy) = resolve(&store, &engine);

        assert!(systems.is_empty(), "no trunk node was drawn");
        assert_eq!(policy.channels.len(), 1);
        assert_eq!(policy.channels[0].node, "dmr");
    }

    #[test]
    fn a_channel_that_keeps_nothing_never_binds() {
        let store = Store::open(None).expect("in-memory store");
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        live_channel_workspace(&store, &engine, channel_node("dmr", "dmr", false));

        let (_, policy) = resolve(&store, &engine);

        assert!(policy.channels.is_empty());
    }

    #[test]
    fn a_channel_the_radio_never_opened_cannot_record_calls() {
        let store = Store::open(None).expect("in-memory store");
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(channel_node("dmr", "dmr", true));
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");

        let (_, policy) = resolve(&store, &engine);

        assert!(policy.channels.is_empty());
    }

    #[test]
    fn only_the_input_side_of_the_events_port_names_a_carrier() {
        let mut graph = sdrmm_wire::PatchGraph::default();
        graph.nodes.push(trunk_node(true));
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
