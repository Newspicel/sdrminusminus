use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use sdrmm_engine::{Engine, TrunkSystem, trunking::TrunkRadio};
use sdrmm_wire::{DV_DECODER_KIND, NodeBody, PatchGraph, ServerEvent, StateScope};
use tokio::sync::{broadcast::error::RecvError, watch};

use crate::Store;

const DEBOUNCE: Duration = Duration::from_millis(250);

const REFRESH: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Frames,
    Squelch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallBinding {
    pub node: String,
    pub device_set: u32,
    pub channel: u32,
    pub gate: Gate,
}

#[derive(Clone, Default, Debug, PartialEq, Eq)]
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
    let saved = store.workspace_state(workspace.info.id).unwrap_or_default();
    let live_trunks = engine.trunk_systems();
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
    let devices: HashMap<String, u32> = crate::workspace::bind_devices(graph, &state)
        .into_iter()
        .collect();
    let mut systems = Vec::new();
    let mut policy = CallPolicy::default();
    for node in &graph.nodes {
        match &node.body {
            NodeBody::DmrTrunk(settings) => {
                systems.push(TrunkSystem {
                    node: node.id.clone(),
                    protocol: settings.protocol,
                    discovery: settings.discovery.clone(),
                    channel_map: settings.channel_map.clone(),
                    learned: learned_for(&saved, &node.id, &live_trunks),
                    radio: own_radio(graph, &node.id, settings, &devices),
                });
                if sends_events(graph, &node.id) {
                    policy.trunk_systems.insert(node.id.clone());
                }
            }
            NodeBody::Channel(_) if sends_events(graph, &node.id) => {
                let Some(&(device_set, channel)) = live.get(&node.id) else {
                    continue;
                };
                let Some(gate) = gate_of(&state, device_set, channel) else {
                    continue;
                };
                policy.channels.push(CallBinding {
                    node: node.id.clone(),
                    device_set,
                    channel,
                    gate,
                });
            }
            _ => {}
        }
    }
    (systems, Arc::new(policy))
}

fn sends_events(graph: &PatchGraph, node: &str) -> bool {
    graph
        .edges
        .iter()
        .any(|edge| edge.from.node == node && edge.from.port == "events")
}

fn gate_of(state: &sdrmm_wire::StateSnapshot, device_set: u32, channel: u32) -> Option<Gate> {
    let info = state
        .device_sets
        .iter()
        .find(|set| set.id == device_set)?
        .channels
        .iter()
        .find(|info| info.id == channel)?;
    let descriptor = sdrmm_channels::descriptor(info.settings.params.type_id())?;
    if descriptor.decoder_kind.as_deref() == Some(DV_DECODER_KIND) {
        Some(Gate::Frames)
    } else if descriptor.has_audio && !info.settings.squelch.is_off() {
        Some(Gate::Squelch)
    } else {
        None
    }
}

/// What the search already worked out for the site this system is sitting on. Keyed by colour
/// code because neighbouring sites of one system reuse logical channel numbers on their own
/// frequencies, so the plan learned at one site would place calls on the wrong one at the next.
pub(crate) fn learned_for(
    saved: &sdrmm_wire::WorkspaceState,
    node: &str,
    systems: &[sdrmm_wire::TrunkSystemStatus],
) -> Vec<sdrmm_wire::DmrChannelEntry> {
    let color_code = systems
        .iter()
        .find(|system| system.node == node)
        .and_then(|system| system.color_code);
    saved
        .trunk(node, color_code)
        .map(|trunk| trunk.channels.clone())
        .unwrap_or_default()
}

fn own_radio(
    graph: &sdrmm_wire::PatchGraph,
    node: &str,
    settings: &sdrmm_wire::DmrTrunkNode,
    devices: &HashMap<String, u32>,
) -> Option<TrunkRadio> {
    let control_hz = settings.control_hz?;
    let source = graph.sources_of(node, "iq").next()?;
    let stream = graph
        .edges
        .iter()
        .find(|edge| edge.to.node == node && edge.to.port == "iq" && edge.from.node == source)
        .and_then(|edge| sdrmm_wire::port_stream("iq", &edge.from.port))
        .unwrap_or(0);
    Some(TrunkRadio {
        device_set: *devices.get(source)?,
        stream,
        control_hz,
        ignore_crc: settings.ignore_crc,
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_device::DeviceRegistry;
    use sdrmm_wire::{
        ChannelNode, ChannelParams, ChannelSettings, DeviceRef, DmrTrunkNode, DmrTrunkProtocol,
        PatchEdge, PatchNode, PortRef, Position, UpdateWorkspaceRequest, WorkspaceSnapshot,
    };

    use super::*;

    fn channel_node(id: &str, channel_type: &str) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body: NodeBody::Channel(ChannelNode {
                channel_type: channel_type.to_owned(),
                tuning_locked: false,
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn live_channel_workspace(
        store: &Store,
        engine: &Engine,
        node: PatchNode,
        squelch: sdrmm_wire::Squelch,
        wired: bool,
    ) {
        let set = engine
            .create_device_set("virtual:band")
            .expect("open the virtual radio");
        let NodeBody::Channel(channel) = &node.body else {
            panic!("the fixture wires a channel node");
        };
        engine
            .add_channel(
                set,
                0,
                ChannelSettings {
                    frequency_hz: 100_000_000.0,
                    squelch,
                    params: ChannelParams::default_for(&channel.channel_type)
                        .expect("a known channel type"),
                    blanker: Default::default(),
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
                node: node_id.clone(),
                port: "iq".to_owned(),
            },
        });
        if wired {
            snapshot.graph.nodes.push(PatchNode {
                id: "log".to_owned(),
                body: NodeBody::DecoderLog,
                position: Position { x: 0.0, y: 0.0 },
                size: None,
                label: None,
            });
            snapshot.graph.edges.push(PatchEdge {
                from: PortRef {
                    node: node_id,
                    port: "events".to_owned(),
                },
                to: PortRef {
                    node: "log".to_owned(),
                    port: "events".to_owned(),
                },
            });
        }
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
            key: Some("band".to_owned()),
        });
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");
    }

    fn trunk_node() -> PatchNode {
        PatchNode {
            id: "trunk".to_owned(),
            body: NodeBody::DmrTrunk(DmrTrunkNode {
                protocol: DmrTrunkProtocol::TierThree,
                ..DmrTrunkNode::default()
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn trunk_on_a_radio(store: &Store, engine: &Engine, control_hz: Option<u64>) {
        let _ = engine.create_device_set("virtual:band");
        let mut snapshot = WorkspaceSnapshot::starter();
        snapshot.graph.nodes.push(PatchNode {
            id: "trunk".to_owned(),
            body: NodeBody::DmrTrunk(DmrTrunkNode {
                protocol: DmrTrunkProtocol::TierThree,
                control_hz,
                ..DmrTrunkNode::default()
            }),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        });
        snapshot.graph.edges.push(PatchEdge {
            from: PortRef {
                node: "device".to_owned(),
                port: "iq".to_owned(),
            },
            to: PortRef {
                node: "trunk".to_owned(),
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
            key: Some("band".to_owned()),
        });
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");
    }

    #[test]
    fn a_system_wired_straight_to_a_radio_names_its_own_control_channel() {
        let store = Store::open(None).expect("in-memory store");
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        trunk_on_a_radio(&store, &engine, Some(451_000_000));

        let (systems, _) = resolve(&store, &engine);

        assert_eq!(systems.len(), 1);
        let radio = systems[0].radio.expect("the radio was not resolved");
        assert_eq!(radio.control_hz, 451_000_000);
        assert_eq!(radio.stream, 0);
    }

    #[test]
    fn a_radio_can_be_wired_before_the_control_channel_is_named() {
        let store = Store::open(None).expect("in-memory store");
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        trunk_on_a_radio(&store, &engine, None);

        let (systems, _) = resolve(&store, &engine);

        assert_eq!(systems.len(), 1);
        assert!(
            systems[0].radio.is_none(),
            "a system tuned a radio without knowing where the control channel sits"
        );
    }

    #[test]
    fn a_system_whose_radio_never_opened_waits_instead_of_guessing() {
        let store = Store::open(None).expect("in-memory store");
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        trunk_on_a_radio(&store, &engine, Some(451_000_000));

        let (systems, _) = resolve(&store, &engine);

        assert_eq!(systems.len(), 1);
        assert!(
            systems[0].radio.is_none(),
            "a system claimed a radio the engine never opened"
        );
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
        detail.snapshot.graph.nodes.push(trunk_node());
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
        assert!(
            policy.trunk_systems.is_empty(),
            "a system with nothing wired to its events must not buffer calls"
        );
    }

    fn policy_for(channel_type: &str, squelch: sdrmm_wire::Squelch, wired: bool) -> CallPolicy {
        let store = Store::open(None).expect("in-memory store");
        let mut registry = DeviceRegistry::new();
        registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
        let engine = Engine::with_registry(registry, None);
        let node = channel_node("radio", channel_type);
        live_channel_workspace(&store, &engine, node, squelch, wired);
        let (systems, policy) = resolve(&store, &engine);
        assert!(systems.is_empty(), "no trunk node was drawn");
        Arc::unwrap_or_clone(policy)
    }

    #[test]
    fn a_wired_voice_channel_keeps_calls_framed_by_its_protocol() {
        let policy = policy_for("dmr", sdrmm_wire::Squelch::Off, true);
        assert_eq!(policy.channels.len(), 1);
        assert_eq!(policy.channels[0].node, "radio");
        assert_eq!(policy.channels[0].gate, Gate::Frames);
    }

    #[test]
    fn a_wired_analog_channel_keeps_calls_framed_by_its_squelch() {
        let squelch = sdrmm_wire::Squelch::Manual { level_db: -80.0 };
        let policy = policy_for("am", squelch, true);
        assert_eq!(policy.channels.len(), 1);
        assert_eq!(policy.channels[0].gate, Gate::Squelch);
    }

    #[test]
    fn an_open_squelch_has_no_calls_to_frame() {
        let policy = policy_for("am", sdrmm_wire::Squelch::Off, true);
        assert!(policy.channels.is_empty());
    }

    #[test]
    fn a_channel_with_nothing_on_its_events_keeps_nothing() {
        let policy = policy_for("dmr", sdrmm_wire::Squelch::Off, false);
        assert!(policy.channels.is_empty());
    }

    #[test]
    fn a_channel_the_radio_never_opened_cannot_record_calls() {
        let store = Store::open(None).expect("in-memory store");
        let engine = Engine::with_registry(DeviceRegistry::new(), None);
        let mut snapshot = WorkspaceSnapshot::empty();
        snapshot.graph.nodes.push(channel_node("dmr", "dmr"));
        let id = store.create_workspace("w", &snapshot).expect("workspace");
        store.activate_workspace(id).expect("activate");

        let (_, policy) = resolve(&store, &engine);

        assert!(policy.channels.is_empty());
    }
}
