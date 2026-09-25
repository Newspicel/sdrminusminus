use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    device::DeviceInfo,
    patch::{DeviceRef, NodeBody, PatchGraph},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelBinding {
    Unwired,
    NoRadio,
    RadioAbsent,
    RadioClosed,
    NotStarted,
}

impl ChannelBinding {
    #[must_use]
    pub fn of(wired: bool, open: bool, named: bool, attached: bool) -> Self {
        if !wired {
            return Self::Unwired;
        }
        if open {
            return Self::NotStarted;
        }
        if !named {
            return Self::NoRadio;
        }
        if attached {
            Self::RadioClosed
        } else {
            Self::RadioAbsent
        }
    }

    #[must_use]
    pub const fn status(self) -> &'static str {
        match self {
            Self::Unwired => "unwired",
            Self::NoRadio => "no radio",
            Self::RadioAbsent => "radio missing",
            Self::RadioClosed => "radio closed",
            Self::NotStarted => "not started",
        }
    }

    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Unwired => "Wire a device's IQ in",
            Self::NoRadio => "Pick a radio on the device node",
            Self::RadioAbsent => "Its radio is not connected",
            Self::RadioClosed => "Its radio is not open",
            Self::NotStarted => "Not started on the radio yet",
        }
    }

    #[must_use]
    pub const fn action(self) -> Option<&'static str> {
        match self {
            Self::RadioClosed => Some("Open radio"),
            Self::NotStarted => Some("Start channel"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IqLane {
    pub source: String,
    pub stream: u32,
}

#[must_use]
pub fn iq_lanes_of(graph: &PatchGraph, node: &str) -> Vec<IqLane> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.to.node == node && edge.to.port == "iq")
        .filter_map(|edge| {
            let stream = port_stream(&edge.from.port)?;
            Some(IqLane {
                source: edge.from.node.clone(),
                stream,
            })
        })
        .collect()
}

fn port_stream(port: &str) -> Option<u32> {
    let suffix = port.strip_prefix("iq")?;
    if suffix.is_empty() {
        return Some(0);
    }
    suffix.parse::<u32>().ok().map(|n| n.saturating_sub(1))
}

fn device_ref_of(graph: &PatchGraph, node: &str) -> Option<DeviceRef> {
    graph
        .node(node)
        .and_then(|found| found.body.device_ref(&found.id))
}

#[must_use]
pub fn radio_ref_of(graph: &PatchGraph, node: &str) -> Option<DeviceRef> {
    let device = crate::binding::device_node_of(graph, node)?;
    device_ref_of(graph, &device)
}

#[must_use]
pub fn radio_refs_of(graph: &PatchGraph, node: &str) -> Vec<DeviceRef> {
    iq_lanes_of(graph, node)
        .iter()
        .filter_map(|lane| device_ref_of(graph, &lane.source))
        .collect()
}

#[must_use]
pub fn radio_is_attached(references: &[DeviceRef], attached: &[DeviceInfo]) -> bool {
    references
        .iter()
        .any(|reference| attached.iter().any(|device| reference.matches(device)))
}

#[must_use]
pub fn tuning_locked(graph: &PatchGraph, node: &str) -> bool {
    graph.node(node).is_some_and(|found| match &found.body {
        NodeBody::Channel(channel) => channel.tuning_locked,
        _ => false,
    })
}

#[must_use]
pub fn locked_channels(graph: &PatchGraph, faces: &HashMap<u32, String>) -> HashSet<u32> {
    faces
        .iter()
        .filter(|(_, node)| tuning_locked(graph, node))
        .map(|(channel, _)| *channel)
        .collect()
}

#[must_use]
pub fn tuning_controller_of(graph: &PatchGraph, channel: &str) -> Option<String> {
    let wire = graph
        .edges
        .iter()
        .find(|edge| edge.to.node == channel && edge.to.port == "control")?;
    let controller = graph.node(&wire.from.node)?;
    let kind = match controller.body {
        NodeBody::Scanner(_) => "Scanner",
        NodeBody::Satellite(_) => "Satellite",
        _ => return None,
    };
    Some(controller.label.clone().unwrap_or_else(|| kind.to_owned()))
}

#[must_use]
pub fn controlled_node_of(graph: &PatchGraph, node: &str) -> Option<String> {
    let driven = graph
        .edges
        .iter()
        .find(|edge| edge.from.node == node && edge.from.port == "control")?;
    let target = graph.node(&driven.to.node)?;
    matches!(target.body, NodeBody::Channel(_)).then(|| target.id.clone())
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{ChannelNode, DeviceNode, PatchEdge, PatchNode, PortRef, Position};

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

    fn channel(kind: &str, locked: bool) -> NodeBody {
        NodeBody::Channel(ChannelNode {
            channel_type: kind.to_owned(),
            record_calls: false,
            tuning_locked: locked,
        })
    }

    fn device(serial: Option<&str>) -> NodeBody {
        NodeBody::Device(DeviceNode {
            device: serial.map(|serial| DeviceRef {
                backend: "rtlsdr".into(),
                serial: Some(serial.into()),
                key: None,
            }),
            locked_streams: Vec::new(),
        })
    }

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.into(),
                port: from.1.into(),
            },
            to: PortRef {
                node: to.0.into(),
                port: to.1.into(),
            },
        }
    }

    fn graph() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node("dev", device(Some("A"))),
                node("blank", device(None)),
                node("nfm", channel("nfm", false)),
                node("loose", channel("am", false)),
            ],
            edges: vec![edge(("dev", "iq"), ("nfm", "iq"))],
        }
    }

    fn rtl(serial: &str) -> DeviceRef {
        DeviceRef {
            backend: "rtlsdr".into(),
            serial: Some(serial.into()),
            key: None,
        }
    }

    #[test]
    fn the_radio_is_the_one_the_upstream_device_node_names() {
        let graph = graph();
        assert_eq!(radio_ref_of(&graph, "nfm"), Some(rtl("A")));
        assert_eq!(radio_ref_of(&graph, "dev"), Some(rtl("A")));
        assert_eq!(radio_ref_of(&graph, "blank"), None);
        assert_eq!(radio_ref_of(&graph, "loose"), None);
        assert_eq!(radio_refs_of(&graph, "nfm"), vec![rtl("A")]);
    }

    #[test]
    fn only_a_named_radio_on_the_bus_counts_as_attached() {
        let attached = vec![DeviceInfo {
            driver: "rtlsdr".into(),
            key: "0".into(),
            label: "RTL-SDR".into(),
            serial: Some("A".into()),
            profile: None,
        }];
        assert!(radio_is_attached(&[rtl("A")], &attached));
        assert!(!radio_is_attached(&[rtl("B")], &attached));
        assert!(!radio_is_attached(&[rtl("A")], &[]));
        assert!(!radio_is_attached(&[], &attached));
        assert!(radio_is_attached(&[rtl("B"), rtl("A")], &attached));
    }

    #[test]
    fn the_wire_is_read_before_the_radio() {
        assert_eq!(
            ChannelBinding::of(false, true, true, true),
            ChannelBinding::Unwired
        );
        assert_eq!(
            ChannelBinding::of(true, true, true, true),
            ChannelBinding::NotStarted
        );
        assert_eq!(
            ChannelBinding::of(true, false, false, true),
            ChannelBinding::NoRadio
        );
        assert_eq!(
            ChannelBinding::of(true, false, true, true),
            ChannelBinding::RadioClosed
        );
        assert_eq!(
            ChannelBinding::of(true, false, true, false),
            ChannelBinding::RadioAbsent
        );
    }

    #[test]
    fn an_action_is_offered_only_where_it_would_do_something() {
        assert_eq!(ChannelBinding::RadioClosed.action(), Some("Open radio"));
        assert_eq!(ChannelBinding::NotStarted.action(), Some("Start channel"));
        assert_eq!(ChannelBinding::RadioAbsent.action(), None);
        assert_eq!(ChannelBinding::NoRadio.action(), None);
        assert_eq!(ChannelBinding::Unwired.action(), None);
    }

    #[test]
    fn each_state_has_a_hint_and_a_short_status() {
        assert_eq!(
            ChannelBinding::RadioAbsent.hint(),
            "Its radio is not connected"
        );
        assert_eq!(ChannelBinding::Unwired.hint(), "Wire a device's IQ in");
        assert_eq!(ChannelBinding::Unwired.status(), "unwired");
        assert_eq!(ChannelBinding::NoRadio.status(), "no radio");
        assert_eq!(ChannelBinding::RadioAbsent.status(), "radio missing");
        assert_eq!(ChannelBinding::RadioClosed.status(), "radio closed");
        assert_eq!(ChannelBinding::NotStarted.status(), "not started");
    }

    #[test]
    fn locked_channels_are_the_live_ones_whose_node_holds_its_frequency() {
        let held = PatchGraph {
            nodes: vec![
                node("nfm", channel("nfm", true)),
                node("am", channel("am", false)),
                node("scope", NodeBody::Scope),
            ],
            edges: Vec::new(),
        };
        let faces = HashMap::from([
            (1, "nfm".to_owned()),
            (2, "am".to_owned()),
            (3, "scope".to_owned()),
            (4, "gone".to_owned()),
        ]);
        assert_eq!(locked_channels(&held, &faces), HashSet::from([1]));
        assert!(locked_channels(&graph(), &faces).is_empty());
    }

    #[test]
    fn a_scanner_or_satellite_on_the_control_input_holds_the_tuning() {
        let mut graph = graph();
        assert_eq!(tuning_controller_of(&graph, "nfm"), None);
        graph.nodes.push(node(
            "scan",
            NodeBody::Scanner(sdrmm_wire::scan::ScannerNode::default()),
        ));
        graph
            .edges
            .push(edge(("scan", "control"), ("nfm", "control")));
        assert_eq!(
            tuning_controller_of(&graph, "nfm").as_deref(),
            Some("Scanner")
        );
        assert_eq!(controlled_node_of(&graph, "scan").as_deref(), Some("nfm"));
        assert_eq!(controlled_node_of(&graph, "dev"), None);
    }

    #[test]
    fn numbered_iq_ports_name_their_lane() {
        let mut graph = graph();
        graph.edges.push(edge(("blank", "iq2"), ("nfm", "iq")));
        assert_eq!(
            iq_lanes_of(&graph, "nfm"),
            vec![
                IqLane {
                    source: "dev".into(),
                    stream: 0
                },
                IqLane {
                    source: "blank".into(),
                    stream: 1
                },
            ]
        );
    }
}
