use std::{
    collections::{HashMap, HashSet, hash_map::RandomState},
    hash::BuildHasher,
    sync::atomic::{AtomicU64, Ordering},
};

use sdrmm_wire::patch::{
    MAX_EDGES, MAX_NODES, MAX_STREAMS, NodeBody, PatchEdge, PatchGraph, PatchNode, PortRef,
    Position, port_stream,
};

pub const HEADER_PX: f32 = 26.0;
pub const PORT_STEP_PX: f32 = 22.0;
pub const PASTE_OFFSET_PX: f32 = 32.0;
const FIT_MIN_W: f32 = 280.0;
const DIAL_MIN_W: f32 = 420.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeSize {
    pub w: f32,
    pub h: Option<f32>,
}

const fn size(w: f32, h: Option<f32>) -> NodeSize {
    NodeSize { w, h }
}

#[must_use]
pub fn natural_size(kind: &str) -> NodeSize {
    match kind {
        "device" | "recording" | "signal_gen" | "array" | "event_output" => size(420.0, None),
        "gps" | "time_machine" => size(360.0, None),
        "channel" => size(440.0, None),
        "event_filter" | "network_export" | "satellite" | "triangulation" => size(380.0, None),
        "audio_fx" | "scanner" | "df" | "combiner" => size(400.0, None),
        "scope" => size(520.0, Some(360.0)),
        "baseband_scope" => size(420.0, Some(340.0)),
        "speaker" | "spectrum_monitor" | "export" => size(320.0, None),
        "map" => size(520.0, Some(380.0)),
        "signal_map" => size(600.0, Some(440.0)),
        "propagation" => size(640.0, Some(560.0)),
        "readout" => size(560.0, Some(320.0)),
        "decoder_log" => size(720.0, Some(380.0)),
        "dmr_trunk" => size(480.0, Some(360.0)),
        "video" => size(380.0, Some(320.0)),
        "recorder" | "audio_recorder" | "baseband_recorder" | "hunt" | "stitch" => {
            size(340.0, None)
        }
        "passive_radar" => size(520.0, Some(420.0)),
        _ => size(320.0, None),
    }
}

#[must_use]
pub fn is_resizable(kind: &str) -> bool {
    natural_size(kind).h.is_some()
}

#[must_use]
pub fn fit_width(kind: &str) -> Option<(f32, f32)> {
    let natural = natural_size(kind);
    if natural.h.is_some() {
        return None;
    }
    let min = if matches!(
        kind,
        "device" | "signal_gen" | "recording" | "array" | "channel"
    ) {
        DIAL_MIN_W
    } else {
        FIT_MIN_W
    };
    Some((min, min.max(natural.w)))
}

fn resize_floor(kind: &str) -> Option<(f32, f32)> {
    Some(match kind {
        "scope" | "baseband_scope" => (320.0, 200.0),
        "map" => (300.0, 220.0),
        "signal_map" => (400.0, 300.0),
        "propagation" => (440.0, 380.0),
        "readout" => (300.0, 160.0),
        "decoder_log" => (360.0, 200.0),
        "dmr_trunk" => (380.0, 240.0),
        "video" => (240.0, 200.0),
        "passive_radar" => (380.0, 300.0),
        _ => return None,
    })
}

#[must_use]
pub fn min_size(kind: &str, inputs: usize, outputs: usize) -> (f32, f32) {
    let (w, h) = resize_floor(kind).unwrap_or((natural_size(kind).w, 0.0));
    let deepest = inputs.max(outputs) as f32;
    (
        w,
        h.max(HEADER_PX + PORT_STEP_PX / 2.0 + PORT_STEP_PX * deepest),
    )
}

#[must_use]
pub fn edge_key(edge: &PatchEdge) -> String {
    format!(
        "{}:{}->{}:{}",
        edge.from.node, edge.from.port, edge.to.node, edge.to.port
    )
}

static MINTED: AtomicU64 = AtomicU64::new(0);

fn random_hex() -> String {
    let salt = MINTED.fetch_add(1, Ordering::Relaxed);
    let bits = RandomState::new().hash_one(salt);
    format!("{:08x}", bits & 0xffff_ffff)
}

#[must_use]
pub fn new_node_id(kind: &str, taken: &HashSet<String>) -> String {
    loop {
        let id = format!("{kind}:{}", random_hex());
        if !taken.contains(&id) {
            return id;
        }
    }
}

#[must_use]
pub fn node_ids(graph: &PatchGraph) -> HashSet<String> {
    graph.nodes.iter().map(|node| node.id.clone()).collect()
}

#[must_use]
pub fn settle_arrays(mut graph: PatchGraph) -> PatchGraph {
    let highest: HashMap<String, i64> = graph
        .edges
        .iter()
        .filter_map(|edge| {
            port_stream("iq", &edge.to.port).map(|stream| (edge.to.node.clone(), i64::from(stream)))
        })
        .fold(HashMap::new(), |mut most, (node, stream)| {
            let held = most.entry(node).or_insert(-1);
            *held = (*held).max(stream);
            most
        });
    for node in &mut graph.nodes {
        if let NodeBody::Array(array) = &mut node.body {
            let wired = highest.get(&node.id).copied().unwrap_or(-1);
            let members = u32::try_from(wired + 1).unwrap_or(0).min(MAX_STREAMS);
            array.members = members;
        }
    }
    graph
}

#[must_use]
pub fn remove_nodes(graph: &PatchGraph, ids: &[String]) -> PatchGraph {
    settle_arrays(PatchGraph {
        nodes: graph
            .nodes
            .iter()
            .filter(|node| !ids.contains(&node.id))
            .cloned()
            .collect(),
        edges: graph
            .edges
            .iter()
            .filter(|edge| !ids.contains(&edge.from.node) && !ids.contains(&edge.to.node))
            .cloned()
            .collect(),
    })
}

#[must_use]
pub fn remove_edges(graph: &PatchGraph, keys: &[String]) -> PatchGraph {
    settle_arrays(PatchGraph {
        nodes: graph.nodes.clone(),
        edges: graph
            .edges
            .iter()
            .filter(|edge| !keys.contains(&edge_key(edge)))
            .cloned()
            .collect(),
    })
}

#[must_use]
pub fn add_edge(graph: &PatchGraph, edge: PatchEdge) -> PatchGraph {
    let mut next = graph.clone();
    if !next.edges.contains(&edge) {
        next.edges.push(edge);
    }
    settle_arrays(next)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clipboard {
    pub nodes: Vec<PatchNode>,
    pub edges: Vec<PatchEdge>,
}

#[must_use]
pub fn copy_nodes(graph: &PatchGraph, ids: &[String]) -> Option<Clipboard> {
    let nodes: Vec<PatchNode> = graph
        .nodes
        .iter()
        .filter(|node| ids.contains(&node.id))
        .cloned()
        .collect();
    if nodes.is_empty() {
        return None;
    }
    let inside: HashSet<&str> = nodes.iter().map(|node| node.id.as_str()).collect();
    let edges = graph
        .edges
        .iter()
        .filter(|edge| {
            inside.contains(edge.from.node.as_str()) && inside.contains(edge.to.node.as_str())
        })
        .cloned()
        .collect();
    Some(Clipboard { nodes, edges })
}

#[must_use]
pub fn paste_refusal(graph: &PatchGraph, clipboard: &Clipboard) -> Option<String> {
    if graph.nodes.len() + clipboard.nodes.len() > MAX_NODES {
        return Some(format!("a patch holds {MAX_NODES} nodes"));
    }
    if graph.edges.len() + clipboard.edges.len() > MAX_EDGES {
        return Some(format!("a patch holds {MAX_EDGES} wires"));
    }
    None
}

fn copy_of(node: &PatchNode, id: String, offset: Position) -> PatchNode {
    let mut copy = node.clone();
    copy.id = id;
    copy.position = Position {
        x: node.position.x + offset.x,
        y: node.position.y + offset.y,
    };
    match &mut copy.body {
        NodeBody::Device(device) => device.device = None,
        NodeBody::Recording(recording) => *recording = Default::default(),
        _ => {}
    }
    copy
}

#[must_use]
pub fn paste_nodes(
    graph: &PatchGraph,
    clipboard: &Clipboard,
    offset: Position,
) -> (PatchGraph, Vec<String>) {
    let mut taken = node_ids(graph);
    let minted: HashMap<String, String> = clipboard
        .nodes
        .iter()
        .map(|node| {
            let id = new_node_id(node.body.kind(), &taken);
            taken.insert(id.clone());
            (node.id.clone(), id)
        })
        .collect();
    let rename = |id: &str| minted.get(id).cloned().unwrap_or_else(|| id.to_owned());
    let mut next = graph.clone();
    let mut ids = Vec::with_capacity(clipboard.nodes.len());
    for node in &clipboard.nodes {
        let id = rename(&node.id);
        ids.push(id.clone());
        next.nodes.push(copy_of(node, id, offset));
    }
    for edge in &clipboard.edges {
        next.edges.push(PatchEdge {
            from: PortRef {
                node: rename(&edge.from.node),
                port: edge.from.port.clone(),
            },
            to: PortRef {
                node: rename(&edge.to.node),
                port: edge.to.port.clone(),
            },
        });
    }
    (settle_arrays(next), ids)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{ArrayNode, DeviceNode, DeviceRef};

    use super::*;

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 10.0, y: 20.0 },
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

    fn radio(id: &str) -> PatchNode {
        node(
            id,
            NodeBody::Device(DeviceNode {
                device: Some(DeviceRef {
                    backend: "virtual".to_owned(),
                    serial: None,
                    key: Some("siggen".to_owned()),
                }),
                locked_streams: Vec::new(),
            }),
        )
    }

    fn graph() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                radio("dev"),
                node("scope", NodeBody::Scope),
                node("log", NodeBody::DecoderLog),
            ],
            edges: vec![edge(("dev", "iq"), ("scope", "iq"))],
        }
    }

    #[test]
    fn a_node_with_a_height_is_resizable_and_one_without_fits_its_width() {
        assert!(is_resizable("scope"));
        assert!(!is_resizable("channel"));
        assert_eq!(fit_width("channel"), Some((420.0, 440.0)));
        assert_eq!(fit_width("speaker"), Some((280.0, 320.0)));
        assert_eq!(fit_width("scope"), None);
    }

    #[test]
    fn a_node_never_shrinks_past_its_ports() {
        let (_, h) = min_size("scope", 1, 12);
        assert_eq!(h, HEADER_PX + PORT_STEP_PX / 2.0 + PORT_STEP_PX * 12.0);
        assert_eq!(min_size("scope", 1, 1), (320.0, 200.0));
    }

    #[test]
    fn copying_takes_the_wires_between_the_copied_nodes_only() {
        let copied = copy_nodes(&graph(), &["scope".to_owned(), "log".to_owned()]).expect("a copy");
        assert_eq!(copied.nodes.len(), 2);
        assert!(copied.edges.is_empty());
        let both = copy_nodes(&graph(), &["dev".to_owned(), "scope".to_owned()]).expect("a copy");
        assert_eq!(both.edges.len(), 1);
        assert!(copy_nodes(&graph(), &["nothing".to_owned()]).is_none());
    }

    #[test]
    fn a_paste_mints_new_ids_moves_the_copies_and_rewires_them() {
        let source = graph();
        let copied = copy_nodes(&source, &["dev".to_owned(), "scope".to_owned()]).expect("a copy");
        let (pasted, ids) = paste_nodes(&source, &copied, Position { x: 32.0, y: 32.0 });
        assert_eq!(pasted.nodes.len(), 5);
        assert_eq!(pasted.edges.len(), 2);
        assert!(
            ids.iter()
                .all(|id| !source.nodes.iter().any(|node| node.id == *id))
        );
        assert!(ids[0].starts_with("device:"));
        let new_edge = &pasted.edges[1];
        assert_eq!(new_edge.from.node, ids[0]);
        assert_eq!(new_edge.to.node, ids[1]);
        let copy = pasted
            .nodes
            .iter()
            .find(|node| node.id == ids[0])
            .expect("the copy");
        assert_eq!(copy.position.x, 42.0);
        assert!(matches!(&copy.body, NodeBody::Device(device) if device.device.is_none()));
    }

    #[test]
    fn a_paste_past_the_limits_is_refused() {
        let mut full = graph();
        while full.nodes.len() < MAX_NODES {
            let id = format!("n{}", full.nodes.len());
            full.nodes.push(node(&id, NodeBody::Scope));
        }
        let copied = copy_nodes(&full, &["scope".to_owned()]).expect("a copy");
        assert!(paste_refusal(&full, &copied).is_some());
        assert!(paste_refusal(&graph(), &copied).is_none());
    }

    #[test]
    fn an_array_carries_one_input_more_than_its_highest_wired_radio() {
        let mut graph = graph();
        graph
            .nodes
            .push(node("array", NodeBody::Array(ArrayNode::default())));
        graph = add_edge(&graph, edge(("dev", "iq"), ("array", "iq2")));
        let members = |graph: &PatchGraph| match &graph.node("array").map(|node| &node.body) {
            Some(NodeBody::Array(array)) => array.members,
            _ => 99,
        };
        assert_eq!(members(&graph), 2);
        let graph = remove_edges(&graph, &[edge_key(&edge(("dev", "iq"), ("array", "iq2")))]);
        assert_eq!(members(&graph), 0);
    }

    #[test]
    fn removing_a_node_takes_its_wires_with_it() {
        let removed = remove_nodes(&graph(), &["dev".to_owned()]);
        assert_eq!(removed.nodes.len(), 2);
        assert!(removed.edges.is_empty());
    }

    #[test]
    fn minted_ids_are_distinct() {
        let mut taken = HashSet::new();
        for _ in 0..200 {
            let id = new_node_id("scope", &taken);
            assert!(taken.insert(id));
        }
    }
}
