use sdrmm_wire::{
    device::DeviceInfo,
    patch::{
        ChannelNode, DeviceNode, DeviceRef, NodeBody, PatchEdge, PatchGraph, PatchNode, PortRef,
        Position, Size,
    },
};

const STARTER_CHANNEL: &str = "nfm";

pub const DEVICE_CENTRE_HZ: f64 = 100_000_000.0;
pub const CHANNEL_HZ: f64 = 100_300_000.0;

#[must_use]
pub fn wants_seeding(graph: &PatchGraph) -> bool {
    let has_channel = graph
        .nodes
        .iter()
        .any(|node| matches!(node.body, NodeBody::Channel(_)));
    let has_radio = graph.nodes.iter().any(|node| match &node.body {
        NodeBody::Device(device) => device.device.is_some(),
        _ => false,
    });
    !has_channel && !has_radio
}

pub fn graph(devices: &[DeviceInfo]) -> PatchGraph {
    let device = pick(devices).map(DeviceRef::from_info);
    PatchGraph {
        nodes: vec![
            node(
                "device",
                NodeBody::Device(DeviceNode {
                    device,
                    locked_streams: Vec::new(),
                }),
                40.0,
                72.0,
                None,
            ),
            node(
                "scope",
                NodeBody::Scope,
                470.0,
                24.0,
                Some(Size { w: 620.0, h: 360.0 }),
            ),
            node(
                "nfm",
                NodeBody::Channel(ChannelNode {
                    channel_type: STARTER_CHANNEL.to_owned(),
                    record_calls: false,
                    tuning_locked: false,
                }),
                40.0,
                440.0,
                None,
            ),
            node("speaker", NodeBody::Speaker, 470.0, 470.0, None),
            node("log", NodeBody::DecoderLog, 880.0, 470.0, None),
        ],
        edges: vec![
            edge(("device", "iq"), ("scope", "iq")),
            edge(("device", "iq"), ("nfm", "iq")),
            edge(("nfm", "audio"), ("speaker", "audio")),
        ],
    }
}

fn pick(devices: &[DeviceInfo]) -> Option<&DeviceInfo> {
    devices
        .iter()
        .find(|info| info.driver == "virtual" && info.key == "siggen")
        .or_else(|| devices.first())
}

fn node(id: &str, body: NodeBody, x: f32, y: f32, size: Option<Size>) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x, y },
        size,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(driver: &str, key: &str) -> DeviceInfo {
        DeviceInfo {
            driver: driver.to_owned(),
            key: key.to_owned(),
            label: key.to_owned(),
            serial: None,
            profile: None,
        }
    }

    #[test]
    fn an_untouched_workspace_is_seeded_and_a_used_one_is_left_alone() {
        let untouched = PatchGraph {
            nodes: vec![node(
                "device",
                NodeBody::Device(DeviceNode::default()),
                0.0,
                0.0,
                None,
            )],
            edges: Vec::new(),
        };
        assert!(wants_seeding(&untouched));

        assert!(!wants_seeding(&graph(&[info("virtual", "siggen")])));

        let chosen = PatchGraph {
            nodes: vec![node(
                "device",
                NodeBody::Device(DeviceNode {
                    device: Some(DeviceRef {
                        backend: "rtlsdr".to_owned(),
                        serial: None,
                        key: Some("0".to_owned()),
                    }),
                    tuning_locked: false,
                }),
                0.0,
                0.0,
                None,
            )],
            edges: Vec::new(),
        };
        assert!(!wants_seeding(&chosen));
    }

    #[test]
    fn the_starter_patch_is_a_radio_a_scope_a_channel_and_a_speaker() {
        let graph = graph(&[info("virtual", "siggen")]);
        let ids: Vec<&str> = graph.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(ids, ["device", "scope", "nfm", "speaker", "log"]);
        assert_eq!(graph.edges.len(), 3);
        graph.validate().expect("a valid starter patch");
    }

    #[test]
    fn the_signal_generator_is_preferred_and_anything_else_will_do() {
        let generator = graph(&[info("rtlsdr", "0"), info("virtual", "siggen")]);
        let NodeBody::Device(device) = &generator.nodes[0].body else {
            panic!("the first node is the radio");
        };
        assert_eq!(
            device.device.as_ref().map(|r| r.backend.as_str()),
            Some("virtual")
        );

        let only_hardware = graph(&[info("rtlsdr", "0")]);
        let NodeBody::Device(device) = &only_hardware.nodes[0].body else {
            panic!("the first node is the radio");
        };
        assert_eq!(
            device.device.as_ref().map(|r| r.backend.as_str()),
            Some("rtlsdr")
        );
    }

    #[test]
    fn no_radio_at_all_still_draws_the_patch() {
        let graph = graph(&[]);
        let NodeBody::Device(device) = &graph.nodes[0].body else {
            panic!("the first node is the radio");
        };
        assert!(device.device.is_none());
        graph.validate().expect("a valid starter patch");
    }
}
