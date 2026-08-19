use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use sdrmm_engine::{Engine, coherent::CoherentUpdate};
use sdrmm_wire::{
    CoherentParams, DF_BEAM_PORT, NodeBody, PatchGraph, RADAR_REFERENCE_PORT,
    RADAR_SURVEILLANCE_PORT, ServerEvent, port_stream, stream_port,
};
use tokio::{sync::broadcast, task::JoinHandle};

use crate::AppState;

/// Where one coherent node in the patch actually lives: which radio it took over, and what the
/// engine calls it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) device_set: u32,
    pub(crate) id: u32,
    pub(crate) kind: &'static str,
    /// The GPS node whose fix says where this array is standing, when one is wired in.
    pub(crate) position_node: Option<String>,
    /// What this receiver calls itself where its bearings are crossed with other receivers'.
    pub(crate) station_id: Option<String>,
    /// The triangulation nodes this finder's bearings are wired into.
    pub(crate) fusion_nodes: Vec<String>,
}

impl Binding {
    /// The name this finder's bearings are filed under. A station that was never named is known
    /// by the node drawing it, which is at least unique.
    pub(crate) fn station(&self, node: &str) -> String {
        self.station_id.clone().unwrap_or_else(|| node.to_owned())
    }
}

/// The one place the patch's node names and the engine's coherent node ids are kept together.
///
/// A coherent node is not a channel: it has no type-and-stream pair to be matched by, so the
/// binding is remembered when it is made rather than worked out again afterwards.
#[derive(Default)]
pub(crate) struct CoherentHub {
    bindings: Mutex<HashMap<String, Binding>>,
    pumps: Mutex<HashMap<u32, JoinHandle<()>>>,
}

impl CoherentHub {
    pub(crate) fn binding(&self, node: &str) -> Option<Binding> {
        self.lock().get(node).cloned()
    }

    pub(crate) fn nodes(&self) -> Vec<(String, Binding)> {
        self.lock()
            .iter()
            .map(|(node, binding)| (node.clone(), binding.clone()))
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Binding>> {
        self.bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn pumps(&self) -> std::sync::MutexGuard<'_, HashMap<u32, JoinHandle<()>>> {
        self.pumps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn forget(&self, node: &str) {
        self.lock().remove(node);
    }

    fn remember(&self, node: String, binding: Binding) {
        self.lock().insert(node, binding);
    }
}

/// Which of the radio's lanes feed a coherent node, in the order its elements are numbered.
///
/// Read off the wires themselves: the port a lane arrives on names the element, and the device
/// port it left names the lane, so a cable swap is a re-wire rather than a recalibration.
#[must_use]
pub(crate) fn wired_lanes(
    graph: &PatchGraph,
    node: &str,
    body: &NodeBody,
) -> Option<(String, Vec<u32>)> {
    let ports: Vec<String> = match body {
        NodeBody::Df(df) => (0..df.settings.geometry.count())
            .map(|element| stream_port("iq", element))
            .collect(),
        NodeBody::Combiner(combiner) => (0..combiner.settings.lanes)
            .map(|element| stream_port("iq", element))
            .collect(),
        NodeBody::PassiveRadar(_) => vec![
            RADAR_REFERENCE_PORT.to_owned(),
            RADAR_SURVEILLANCE_PORT.to_owned(),
        ],
        _ => return None,
    };
    let mut device = None;
    let mut lanes = Vec::with_capacity(ports.len());
    for port in &ports {
        let edge = graph
            .edges
            .iter()
            .find(|edge| edge.to.node == node && edge.to.port == *port)?;
        let source = device.get_or_insert_with(|| edge.from.node.clone());
        if *source != edge.from.node {
            return None;
        }
        lanes.push(port_stream("iq", &edge.from.port)?);
    }
    device.map(|device| (device, lanes))
}

/// The triangulation nodes a finder's bearings are wired into, which is where they are crossed
/// with whatever the other finders on the canvas are seeing.
#[must_use]
pub(crate) fn wired_crossings(graph: &PatchGraph, node: &str) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from.node == node && edge.from.port == "events")
        .filter(|edge| {
            graph
                .node(&edge.to.node)
                .is_some_and(|target| matches!(target.body, NodeBody::Triangulation))
        })
        .map(|edge| edge.to.node.clone())
        .collect()
}

/// The channel node, if any, listening to a direction finder's beam.
#[must_use]
pub(crate) fn beam_listener(graph: &PatchGraph, node: &str) -> Option<String> {
    graph
        .edges
        .iter()
        .find(|edge| edge.from.node == node && edge.from.port == DF_BEAM_PORT)
        .map(|edge| edge.to.node.clone())
}

/// Which channels are listening to a beam rather than to an antenna, and on which lane.
///
/// The aggregator writes the summed array one past the radio's own lanes, so a channel wired to
/// the beam is an ordinary channel on an ordinary lane — it just is not a lane the radio has.
#[must_use]
pub(crate) fn beam_channels(
    state: &crate::AppState,
    graph: &PatchGraph,
    snapshot: &sdrmm_wire::StateSnapshot,
) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    for node in &graph.nodes {
        if !matches!(node.body, NodeBody::Df(_) | NodeBody::Combiner(_)) {
            continue;
        }
        let Some(listener) = beam_listener(graph, &node.id) else {
            continue;
        };
        let Some(binding) = state.coherent.binding(&node.id) else {
            continue;
        };
        let Some(set) = snapshot
            .device_sets
            .iter()
            .find(|set| set.id == binding.device_set)
        else {
            continue;
        };
        out.push((listener, set.id, set.capabilities.rx_streams));
    }
    out
}

#[must_use]
pub(crate) fn settings_of(body: &NodeBody) -> Option<CoherentParams> {
    match body {
        NodeBody::Df(df) => Some(CoherentParams::Df(df.settings.clone())),
        NodeBody::Combiner(combiner) => Some(CoherentParams::Combiner(combiner.settings)),
        NodeBody::PassiveRadar(radar) => Some(CoherentParams::PassiveRadar(radar.settings)),
        _ => None,
    }
}

/// Puts every coherent node the patch draws onto the radio it is wired to, and takes down the
/// ones that are no longer drawn.
pub(crate) fn apply(
    state: &AppState,
    graph: &PatchGraph,
    bound: &[(String, u32)],
) -> Vec<(String, String)> {
    let mut refused = Vec::new();
    let live: Vec<(String, Binding)> = state.coherent.nodes();
    for (node, binding) in live {
        let still_there = graph
            .node(&node)
            .and_then(|node| settings_of(&node.body))
            .is_some();
        if !still_there {
            let _ = state.engine.remove_coherent(binding.device_set, binding.id);
            state.coherent.forget(&node);
        }
    }
    for node in &graph.nodes {
        let Some(params) = settings_of(&node.body) else {
            continue;
        };
        let Some((device_node, lanes)) = wired_lanes(graph, &node.id, &node.body) else {
            refused.push((
                node.id.clone(),
                "every lane of a coherent node has to come from one radio".to_owned(),
            ));
            continue;
        };
        let Some((_, device_set)) = bound.iter().find(|(name, _)| *name == device_node) else {
            continue;
        };
        let kind = match &node.body {
            NodeBody::Df(_) => "df",
            NodeBody::Combiner(_) => "combiner",
            _ => "passive_radar",
        };
        let station_id = match &node.body {
            NodeBody::Df(df) => df.settings.station_id.clone(),
            _ => None,
        };
        let position_node = graph
            .edges
            .iter()
            .find(|edge| edge.to.node == node.id && edge.to.port == "position")
            .map(|edge| edge.from.node.clone());
        let fusion_nodes = wired_crossings(graph, &node.id);
        match state.coherent.binding(&node.id) {
            Some(existing) if existing.device_set == *device_set => {
                if let Err(err) =
                    state
                        .engine
                        .apply_coherent(*device_set, existing.id, params, lanes)
                {
                    refused.push((node.id.clone(), err.to_string()));
                    continue;
                }
                state.coherent.remember(
                    node.id.clone(),
                    Binding {
                        position_node,
                        station_id,
                        fusion_nodes,
                        ..existing
                    },
                );
            }
            existing => {
                if let Some(existing) = existing {
                    let _ = state
                        .engine
                        .remove_coherent(existing.device_set, existing.id);
                }
                match state.engine.add_coherent(*device_set, params, lanes) {
                    Ok(id) => {
                        state.coherent.remember(
                            node.id.clone(),
                            Binding {
                                device_set: *device_set,
                                id,
                                kind,
                                position_node,
                                station_id,
                                fusion_nodes,
                            },
                        );
                        start_pump(state, *device_set);
                    }
                    Err(err) => refused.push((node.id.clone(), err.to_string())),
                }
            }
        }
    }
    refused
}

/// Turns what the aggregator reports into the events every client already listens to, naming the
/// node the operator drew rather than the number the engine gave it.
fn start_pump(state: &AppState, device_set: u32) {
    let mut pumps = state.coherent.pumps();
    if pumps
        .get(&device_set)
        .is_some_and(|pump| !pump.is_finished())
    {
        return;
    }
    let Some(updates) = state.engine.subscribe_coherent(device_set) else {
        return;
    };
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: coherent updates will not reach clients");
        return;
    };
    let _guard = handle.enter();
    let pump_state = PumpState {
        engine: state.engine.clone(),
        hub: state.coherent.clone(),
        fusion: state.fusion.clone(),
        gps: state.gps.clone(),
    };
    let task = tokio::spawn(async move {
        pump(device_set, updates, pump_state).await;
    });
    pumps.insert(device_set, task);
}

struct PumpState {
    engine: Arc<Engine>,
    hub: Arc<CoherentHub>,
    fusion: crate::df_fusion::SharedFusion,
    gps: Arc<crate::gps::GpsHub>,
}

async fn pump(device_set: u32, mut updates: broadcast::Receiver<CoherentUpdate>, state: PumpState) {
    loop {
        let update = match updates.recv().await {
            Ok(update) => update,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Some((node, binding)) = state
            .hub
            .nodes()
            .into_iter()
            .find(|(_, binding)| binding.device_set == device_set && binding.id == update.node)
        else {
            continue;
        };
        if !update.detections.is_empty() {
            state.engine.emit_event(ServerEvent::RadarDetections {
                device_set,
                node,
                detections: update.detections,
            });
            continue;
        }
        let reading = update.reading.unwrap_or_default();
        let fix = binding
            .position_node
            .as_deref()
            .and_then(|node| state.gps.fix(node))
            .or_else(|| state.gps.any_fix());
        let at = format!("{:.9}", jiff::Timestamp::now());
        let station = binding.station(&node);
        if reading.confidence > 0.0 {
            state.engine.publish_decoded(sdrmm_wire::DecodedRecord {
                device_set,
                channel: binding.id,
                at: at.clone(),
                freq_hz: 0.0,
                event: sdrmm_wire::DecoderEvent::Df(sdrmm_wire::DfBearing {
                    bearing_deg: reading.bearing_deg,
                    confidence: reading.confidence,
                    lat: fix.as_ref().map(|fix| fix.latitude),
                    lon: fix.as_ref().map(|fix| fix.longitude),
                    station_id: Some(station.clone()),
                }),
            });
        }
        for crossing in &binding.fusion_nodes {
            let Some(outcome) =
                state
                    .fusion
                    .observe(crossing, &station, &reading, fix.as_ref(), &at)
            else {
                continue;
            };
            if let Some(estimate) = outcome.first_fix {
                state.engine.publish_decoded(sdrmm_wire::DecodedRecord {
                    device_set,
                    channel: binding.id,
                    at: at.clone(),
                    freq_hz: 0.0,
                    event: sdrmm_wire::DecoderEvent::DfFix(estimate),
                });
            }
            state.engine.emit_event(ServerEvent::DfFusionUpdate {
                node: crossing.clone(),
                state: Box::new(outcome.state),
            });
        }
        state.engine.emit_event(ServerEvent::DfUpdate {
            device_set,
            node,
            reading: Box::new(reading),
            cal: Box::new(update.cal),
        });
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        ArrayGeometry, DfNode, DfParams, PassiveRadarNode, PatchEdge, PatchNode, PortRef, Position,
    };

    use super::*;

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.to_owned(),
                port: from.1.to_owned(),
            },
            to: PortRef {
                node: to.0.to_owned(),
                port: to.1.to_owned(),
            },
        }
    }

    fn df_node(count: u32) -> NodeBody {
        NodeBody::Df(DfNode {
            settings: DfParams {
                geometry: ArrayGeometry::Uca {
                    radius_m: 0.35,
                    count,
                },
                ..DfParams::default()
            },
        })
    }

    #[test]
    fn the_wiring_says_which_lane_feeds_which_element() {
        let graph = PatchGraph {
            nodes: vec![
                node("radio", NodeBody::Device(sdrmm_wire::DeviceNode::default())),
                node("df", df_node(4)),
            ],
            edges: vec![
                edge(("radio", "iq3"), ("df", "iq")),
                edge(("radio", "iq"), ("df", "iq2")),
                edge(("radio", "iq4"), ("df", "iq3")),
                edge(("radio", "iq2"), ("df", "iq4")),
            ],
        };
        let body = &graph.node("df").expect("df node").body;
        let (device, lanes) = wired_lanes(&graph, "df", body).expect("a complete wiring");
        assert_eq!(device, "radio");
        assert_eq!(lanes, vec![2, 0, 3, 1]);
    }

    #[test]
    fn a_half_wired_array_binds_to_nothing() {
        let graph = PatchGraph {
            nodes: vec![
                node("radio", NodeBody::Device(sdrmm_wire::DeviceNode::default())),
                node("df", df_node(4)),
            ],
            edges: vec![
                edge(("radio", "iq"), ("df", "iq")),
                edge(("radio", "iq2"), ("df", "iq2")),
            ],
        };
        let body = &graph.node("df").expect("df node").body;
        assert!(wired_lanes(&graph, "df", body).is_none());
    }

    #[test]
    fn lanes_from_two_radios_are_refused() {
        let graph = PatchGraph {
            nodes: vec![
                node("a", NodeBody::Device(sdrmm_wire::DeviceNode::default())),
                node("b", NodeBody::Device(sdrmm_wire::DeviceNode::default())),
                node("df", df_node(2)),
            ],
            edges: vec![
                edge(("a", "iq"), ("df", "iq")),
                edge(("b", "iq"), ("df", "iq2")),
            ],
        };
        let body = &graph.node("df").expect("df node").body;
        assert!(wired_lanes(&graph, "df", body).is_none());
    }

    #[test]
    fn a_radar_reads_its_reference_and_surveillance_ports_in_that_order() {
        let graph = PatchGraph {
            nodes: vec![
                node("radio", NodeBody::Device(sdrmm_wire::DeviceNode::default())),
                node("radar", NodeBody::PassiveRadar(PassiveRadarNode::default())),
            ],
            edges: vec![
                edge(("radio", "iq2"), ("radar", RADAR_REFERENCE_PORT)),
                edge(("radio", "iq"), ("radar", RADAR_SURVEILLANCE_PORT)),
            ],
        };
        let body = &graph.node("radar").expect("radar node").body;
        let (device, lanes) = wired_lanes(&graph, "radar", body).expect("a complete wiring");
        assert_eq!(device, "radio");
        assert_eq!(lanes, vec![1, 0]);
    }
}
