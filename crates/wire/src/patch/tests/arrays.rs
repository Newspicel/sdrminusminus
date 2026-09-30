use super::*;
use crate::{
    array::{ArrayNode, MAX_ARRAY_LANES},
    device::Coherence,
    processor::df::DfParams,
};

fn array(id: &str) -> PatchNode {
    node(id, NodeBody::Array(ArrayNode::default()))
}

fn labelled(mut node: PatchNode, label: &str) -> PatchNode {
    node.label = Some(label.to_owned());
    node
}

fn radio(id: &str) -> PatchNode {
    node(id, NodeBody::Device(DeviceNode::default()))
}

fn body(kind: &str) -> NodeBody {
    NodeBody::default_for(kind).unwrap_or_else(|| panic!("no default body for {kind}"))
}

fn refusal(graph: &PatchGraph) -> Option<String> {
    match graph.validate() {
        Err(PatchError::Wire { reason, .. }) => Some(reason),
        Err(other) => panic!("expected a wiring refusal, got {other}"),
        Ok(()) => None,
    }
}

fn graph(nodes: Vec<PatchNode>, edges: Vec<PatchEdge>) -> PatchGraph {
    PatchGraph { nodes, edges }
}

#[test]
fn patch_array_output_only_connects_to_array_inputs() {
    let wired = graph(
        vec![radio("dev"), array("arr"), node("df", body("df"))],
        vec![
            edge(("dev", "iq"), ("arr", "lane")),
            edge(("arr", "array"), ("df", "array")),
        ],
    );
    assert!(wired.validate().is_ok());
    assert_eq!(wired.array_of_processor("df"), Some("arr"));

    let into_scope = graph(
        vec![array("arr"), node("scope", NodeBody::Scope)],
        vec![edge(("arr", "array"), ("scope", "iq"))],
    );
    assert_eq!(
        into_scope.validate(),
        Err(PatchError::TypeMismatch {
            from: PortType::Array,
            to: PortType::Iq,
        })
    );

    let radio_into_df = graph(
        vec![radio("dev"), node("df", body("df"))],
        vec![edge(("dev", "iq"), ("df", "array"))],
    );
    assert_eq!(
        radio_into_df.validate(),
        Err(PatchError::TypeMismatch {
            from: PortType::Iq,
            to: PortType::Array,
        })
    );
    assert_eq!(PortType::Array.as_str(), "array");
}

#[test]
fn one_device_lane_feeds_one_array_lane() {
    let twice = graph(
        vec![
            radio("dev"),
            labelled(array("north"), "North mast"),
            array("south"),
        ],
        vec![
            edge(("dev", "iq"), ("north", "lane")),
            edge(("dev", "iq"), ("south", "lane")),
        ],
    );
    assert_eq!(
        refusal(&twice).as_deref(),
        Some("that lane is in North mast")
    );
    let Err(PatchError::Wire { from, to, .. }) = twice.validate() else {
        panic!("a lane in two arrays must be refused");
    };
    assert_eq!((from.node.as_str(), to.node.as_str()), ("dev", "south"));

    let same_array = graph(
        vec![radio("dev"), array("arr")],
        vec![
            edge(("dev", "iq"), ("arr", "lane")),
            edge(("dev", "iq"), ("arr", "lane2")),
        ],
    );
    assert_eq!(
        refusal(&same_array).as_deref(),
        Some("that lane is in an Array")
    );

    let fine = graph(
        vec![radio("dev"), array("arr"), node("scope", NodeBody::Scope)],
        vec![
            edge(("dev", "iq"), ("arr", "lane")),
            edge(("dev", "iq2"), ("arr", "lane2")),
            edge(("dev", "iq"), ("scope", "iq")),
        ],
    );
    assert!(fine.validate().is_ok());
}

#[test]
fn an_array_lane_takes_only_a_radio_lane() {
    for (source, port) in [
        (
            node("gen", NodeBody::SignalGen(SignalGenNode::default())),
            "iq",
        ),
        (node("gen", body("beamformer")), "beam"),
    ] {
        let wrong = graph(
            vec![source, array("arr")],
            vec![edge(("gen", port), ("arr", "lane"))],
        );
        assert_eq!(refusal(&wrong).as_deref(), Some(REFUSAL_ARRAY_LANE));
    }
    let played = graph(
        vec![node("gen", body("recording")), array("arr")],
        vec![edge(("gen", "iq"), ("arr", "lane"))],
    );
    assert!(played.validate().is_ok());
}

#[test]
fn a_recorded_collection_offers_one_iq_output_per_lane() {
    let recording = body("recording");
    let names = |caps: Option<&Capabilities>| -> Vec<String> {
        recording
            .ports_with(caps.map(PortBacking::Device))
            .into_iter()
            .map(|port| port.name)
            .collect()
    };
    assert_eq!(names(None), ["iq"]);
    assert_eq!(names(Some(&capabilities(Duplex::RxOnly, 1, 0))), ["iq"]);
    assert_eq!(
        names(Some(&capabilities(Duplex::RxOnly, 5, 0))),
        ["iq", "iq2", "iq3", "iq4", "iq5"]
    );
    assert_eq!(
        body("signal_gen")
            .ports_with(Some(PortBacking::Device(&capabilities(
                Duplex::RxOnly,
                5,
                0
            ))))
            .len(),
        1
    );

    let played = graph(
        vec![node("rec", body("recording")), array("arr")],
        (0..5)
            .map(|lane| {
                edge(
                    ("rec", stream_port("iq", lane).as_str()),
                    ("arr", stream_port(ARRAY_LANE_PORT, lane).as_str()),
                )
            })
            .collect(),
    );
    assert_eq!(played.validate(), Ok(()));
    assert_eq!(played.array_lanes("arr")[4], Some(("rec", 4)));
}

#[test]
fn array_lanes_report_an_unwired_middle_port() {
    let wired = graph(
        vec![radio("dev"), array("arr")],
        vec![
            edge(("dev", "iq3"), ("arr", "lane3")),
            edge(("dev", "iq"), ("arr", "lane")),
        ],
    );
    assert!(wired.validate().is_ok());
    assert_eq!(
        wired.array_lanes("arr"),
        vec![Some(("dev", 0)), None, Some(("dev", 2))]
    );
    assert_eq!(wired.array_wired_lanes("arr"), 3);
    assert_eq!(wired.array_of_lane("dev", 2), Some("arr"));
    assert_eq!(wired.array_of_lane("dev", 1), None);
    assert!(wired.array_lanes("dev").is_empty());
    assert_eq!(wired.array_wired_lanes("nothing"), 0);
}

#[test]
fn per_lane_ports_grow_with_the_wired_lanes() {
    let array = NodeBody::Array(ArrayNode::default());
    let lanes = |backing: Option<PortBacking<'_>>| -> Vec<String> {
        array
            .ports_with(backing)
            .into_iter()
            .filter(|port| port.port_type == PortType::Iq)
            .map(|port| port.name)
            .collect()
    };
    assert_eq!(lanes(None), ["lane"]);
    assert_eq!(lanes(Some(PortBacking::Lanes(0))), ["lane"]);
    assert_eq!(
        lanes(Some(PortBacking::Lanes(2))),
        ["lane", "lane2", "lane3"]
    );
    let full = lanes(Some(PortBacking::Lanes(MAX_ARRAY_LANES)));
    assert_eq!(full.len(), MAX_ARRAY_LANES as usize);
    assert_eq!(full.last().map(String::as_str), Some("lane16"));
    assert_eq!(
        lanes(Some(PortBacking::Lanes(u32::MAX))).len(),
        MAX_ARRAY_LANES as usize
    );

    let catalog = PatchCatalog::build();
    let entry = catalog
        .nodes
        .iter()
        .find(|entry| entry.kind == "array")
        .expect("array in the palette");
    let lane = &entry.ports[0];
    assert_eq!(lane.name, ARRAY_LANE_PORT);
    assert_eq!(lane.repeat, PortRepeat::PerLane);
    assert!(!lane.multi);
    let json = serde_json::to_value(lane).expect("port json");
    assert_eq!(json["repeat"], "per_lane");
}

#[test]
fn steer_accepts_only_a_df() {
    let steered = graph(
        vec![node("df", body("df")), node("bf", body("beamformer"))],
        vec![edge(("df", "events"), ("bf", "steer"))],
    );
    assert!(steered.validate().is_ok());
    assert_eq!(steered.steer_source("bf"), Some("df"));

    for source in [channel("src", "adsb"), node("src", body("hunt"))] {
        let wrong = graph(
            vec![source, node("bf", body("beamformer"))],
            vec![edge(("src", "events"), ("bf", "steer"))],
        );
        assert_eq!(refusal(&wrong).as_deref(), Some(REFUSAL_STEER));
    }
}

#[test]
fn adsb_accepts_only_adsb_or_a_filter() {
    for source in [
        channel("src", "adsb"),
        node("src", NodeBody::EventFilter(EventFilterNode::default())),
    ] {
        let truth = graph(
            vec![source, node("radar", body("passive_radar"))],
            vec![edge(("src", "events"), ("radar", "adsb"))],
        );
        assert!(truth.validate().is_ok());
    }
    for source in [channel("src", "ais"), node("src", body("df"))] {
        let wrong = graph(
            vec![source, node("radar", body("passive_radar"))],
            vec![edge(("src", "events"), ("radar", "adsb"))],
        );
        assert_eq!(refusal(&wrong).as_deref(), Some(REFUSAL_ADSB));
    }
    let two_decoders = graph(
        vec![
            channel("a", "adsb"),
            channel("b", "adsb"),
            node("radar", body("passive_radar")),
        ],
        vec![
            edge(("a", "events"), ("radar", "adsb")),
            edge(("b", "events"), ("radar", "adsb")),
        ],
    );
    assert!(two_decoders.validate().is_ok());
}

#[test]
fn triangulation_accepts_only_bearing_sources() {
    for source in [
        node("src", body("df")),
        node("src", body("hunt")),
        node("src", NodeBody::EventFilter(EventFilterNode::default())),
    ] {
        let bearings = graph(
            vec![source, node("tri", body("triangulation"))],
            vec![edge(("src", "events"), ("tri", "events"))],
        );
        assert!(bearings.validate().is_ok());
    }
    for source in [channel("src", "adsb"), node("src", body("passive_radar"))] {
        let wrong = graph(
            vec![source, node("tri", body("triangulation"))],
            vec![edge(("src", "events"), ("tri", "events"))],
        );
        assert_eq!(refusal(&wrong).as_deref(), Some(REFUSAL_TRIANGULATION));
    }
}

#[test]
fn a_position_wire_names_its_source() {
    let wired = graph(
        vec![
            node("gps", NodeBody::Gps(GpsNode::default())),
            array("arr"),
            node("hunt", body("hunt")),
            node("tri", body("triangulation")),
            node("radar", body("passive_radar")),
        ],
        vec![
            edge(("gps", "position"), ("arr", "position")),
            edge(("gps", "position"), ("hunt", "position")),
            edge(("gps", "position"), ("tri", "position")),
            edge(("gps", "position"), ("radar", "tx")),
        ],
    );
    assert!(wired.validate().is_ok());
    for id in ["arr", "hunt", "tri"] {
        assert_eq!(wired.position_source(id), Some("gps"));
    }
    assert_eq!(wired.position_source("radar"), None);
    assert_eq!(wired.sources_of("radar", RADAR_TX_PORT).next(), Some("gps"));
}

#[test]
fn every_catalog_kind_has_a_default_body() {
    let catalog = PatchCatalog::build();
    assert_eq!(catalog.nodes.len(), catalog::catalog_rows().count());
    for entry in &catalog.nodes {
        assert_eq!(entry.default_body.kind(), entry.kind);
        assert_eq!(
            NodeBody::default_for(&entry.kind).as_ref(),
            Some(&entry.default_body)
        );
        assert_eq!(entry.category, entry.default_body.category());
    }
    assert_eq!(NodeBody::default_for("combiner"), None);
    let json = serde_json::to_value(&catalog).expect("catalog json");
    assert_eq!(json["nodes"][0]["default_body"]["kind"], "device");
    for kind in [
        "array",
        "df",
        "beamformer",
        "passive_radar",
        "stitch",
        "spatial_spectrum",
        "correlator",
        "polarimeter",
        "triangulation",
    ] {
        let entry = catalog
            .nodes
            .iter()
            .find(|entry| entry.kind == kind)
            .unwrap_or_else(|| panic!("{kind} in the palette"));
        assert_eq!(entry.category, NodeCategory::Tool, "{kind}");
    }
}

#[test]
fn processor_params_follow_the_body() {
    for entry in PatchCatalog::build().nodes {
        let body = &entry.default_body;
        match body.processor_params() {
            Some(params) => {
                assert!(body.is_array_processor(), "{}", entry.kind);
                assert_eq!(params.type_id(), entry.kind);
                assert!(params.valid(), "{}", entry.kind);
            }
            None => assert!(!body.is_array_processor(), "{}", entry.kind),
        }
    }
    let tuned = NodeBody::Df(DfNode {
        settings: DfParams {
            report_ms: 250,
            ..DfParams::default()
        },
    });
    assert_eq!(
        tuned.processor_params(),
        Some(crate::processor::ProcessorParams::Df(DfParams {
            report_ms: 250,
            ..DfParams::default()
        }))
    );
}

#[test]
fn lane_outputs_follow_the_port_table() {
    for entry in PatchCatalog::build().nodes {
        let iq_outputs: Vec<&str> = entry
            .ports
            .iter()
            .filter(|port| port.direction == PortDirection::Out && port.port_type == PortType::Iq)
            .map(|port| port.name.as_str())
            .collect();
        if entry.default_body.is_array_processor() {
            assert_eq!(
                entry.default_body.lane_outputs(),
                iq_outputs,
                "{}",
                entry.kind
            );
        } else {
            assert!(
                entry.default_body.lane_outputs().is_empty(),
                "{}",
                entry.kind
            );
        }
    }
    assert_eq!(body("beamformer").lane_outputs(), [BEAM_PORT]);
    assert_eq!(body("polarimeter").lane_outputs(), [BEAM_PORT]);
    assert_eq!(body("stitch").lane_outputs(), [STITCH_WIDE_PORT]);
    assert!(body("df").lane_outputs().is_empty());
}

#[test]
fn processor_ports_follow_the_contract() {
    let ports = |kind: &str| -> Vec<(String, PortType, PortDirection, bool)> {
        body(kind)
            .ports()
            .into_iter()
            .map(|port| (port.name, port.port_type, port.direction, port.multi))
            .collect()
    };
    let row = |name: &str, ty, dir, multi| (name.to_owned(), ty, dir, multi);
    use PortDirection::{In, Out};
    assert_eq!(
        ports("passive_radar"),
        [
            row("array", PortType::Array, In, false),
            row("tx", PortType::Position, In, false),
            row("adsb", PortType::Events, In, true),
            row("events", PortType::Events, Out, true),
        ]
    );
    assert_eq!(
        ports("triangulation"),
        [
            row("events", PortType::Events, In, true),
            row("position", PortType::Position, In, false),
            row("events", PortType::Events, Out, true),
        ]
    );
    assert_eq!(
        ports("hunt"),
        [
            row("control", PortType::Control, Out, false),
            row("position", PortType::Position, In, false),
            row("events", PortType::Events, Out, true),
        ]
    );
    assert_eq!(
        ports("array"),
        [
            row("lane", PortType::Iq, In, false),
            row("position", PortType::Position, In, false),
            row("array", PortType::Array, Out, true),
        ]
    );
}

#[test]
fn bad_array_and_processor_settings_are_refused() {
    let tier_none = graph(
        vec![node(
            "arr",
            NodeBody::Array(ArrayNode {
                declared: Coherence::None,
                ..ArrayNode::default()
            }),
        )],
        Vec::new(),
    );
    assert!(matches!(tier_none.validate(), Err(PatchError::NodeSettings(id)) if id == "arr"));

    let bad_df = graph(
        vec![node(
            "df",
            NodeBody::Df(DfNode {
                settings: DfParams {
                    max_peaks: 0,
                    ..DfParams::default()
                },
            }),
        )],
        Vec::new(),
    );
    assert!(matches!(bad_df.validate(), Err(PatchError::NodeSettings(id)) if id == "df"));

    let mut hunt = HuntNode::default();
    hunt.sweep.beamwidth_deg = 1.0;
    let bad_hunt = graph(vec![node("hunt", NodeBody::Hunt(hunt))], Vec::new());
    assert!(matches!(bad_hunt.validate(), Err(PatchError::NodeSettings(id)) if id == "hunt"));

    let mut tri = TriangulationNode::default();
    tri.settings.extent_km = 0.0;
    let bad_tri = graph(vec![node("tri", NodeBody::Triangulation(tri))], Vec::new());
    assert!(matches!(bad_tri.validate(), Err(PatchError::NodeSettings(id)) if id == "tri"));
}

#[test]
fn a_hunt_saved_before_the_sweep_loads_with_the_default_sweep() {
    let stored = serde_json::json!({ "kind": "hunt", "data": { "clicks": false } });
    let body: NodeBody = serde_json::from_value(stored).expect("old hunt");
    assert_eq!(
        body,
        NodeBody::Hunt(HuntNode {
            clicks: false,
            sweep: crate::hunt::HuntSweepParams::default(),
        })
    );
}

#[test]
fn a_processor_body_is_tagged_by_its_type_id() {
    let json = serde_json::to_value(body("spatial_spectrum")).expect("body json");
    assert_eq!(json["kind"], "spatial_spectrum");
    assert!(json["data"]["settings"].is_object());
    let bare: NodeBody =
        serde_json::from_value(serde_json::json!({ "kind": "passive_radar", "data": {} }))
            .expect("bare radar");
    assert_eq!(bare, body("passive_radar"));
}

#[test]
fn a_wiring_refusal_reads_as_its_reason() {
    let error = PatchError::Wire {
        from: PortRef {
            node: "a".to_owned(),
            port: "events".to_owned(),
        },
        to: PortRef {
            node: "b".to_owned(),
            port: "steer".to_owned(),
        },
        reason: REFUSAL_STEER.to_owned(),
    };
    assert_eq!(error.to_string(), REFUSAL_STEER);
}
