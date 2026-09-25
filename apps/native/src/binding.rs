use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    AudioRoute, MAX_AUDIO_FX_CHAIN,
    channel::ChannelInfo,
    patch::{NodeBody, PatchGraph},
    state::{DeviceSet, TrunkSystemStatus},
};

pub fn device_sets(graph: &PatchGraph, sets: &[DeviceSet]) -> HashMap<String, u32> {
    let mut bound = HashMap::new();
    let mut claimed: HashSet<u32> = HashSet::new();
    for node in &graph.nodes {
        let Some(reference) = node.body.device_ref(&node.id) else {
            continue;
        };
        let found = sets
            .iter()
            .find(|set| !claimed.contains(&set.id) && reference.matches(&set.device));
        if let Some(set) = found {
            claimed.insert(set.id);
            bound.insert(node.id.clone(), set.id);
        }
    }
    bound
}

pub fn channels(
    graph: &PatchGraph,
    sets: &[DeviceSet],
    devices: &HashMap<String, u32>,
) -> HashMap<String, ChannelInfo> {
    let mut bound = HashMap::new();
    for (device_node, set_id) in devices {
        let Some(set) = sets.iter().find(|set| set.id == *set_id) else {
            continue;
        };
        let mut free: Vec<&ChannelInfo> = set.channels.iter().collect();
        for (node_id, channel_type, stream) in channel_nodes_of(graph, device_node) {
            let at = free.iter().position(|channel| {
                channel.settings.params.type_id() == channel_type && channel.stream == stream
            });
            if let Some(at) = at {
                bound.insert(node_id, free.remove(at).clone());
            }
        }
    }
    bound
}

pub fn iq_source_of(graph: &PatchGraph, node: &str) -> Option<(String, u32)> {
    graph.edges.iter().find_map(|edge| {
        (edge.to.node == node && edge.to.port == "iq")
            .then(|| (edge.from.node.clone(), port_stream(&edge.from.port)))
    })
}

pub fn sources_of(graph: &PatchGraph, node: &str, port: &str) -> Vec<String> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.to.node == node && edge.to.port == port)
        .map(|edge| edge.from.node.clone())
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioSource {
    pub node: String,
    pub fx: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub node: String,
    pub device_set: u32,
    pub channel: ChannelInfo,
    pub fx: Vec<String>,
}

impl Input {
    #[must_use]
    pub fn route(&self) -> AudioRoute {
        AudioRoute {
            device_set: self.device_set,
            channel: self.channel.id,
            fx: self.fx.clone(),
        }
    }
}

pub fn audio_sources_of(graph: &PatchGraph, node: &str) -> Vec<AudioSource> {
    let mut found = Vec::new();
    walk_audio_sources(graph, node, Vec::new(), &mut found);
    found
}

fn walk_audio_sources(
    graph: &PatchGraph,
    node: &str,
    fx: Vec<String>,
    found: &mut Vec<AudioSource>,
) {
    if fx.len() > MAX_AUDIO_FX_CHAIN {
        return;
    }
    for source in sources_of(graph, node, "audio") {
        let filter = graph
            .node(&source)
            .is_some_and(|upstream| matches!(upstream.body, NodeBody::AudioFx(_)));
        if filter {
            let mut chain = Vec::with_capacity(fx.len() + 1);
            chain.push(source.clone());
            chain.extend(fx.iter().cloned());
            walk_audio_sources(graph, &source, chain, found);
        } else {
            found.push(AudioSource {
                node: source,
                fx: fx.clone(),
            });
        }
    }
}

pub fn inputs_of(
    graph: &PatchGraph,
    node: &str,
    port: &str,
    sets: &[DeviceSet],
    trunks: &[TrunkSystemStatus],
) -> Vec<Input> {
    let devices = device_sets(graph, sets);
    let channels = channels(graph, sets, &devices);
    let sources = if port == "audio" {
        audio_sources_of(graph, node)
    } else {
        sources_of(graph, node, port)
            .into_iter()
            .map(|node| AudioSource {
                node,
                fx: Vec::new(),
            })
            .collect()
    };
    let mut inputs = Vec::new();
    for AudioSource { node: source, fx } in sources {
        if let Some(trunk) = trunks.iter().find(|trunk| trunk.node == source) {
            inputs.extend(trunk_inputs(trunk, sets));
            continue;
        }
        let Some(channel) = channels.get(&source) else {
            continue;
        };
        let set = device_node_of(graph, &source).and_then(|owner| devices.get(&owner).copied());
        if let Some(device_set) = set {
            inputs.push(Input {
                node: source,
                device_set,
                channel: channel.clone(),
                fx,
            });
        }
    }
    inputs
}

fn trunk_inputs(trunk: &TrunkSystemStatus, sets: &[DeviceSet]) -> Vec<Input> {
    let wired = |device_set: u32, channel: u32| {
        sets.iter()
            .find(|set| set.id == device_set)
            .and_then(|set| {
                set.channels
                    .iter()
                    .find(|candidate| candidate.id == channel)
            })
            .map(|info| Input {
                node: trunk.node.clone(),
                device_set,
                channel: info.clone(),
                fx: Vec::new(),
            })
    };
    trunk
        .control
        .iter()
        .map(|control| (control.device_set, control.channel))
        .chain(
            trunk
                .followers
                .iter()
                .map(|follower| (follower.device_set, follower.channel)),
        )
        .filter_map(|(device_set, channel)| wired(device_set, channel))
        .collect()
}

pub fn speaker_routes(
    graph: &PatchGraph,
    sets: &[DeviceSet],
    trunks: &[TrunkSystemStatus],
) -> Vec<AudioRoute> {
    graph
        .nodes
        .iter()
        .filter(|node| matches!(node.body, NodeBody::Speaker))
        .flat_map(|node| inputs_of(graph, &node.id, "audio", sets, trunks))
        .map(|input| input.route())
        .collect()
}

pub fn controlled_node_of(graph: &PatchGraph, node: &str) -> Option<String> {
    let driven = graph
        .edges
        .iter()
        .find(|edge| edge.from.node == node && edge.from.port == "control")?;
    graph
        .node(&driven.to.node)
        .filter(|target| matches!(target.body, NodeBody::Channel(_)))
        .map(|target| target.id.clone())
}

pub fn device_node_of(graph: &PatchGraph, node: &str) -> Option<String> {
    let is_device = |id: &str| {
        graph
            .nodes
            .iter()
            .any(|candidate| candidate.id == id && candidate.body.opens_device())
    };
    if is_device(node) {
        return Some(node.to_owned());
    }
    if let Some((source, _)) = iq_source_of(graph, node)
        && is_device(&source)
    {
        return Some(source);
    }
    graph.edges.iter().find_map(|edge| {
        (edge.from.node == node && edge.from.port == "control" && is_device(&edge.to.node))
            .then(|| edge.to.node.clone())
    })
}

fn channel_nodes_of(graph: &PatchGraph, device_node: &str) -> Vec<(String, String, u32)> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let NodeBody::Channel(channel) = &node.body else {
                return None;
            };
            let (source, stream) = iq_source_of(graph, &node.id)?;
            (source == device_node).then(|| (node.id.clone(), channel.channel_type.clone(), stream))
        })
        .collect()
}

fn port_stream(port: &str) -> u32 {
    port.strip_prefix("iq")
        .and_then(|suffix| suffix.parse::<u32>().ok())
        .map_or(0, |n| n.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{
        ChannelNode, DeviceNode, DeviceRef, PatchEdge, PatchNode, PortRef, Position,
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

    fn device_ref(key: &str) -> DeviceRef {
        DeviceRef {
            backend: "virtual".to_owned(),
            serial: None,
            key: Some(key.to_owned()),
        }
    }

    fn channel(id: u32, stream: u32) -> ChannelInfo {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "stream": stream,
            "settings": {
                "frequency_hz": 100e6,
                "params": { "type": "nfm", "settings": {} }
            },
            "out_of_band": false
        }))
        .expect("a channel")
    }

    fn set(id: u32, key: &str, channels: Vec<ChannelInfo>) -> DeviceSet {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "device": { "driver": "virtual", "key": key, "label": key },
            "capabilities": {
                "freq_ranges": [],
                "sample_rates": [],
                "gains": [],
                "antennas": [],
                "bandwidths": []
            },
            "settings": {},
            "status": "running",
            "channels": channels
        }))
        .expect("a device set")
    }

    fn patch() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node(
                    "dev",
                    NodeBody::Device(DeviceNode {
                        device: Some(device_ref("siggen")),
                        locked_streams: Vec::new(),
                    }),
                ),
                node(
                    "nfm",
                    NodeBody::Channel(ChannelNode {
                        channel_type: "nfm".to_owned(),
                        record_calls: false,
                        tuning_locked: false,
                    }),
                ),
                node("speaker", NodeBody::Speaker),
            ],
            edges: vec![
                edge(("dev", "iq"), ("nfm", "iq")),
                edge(("nfm", "audio"), ("speaker", "audio")),
            ],
        }
    }

    #[test]
    fn a_device_node_takes_the_set_its_reference_matches() {
        let graph = patch();
        let sets = vec![set(3, "siggen", vec![])];
        assert_eq!(device_sets(&graph, &sets).get("dev"), Some(&3));
    }

    #[test]
    fn two_device_nodes_never_share_one_set() {
        let mut graph = patch();
        graph.nodes.push(node(
            "dev2",
            NodeBody::Device(DeviceNode {
                device: Some(device_ref("siggen")),
                locked_streams: Vec::new(),
            }),
        ));
        let sets = vec![set(1, "siggen", vec![])];
        let bound = device_sets(&graph, &sets);
        assert_eq!(bound.len(), 1);
        assert_eq!(bound.get("dev"), Some(&1));
    }

    #[test]
    fn a_channel_node_takes_a_channel_of_its_own_type_on_its_own_stream() {
        let graph = patch();
        let sets = vec![set(1, "siggen", vec![channel(7, 0)])];
        let devices = device_sets(&graph, &sets);
        let bound = channels(&graph, &sets, &devices);
        assert_eq!(bound.get("nfm").map(|channel| channel.id), Some(7));
    }

    #[test]
    fn an_unwired_channel_node_binds_to_nothing() {
        let mut graph = patch();
        graph.edges.retain(|edge| edge.to.node != "nfm");
        let sets = vec![set(1, "siggen", vec![channel(7, 0)])];
        let devices = device_sets(&graph, &sets);
        assert!(channels(&graph, &sets, &devices).is_empty());
    }

    #[test]
    fn the_owning_device_is_found_from_a_node_and_from_the_device_itself() {
        let graph = patch();
        assert_eq!(device_node_of(&graph, "nfm").as_deref(), Some("dev"));
        assert_eq!(device_node_of(&graph, "dev").as_deref(), Some("dev"));
        assert_eq!(device_node_of(&graph, "speaker"), None);
    }

    #[test]
    fn a_numbered_iq_port_names_the_stream_it_carries() {
        assert_eq!(port_stream("iq"), 0);
        assert_eq!(port_stream("iq2"), 1);
        assert_eq!(port_stream("audio"), 0);
    }

    #[test]
    fn wires_into_a_port_are_listed_by_their_source() {
        let graph = patch();
        assert_eq!(sources_of(&graph, "speaker", "audio"), vec!["nfm"]);
        assert!(sources_of(&graph, "speaker", "events").is_empty());
    }

    fn fx(id: &str) -> PatchNode {
        node(id, NodeBody::AudioFx(sdrmm_wire::AudioFxNode::default()))
    }

    #[test]
    fn a_speaker_hears_only_the_channels_wired_into_it() {
        let graph = patch();
        let sets = vec![set(1, "siggen", vec![channel(4, 0)])];
        assert_eq!(
            speaker_routes(&graph, &sets, &[]),
            vec![AudioRoute::channel(1, 4)]
        );
        let mut unwired = patch();
        unwired.edges.retain(|edge| edge.to.node != "speaker");
        assert!(speaker_routes(&unwired, &sets, &[]).is_empty());
    }

    #[test]
    fn audio_is_followed_through_a_chain_of_fx_channel_side_first() {
        let mut graph = patch();
        graph.nodes.extend([fx("near"), fx("far")]);
        graph.edges.extend([
            edge(("nfm", "audio"), ("near", "audio")),
            edge(("near", "audio"), ("far", "audio")),
            edge(("far", "audio"), ("speaker", "audio")),
        ]);
        let sets = vec![set(1, "siggen", vec![channel(3, 0)])];
        let routes = speaker_routes(&graph, &sets, &[]);
        assert_eq!(
            routes,
            vec![
                AudioRoute::channel(1, 3),
                AudioRoute {
                    device_set: 1,
                    channel: 3,
                    fx: vec!["near".to_owned(), "far".to_owned()],
                },
            ]
        );
        assert_eq!(
            audio_sources_of(&graph, "far"),
            vec![AudioSource {
                node: "nfm".to_owned(),
                fx: vec!["near".to_owned()],
            }]
        );
    }

    #[test]
    fn an_fx_chain_deeper_than_the_cap_is_given_up() {
        let mut graph = patch();
        graph.edges.retain(|edge| edge.to.node != "speaker");
        let chain: Vec<String> = (0..40).map(|at| format!("fx{at}")).collect();
        graph.nodes.extend(chain.iter().map(|id| fx(id)));
        graph
            .edges
            .push(edge(("nfm", "audio"), (&chain[0], "audio")));
        for pair in chain.windows(2) {
            graph
                .edges
                .push(edge((&pair[0], "audio"), (&pair[1], "audio")));
        }
        graph
            .edges
            .push(edge((&chain[39], "audio"), ("speaker", "audio")));
        assert!(audio_sources_of(&graph, "speaker").is_empty());
    }

    #[test]
    fn a_trunk_wired_in_brings_its_control_and_followers() {
        let mut graph = patch();
        graph
            .edges
            .push(edge(("trunk", "audio"), ("speaker", "audio")));
        let sets = vec![set(
            1,
            "siggen",
            vec![channel(4, 0), channel(5, 0), channel(6, 0)],
        )];
        let trunk: TrunkSystemStatus = serde_json::from_value(serde_json::json!({
            "node": "trunk",
            "carriers": 1,
            "control": { "device_set": 1, "channel": 5, "freq_hz": 1 },
            "followers": [{ "device_set": 1, "channel": 6, "slot": 1, "freq_hz": 2 }],
            "problems": []
        }))
        .expect("a trunk");
        assert_eq!(
            speaker_routes(&graph, &sets, &[trunk]),
            vec![
                AudioRoute::channel(1, 4),
                AudioRoute::channel(1, 5),
                AudioRoute::channel(1, 6),
            ]
        );
    }

    #[test]
    fn a_tool_drives_the_channel_its_control_is_wired_to() {
        let mut graph = patch();
        graph.nodes.push(node(
            "hunt",
            NodeBody::Hunt(sdrmm_wire::patch::HuntNode { clicks: true }),
        ));
        assert_eq!(controlled_node_of(&graph, "hunt"), None);
        graph
            .edges
            .push(edge(("hunt", "control"), ("nfm", "control")));
        assert_eq!(controlled_node_of(&graph, "hunt").as_deref(), Some("nfm"));
        graph
            .edges
            .push(edge(("speaker", "control"), ("dev", "control")));
        assert_eq!(controlled_node_of(&graph, "speaker"), None);
    }

    #[test]
    fn a_baseband_input_lists_its_channel_with_its_set() {
        let mut graph = patch();
        graph
            .nodes
            .push(node("bb", NodeBody::BasebandRecorder(Default::default())));
        graph
            .edges
            .push(edge(("nfm", "baseband"), ("bb", "baseband")));
        let sets = vec![set(2, "siggen", vec![channel(9, 0)])];
        let inputs = inputs_of(&graph, "bb", "baseband", &sets, &[]);
        assert_eq!(inputs.len(), 1);
        assert_eq!((inputs[0].device_set, inputs[0].channel.id), (2, 9));
        assert!(inputs[0].fx.is_empty());
    }
}
