use sdrmm_wire::patch::{NodeCategory, PatchNode, PortBacking, PortDirection, PortSpec, PortType};

pub const DEFAULT_WIDTH: f32 = 320.0;
pub const BAR_HEIGHT: f32 = 26.0;
const FIRST_PORT: f32 = 11.0;
const PORT_PITCH: f32 = 22.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub port_type: PortType,
    pub direction: PortDirection,
    pub x: f32,
    pub y: f32,
}

#[must_use]
pub fn width_of(node: &PatchNode) -> f32 {
    node.size.map_or(DEFAULT_WIDTH, |size| size.w)
}

#[must_use]
pub fn places(node: &PatchNode, backing: Option<PortBacking<'_>>) -> Vec<Place> {
    let width = width_of(node);
    let specs = node.body.ports_with(backing);
    let mut inputs = 0.0;
    let mut outputs = 0.0;
    specs
        .into_iter()
        .map(|spec: PortSpec| {
            let (x, slot) = match spec.direction {
                PortDirection::In => {
                    let slot = inputs;
                    inputs += 1.0;
                    (0.0, slot)
                }
                PortDirection::Out => {
                    let slot = outputs;
                    outputs += 1.0;
                    (width, slot)
                }
            };
            Place {
                name: spec.name,
                port_type: spec.port_type,
                direction: spec.direction,
                x,
                y: BAR_HEIGHT + FIRST_PORT + slot * PORT_PITCH,
            }
        })
        .collect()
}

#[must_use]
pub fn category_class(category: NodeCategory) -> &'static str {
    match category {
        NodeCategory::Source => "source",
        NodeCategory::Channel => "channel",
        NodeCategory::Tool => "tool",
        NodeCategory::Output => "output",
    }
}

#[must_use]
pub fn title_of(node: &PatchNode) -> String {
    if let Some(label) = &node.label {
        return label.clone();
    }
    match &node.body {
        sdrmm_wire::patch::NodeBody::Channel(channel) => channel.channel_type.to_uppercase(),
        body => body.kind().replace('_', " ").to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{ChannelNode, DeviceNode, NodeBody, Position, Size};

    use super::*;

    fn node(body: NodeBody, size: Option<Size>) -> PatchNode {
        PatchNode {
            id: "n".to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size,
            label: None,
        }
    }

    #[test]
    fn inputs_sit_on_the_left_edge_and_outputs_on_the_right() {
        let node = node(NodeBody::Device(DeviceNode::default()), None);
        let places = places(&node, None);
        for place in &places {
            match place.direction {
                PortDirection::In => assert_eq!(place.x, 0.0),
                PortDirection::Out => assert_eq!(place.x, DEFAULT_WIDTH),
            }
        }
        assert!(places.iter().any(|place| place.name == "iq"));
    }

    #[test]
    fn each_side_stacks_its_own_ports_below_the_title_bar() {
        for entry in sdrmm_wire::patch::PatchCatalog::build().nodes {
            let Some(body) = NodeBody::default_for(&entry.kind) else {
                continue;
            };
            let placed = places(&node(body, None), None);
            for side in [PortDirection::In, PortDirection::Out] {
                let heights: Vec<f32> = placed
                    .iter()
                    .filter(|place| place.direction == side)
                    .map(|place| place.y)
                    .collect();
                for (at, y) in heights.iter().enumerate() {
                    assert_eq!(*y, BAR_HEIGHT + FIRST_PORT + PORT_PITCH * at as f32);
                }
            }
        }
    }

    #[test]
    fn a_resized_node_moves_its_outputs_out_to_its_own_edge() {
        let node = node(
            NodeBody::Device(DeviceNode::default()),
            Some(Size { w: 640.0, h: 400.0 }),
        );
        assert_eq!(width_of(&node), 640.0);
        let out = places(&node, None)
            .into_iter()
            .find(|place| place.direction == PortDirection::Out)
            .expect("an output");
        assert_eq!(out.x, 640.0);
    }

    #[test]
    fn a_title_reads_the_label_then_the_channel_then_the_kind() {
        let mut plain = node(NodeBody::Scope, None);
        assert_eq!(title_of(&plain), "SCOPE");
        plain.label = Some("Waterfall".to_owned());
        assert_eq!(title_of(&plain), "Waterfall");

        let channel = node(
            NodeBody::Channel(ChannelNode {
                channel_type: "wfm".to_owned(),
                record_calls: false,
                tuning_locked: false,
            }),
            None,
        );
        assert_eq!(title_of(&channel), "WFM");

        let log = node(NodeBody::DecoderLog, None);
        assert_eq!(title_of(&log), "DECODER LOG");
    }
}
