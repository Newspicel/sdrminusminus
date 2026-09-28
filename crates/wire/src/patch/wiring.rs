use super::{
    ARRAY_LANE_PORT, ARRAY_PORT, EVENTS_PORT, NodeBody, POSITION_PORT, PatchEdge, PatchGraph,
    PortSpec, RADAR_TRUTH_PORT, STEER_PORT, port_stream,
};
use crate::array::MAX_ARRAY_LANES;

pub const REFUSAL_ARRAY_LANE: &str = "an array lane takes a radio lane";
pub const REFUSAL_LANE_TAKEN: &str = "that lane is in {label}";
pub const REFUSAL_UNNAMED_ARRAY: &str = "an Array";
pub const REFUSAL_STEER: &str = "steer takes a direction finder";
pub const REFUSAL_ADSB: &str = "adsb takes ADS-B events";
pub const REFUSAL_TRIANGULATION: &str = "triangulation takes bearings";
pub const REFUSAL_SHARED_CLOCK: &str = "this radio's lanes share no clock";

pub const REFUSALS: [(&str, &str); 7] = [
    ("array_lane", REFUSAL_ARRAY_LANE),
    ("lane_taken", REFUSAL_LANE_TAKEN),
    ("unnamed_array", REFUSAL_UNNAMED_ARRAY),
    ("steer", REFUSAL_STEER),
    ("adsb", REFUSAL_ADSB),
    ("triangulation", REFUSAL_TRIANGULATION),
    ("shared_clock", REFUSAL_SHARED_CLOCK),
];

const ADSB_CHANNEL_TYPE: &str = "adsb";

impl PatchGraph {
    #[must_use]
    pub fn array_lanes(&self, node: &str) -> Vec<Option<(&str, u32)>> {
        let mut lanes: Vec<Option<(&str, u32)>> = Vec::new();
        for edge in self.edges.iter().filter(|edge| edge.to.node == node) {
            let Some(lane) = port_stream(ARRAY_LANE_PORT, &edge.to.port) else {
                continue;
            };
            let Some(stream) = port_stream("iq", &edge.from.port) else {
                continue;
            };
            let index = lane as usize;
            if lanes.len() <= index {
                lanes.resize(index + 1, None);
            }
            lanes[index] = Some((edge.from.node.as_str(), stream));
        }
        lanes
    }

    #[must_use]
    pub fn array_wired_lanes(&self, node: &str) -> u32 {
        u32::try_from(self.array_lanes(node).len()).unwrap_or(MAX_ARRAY_LANES)
    }

    #[must_use]
    pub fn array_of_processor(&self, node: &str) -> Option<&str> {
        self.first_source(node, ARRAY_PORT)
    }

    #[must_use]
    pub fn array_of_lane(&self, device_node: &str, stream: u32) -> Option<&str> {
        self.edges
            .iter()
            .find(|edge| {
                edge.from.node == device_node
                    && port_stream("iq", &edge.from.port) == Some(stream)
                    && self.is_array_lane(edge)
            })
            .map(|edge| edge.to.node.as_str())
    }

    #[must_use]
    pub fn steer_source(&self, beamformer: &str) -> Option<&str> {
        self.first_source(beamformer, STEER_PORT)
    }

    #[must_use]
    pub fn position_source(&self, node: &str) -> Option<&str> {
        self.first_source(node, POSITION_PORT)
    }

    fn first_source(&self, node: &str, port: &str) -> Option<&str> {
        self.edges
            .iter()
            .find(|edge| edge.to.node == node && edge.to.port == port)
            .map(|edge| edge.from.node.as_str())
    }

    fn is_array_lane(&self, edge: &PatchEdge) -> bool {
        port_stream(ARRAY_LANE_PORT, &edge.to.port).is_some()
            && self
                .node(&edge.to.node)
                .is_some_and(|node| matches!(node.body, NodeBody::Array(_)))
    }

    pub(super) fn wiring_refusal(&self, index: usize, input: &PortSpec) -> Option<String> {
        let edge = self.edges.get(index)?;
        let source = &self.node(&edge.from.node)?.body;
        let target = &self.node(&edge.to.node)?.body;
        match (target, input.name.as_str()) {
            (NodeBody::Array(_), ARRAY_LANE_PORT) => self.lane_refusal(index, source),
            (NodeBody::Beamformer(_), STEER_PORT) => (!matches!(source, NodeBody::Df(_))
                || edge.from.port != EVENTS_PORT)
                .then(|| REFUSAL_STEER.to_owned()),
            (NodeBody::PassiveRadar(_), RADAR_TRUTH_PORT) => {
                (!carries_adsb(source)).then(|| REFUSAL_ADSB.to_owned())
            }
            (NodeBody::Triangulation(_), EVENTS_PORT) => (!matches!(
                source,
                NodeBody::Df(_) | NodeBody::Hunt(_) | NodeBody::EventFilter(_)
            ))
            .then(|| REFUSAL_TRIANGULATION.to_owned()),
            _ => None,
        }
    }

    fn lane_refusal(&self, index: usize, source: &NodeBody) -> Option<String> {
        if !matches!(source, NodeBody::Device(_) | NodeBody::Recording(_)) {
            return Some(REFUSAL_ARRAY_LANE.to_owned());
        }
        let edge = self.edges.get(index)?;
        let holder = self.edges[..index]
            .iter()
            .find(|earlier| earlier.from == edge.from && self.is_array_lane(earlier))?;
        let label = self
            .node(&holder.to.node)
            .and_then(|array| array.label.as_deref())
            .unwrap_or(REFUSAL_UNNAMED_ARRAY);
        Some(REFUSAL_LANE_TAKEN.replace("{label}", label))
    }
}

fn carries_adsb(source: &NodeBody) -> bool {
    match source {
        NodeBody::Channel(channel) => channel.channel_type == ADSB_CHANNEL_TYPE,
        NodeBody::EventFilter(_) => true,
        _ => false,
    }
}
