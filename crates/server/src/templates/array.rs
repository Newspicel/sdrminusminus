use sdrmm_wire::{
    ARRAY_LANE_PORT, ARRAY_PORT, ArrayNode, ArrayOrientation, DeviceNode, DfNode, EVENTS_PORT,
    GpsNode, NodeBody, POSITION_PORT, PatchEdge, PatchGraph, PatchNode, PortRef, Position,
    TemplateInfo, TriangulationNode,
};

const KRAKEN_LANES: u32 = 5;
const CENTER_HZ: f64 = 433_920_000.0;
const SAMPLE_RATE: f64 = 2_400_000.0;
const COLUMN: f32 = 520.0;

fn lane_port(base: &str, lane: u32) -> String {
    if lane == 0 {
        base.to_owned()
    } else {
        format!("{base}{}", lane + 1)
    }
}

fn node(id: &str, body: NodeBody, x: f32, y: f32) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x, y },
        size: None,
        label: None,
    }
}

fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
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

fn drive_patch(lanes: u32) -> PatchGraph {
    let array = ArrayNode {
        orientation: ArrayOrientation::Heading {
            mount_offset_deg: 0.0,
        },
        ..ArrayNode::default()
    };
    let nodes = vec![
        node("dev", NodeBody::Device(DeviceNode::default()), 0.0, 0.0),
        node("gps", NodeBody::Gps(GpsNode::default()), 0.0, 420.0),
        node("scope", NodeBody::Scope, COLUMN, -300.0),
        node("array", NodeBody::Array(array), COLUMN, 0.0),
        node("df", NodeBody::Df(DfNode::default()), COLUMN * 2.0, 0.0),
        node(
            "fix",
            NodeBody::Triangulation(TriangulationNode::default()),
            COLUMN * 3.0,
            0.0,
        ),
        node("map", NodeBody::Map, COLUMN * 4.0, 0.0),
    ];
    let mut edges = vec![wire(("dev", "iq"), ("scope", "iq"))];
    edges.extend((0..lanes).map(|lane| {
        wire(
            ("dev", &lane_port("iq", lane)),
            ("array", &lane_port(ARRAY_LANE_PORT, lane)),
        )
    }));
    edges.extend([
        wire(("gps", POSITION_PORT), ("array", POSITION_PORT)),
        wire(("array", ARRAY_PORT), ("df", ARRAY_PORT)),
        wire(("df", EVENTS_PORT), ("fix", EVENTS_PORT)),
        wire(("gps", POSITION_PORT), ("fix", POSITION_PORT)),
        wire(("fix", EVENTS_PORT), ("map", EVENTS_PORT)),
        wire(("gps", POSITION_PORT), ("map", POSITION_PORT)),
    ]);
    PatchGraph { nodes, edges }
}

pub(super) fn df_drive() -> TemplateInfo {
    TemplateInfo {
        id: "df-drive".to_owned(),
        name: "DF drive".to_owned(),
        description: "Five-lane array bearings, fixed on the map, driven to by phone.".to_owned(),
        explainer: "A KrakenSDR or any radio with five lanes on one clock feeds an Array. The \
                    direction finder turns it into bearings, and the fix crosses them as you \
                    drive. Mount the antennas with element 1 facing forward and pick your \
                    paired phone on the GPS node: it gives the Array its place and heading and \
                    gets turn-by-turn directions to the fix. Tune from the phone."
            .to_owned(),
        center_hz: CENTER_HZ,
        sample_rate: SAMPLE_RATE,
        channels: Vec::new(),
        min_freq_hz: CENTER_HZ,
        max_freq_hz: CENTER_HZ,
        patch: Some(drive_patch(KRAKEN_LANES)),
        direction: sdrmm_wire::Direction::Rx,
        supported_devices: Vec::new(),
        min_lanes: KRAKEN_LANES,
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{Coherence, DeviceProfile, Range};

    use super::*;

    fn radio(rx_streams: u32, coherence: Coherence) -> DeviceProfile {
        DeviceProfile {
            freq_ranges: vec![Range {
                min: 24e6,
                max: 1.766e9,
                step: None,
            }],
            sample_rates: vec![SAMPLE_RATE],
            rx_streams,
            coherence,
            ..DeviceProfile::default()
        }
    }

    #[test]
    fn a_kraken_runs_the_drive_and_a_dongle_does_not() {
        let drive = df_drive();
        assert_eq!(drive.unmet_by(&radio(5, Coherence::TimeSync)), None);
        assert_eq!(drive.unmet_by(&radio(8, Coherence::PhaseCoherent)), None);
        assert!(drive.unmet_by(&radio(1, Coherence::None)).is_some());
        assert!(drive.unmet_by(&radio(4, Coherence::TimeSync)).is_some());
        assert!(drive.unmet_by(&radio(5, Coherence::None)).is_some());
    }

    #[test]
    fn every_lane_reaches_the_array_in_order() {
        let patch = drive_patch(KRAKEN_LANES);
        let lanes: Vec<(&str, &str)> = patch
            .edges
            .iter()
            .filter(|edge| edge.to.node == "array" && edge.from.node == "dev")
            .map(|edge| (edge.from.port.as_str(), edge.to.port.as_str()))
            .collect();
        assert_eq!(
            lanes,
            [
                ("iq", "lane"),
                ("iq2", "lane2"),
                ("iq3", "lane3"),
                ("iq4", "lane4"),
                ("iq5", "lane5"),
            ]
        );
    }

    #[test]
    fn the_phone_places_the_array_and_is_guided_to_the_fix() {
        let patch = drive_patch(KRAKEN_LANES);
        let fed: Vec<&str> = patch
            .edges
            .iter()
            .filter(|edge| edge.from.node == "gps")
            .map(|edge| edge.to.node.as_str())
            .collect();
        assert_eq!(fed, ["array", "fix", "map"]);
        assert!(matches!(
            patch.node("array").map(|node| &node.body),
            Some(NodeBody::Array(ArrayNode {
                orientation: ArrayOrientation::Heading { .. },
                ..
            }))
        ));
    }
}
