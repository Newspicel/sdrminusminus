use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    channel::ChannelInfo,
    patch::{NodeBody, PatchGraph},
    state::DeviceSet,
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

pub fn device_node_of(graph: &PatchGraph, node: &str) -> Option<String> {
    let is_device = |id: &str| {
        graph
            .nodes
            .iter()
            .any(|candidate| candidate.id == id && matches!(candidate.body, NodeBody::Device(_)))
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
    use sdrmm_wire::{
        channel::{ChannelParams, ChannelSettings, NfmParams, Squelch},
        device::{Capabilities, DeviceInfo, DeviceSettings},
        patch::{ChannelNode, DeviceNode, DeviceRef, PatchEdge, PatchNode, PortRef, Position},
        state::DeviceSetStatus,
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
        ChannelInfo {
            id,
            stream,
            settings: ChannelSettings {
                frequency_hz: 100e6,
                squelch: Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: Default::default(),
            },
            out_of_band: false,
            audio_recording: None,
            baseband_recording: None,
            network_export: None,
        }
    }

    fn set(id: u32, key: &str, channels: Vec<ChannelInfo>) -> DeviceSet {
        DeviceSet {
            id,
            device: DeviceInfo {
                driver: "virtual".to_owned(),
                key: key.to_owned(),
                label: key.to_owned(),
                serial: None,
                profile: None,
            },
            capabilities: Capabilities {
                freq_ranges: Vec::new(),
                sample_rates: Vec::new(),
                sample_rate_ranges: Vec::new(),
                gains: Vec::new(),
                antennas: Vec::new(),
                bandwidths: Vec::new(),
                bandwidth_ranges: Vec::new(),
                extra: Vec::new(),
                ppm: false,
                duplex: Default::default(),
                rx_streams: 1,
                tx_streams: 0,
                per_stream: Default::default(),
                directional: None,
                dc_artifact: Default::default(),
                hardware_sweep: false,
                noise_source: false,
                coherence: Default::default(),
            },
            settings: DeviceSettings::default(),
            status: DeviceSetStatus::Running,
            lo_offset_in_force_hz: 0.0,
            channels,
            overruns: 0,
            error: None,
            fault: None,
            recording: None,
            network_export: None,
            time_machine: None,
            scanner: None,
            hunt: None,
            playback: None,
        }
    }

    fn patch() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node(
                    "dev",
                    NodeBody::Device(DeviceNode {
                        device: Some(device_ref("siggen")),
                        tuning_locked: false,
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
                tuning_locked: false,
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
}
