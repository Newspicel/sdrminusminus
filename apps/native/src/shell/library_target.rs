use std::collections::HashMap;

use sdrmm_wire::{
    patch::{NodeBody, PatchGraph},
    state::DeviceSet,
};

use crate::binding;

#[derive(Clone, Debug, PartialEq)]
pub enum TuneTarget {
    Device {
        node: String,
        set: DeviceSet,
        locked: bool,
    },
    Channel {
        node: String,
        set: Option<DeviceSet>,
        locked: bool,
    },
}

impl TuneTarget {
    #[must_use]
    pub fn locked(&self) -> bool {
        match self {
            Self::Device { locked, .. } | Self::Channel { locked, .. } => *locked,
        }
    }
}

#[must_use]
pub fn tuning_locked(graph: &PatchGraph, id: &str) -> bool {
    match graph.node(id).map(|node| &node.body) {
        Some(NodeBody::Device(device)) => device.locked_streams.contains(&0),
        Some(NodeBody::Channel(channel)) => channel.tuning_locked,
        _ => false,
    }
}

#[must_use]
pub fn bound_sets(graph: &PatchGraph, sets: &[DeviceSet]) -> Vec<(String, DeviceSet)> {
    let bound: HashMap<String, u32> = binding::device_sets(graph, sets);
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            let id = bound.get(&node.id)?;
            let set = sets.iter().find(|set| set.id == *id)?;
            Some((node.id.clone(), set.clone()))
        })
        .collect()
}

#[must_use]
pub fn library_target(
    graph: &PatchGraph,
    devices: &[(String, DeviceSet)],
    selected: Option<&str>,
) -> Option<TuneTarget> {
    let set_of = |node: &str| {
        devices
            .iter()
            .find(|(id, _)| id == node)
            .map(|(_, set)| set.clone())
    };
    if let Some(selected) = selected {
        if let Some(set) = set_of(selected) {
            return Some(TuneTarget::Device {
                node: selected.to_owned(),
                set,
                locked: tuning_locked(graph, selected),
            });
        }
        if matches!(
            graph.node(selected).map(|node| &node.body),
            Some(NodeBody::Channel(_))
        ) {
            return Some(TuneTarget::Channel {
                node: selected.to_owned(),
                set: binding::device_node_of(graph, selected).and_then(|owner| set_of(&owner)),
                locked: tuning_locked(graph, selected),
            });
        }
    }
    match devices {
        [(node, set)] => Some(TuneTarget::Device {
            node: node.clone(),
            set: set.clone(),
            locked: tuning_locked(graph, node),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{ChannelNode, DeviceNode, PatchEdge, PatchNode, PortRef, Position};

    use super::*;

    fn set(id: u32) -> DeviceSet {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "device": { "driver": "rtlsdr", "key": id.to_string(), "label": "RTL-SDR" },
            "capabilities": {
                "freq_ranges": [], "sample_rates": [], "gains": [],
                "antennas": [], "bandwidths": [], "duplex": "rx_only"
            },
            "settings": {},
            "status": "running",
            "channels": [],
        }))
        .expect("a device set")
    }

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn channel(locked: bool) -> NodeBody {
        NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
            record_calls: false,
            tuning_locked: locked,
        })
    }

    fn graph() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node("device:1", NodeBody::Device(DeviceNode::default())),
                node("device:2", NodeBody::Device(DeviceNode::default())),
                node("channel:1", channel(false)),
                node("channel:2", channel(true)),
                node("speaker:1", NodeBody::Speaker),
            ],
            edges: vec![PatchEdge {
                from: PortRef {
                    node: "device:1".to_owned(),
                    port: "iq".to_owned(),
                },
                to: PortRef {
                    node: "channel:1".to_owned(),
                    port: "iq".to_owned(),
                },
            }],
        }
    }

    fn one() -> Vec<(String, DeviceSet)> {
        vec![("device:1".to_owned(), set(1))]
    }

    fn two() -> Vec<(String, DeviceSet)> {
        vec![
            ("device:1".to_owned(), set(1)),
            ("device:2".to_owned(), set(2)),
        ]
    }

    #[test]
    fn takes_the_selected_device() {
        assert_eq!(
            library_target(&graph(), &two(), Some("device:2")),
            Some(TuneTarget::Device {
                node: "device:2".to_owned(),
                set: set(2),
                locked: false,
            })
        );
    }

    #[test]
    fn takes_a_selected_channel_with_the_radio_it_is_wired_to() {
        assert_eq!(
            library_target(&graph(), &two(), Some("channel:1")),
            Some(TuneTarget::Channel {
                node: "channel:1".to_owned(),
                set: Some(set(1)),
                locked: false,
            })
        );
    }

    #[test]
    fn keeps_an_unwired_channel_as_a_target_without_a_radio() {
        assert_eq!(
            library_target(&graph(), &two(), Some("channel:2")),
            Some(TuneTarget::Channel {
                node: "channel:2".to_owned(),
                set: None,
                locked: true,
            })
        );
    }

    #[test]
    fn falls_back_to_the_only_device_when_nothing_is_selected() {
        let only = Some(TuneTarget::Device {
            node: "device:1".to_owned(),
            set: set(1),
            locked: false,
        });
        assert_eq!(library_target(&graph(), &one(), None), only);
        assert_eq!(library_target(&graph(), &one(), Some("speaker:1")), only);
    }

    #[test]
    fn has_no_target_when_several_devices_are_drawn_and_none_is_selected() {
        assert_eq!(library_target(&graph(), &two(), None), None);
        assert_eq!(library_target(&graph(), &[], None), None);
    }
}
