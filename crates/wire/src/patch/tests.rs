use super::*;
use crate::{
    channel::ChannelSettings,
    device::{DcArtifact, Duplex, StreamScope},
    filter::MAX_FILTER_IDS,
};

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn channel(id: &str, ty: &str) -> PatchNode {
    node(
        id,
        NodeBody::Channel(ChannelNode {
            channel_type: ty.to_owned(),
            record_calls: false,
        }),
    )
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

fn capabilities(duplex: Duplex, rx_streams: u32, tx_streams: u32) -> Capabilities {
    Capabilities {
        freq_ranges: Vec::new(),
        sample_rates: Vec::new(),
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: Vec::new(),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: Vec::new(),
        ppm: false,
        duplex,
        rx_streams,
        tx_streams,
        per_stream: StreamScope::default(),
        directional: None,
        dc_artifact: DcArtifact::Operator,
        hardware_sweep: false,
    }
}

fn descriptors() -> Vec<ChannelDescriptor> {
    vec![
        ChannelDescriptor {
            type_id: "nfm".to_owned(),
            name: "NFM".to_owned(),
            bandwidth_hz: 12_500.0,
            input_rate_hz: 48_000.0,
            ..ChannelDescriptor::default()
        },
        ChannelDescriptor {
            type_id: "adsb".to_owned(),
            name: "ADS-B".to_owned(),
            bandwidth_hz: 2_000_000.0,
            input_rate_hz: 2_000_000.0,
            has_audio: false,
            decoder_kind: Some("adsb".to_owned()),
            native_rate_max_hz: Some(4_000_000.0),
            ..ChannelDescriptor::default()
        },
        ChannelDescriptor {
            type_id: "dmr".to_owned(),
            name: "DMR".to_owned(),
            bandwidth_hz: 12_500.0,
            input_rate_hz: 48_000.0,
            decoder_kind: Some(DV_DECODER_KIND.to_owned()),
            ..ChannelDescriptor::default()
        },
    ]
}

fn recording_calls(id: &str, ty: &str, on: bool) -> PatchNode {
    node(
        id,
        NodeBody::Channel(ChannelNode {
            channel_type: ty.to_owned(),
            record_calls: on,
        }),
    )
}

#[test]
fn a_voice_channel_may_record_its_calls() {
    let graph = PatchGraph {
        nodes: vec![recording_calls("dmr", "dmr", true)],
        edges: Vec::new(),
    };
    assert!(graph.validate_against(&descriptors()).is_ok());
}

#[test]
fn a_channel_that_carries_no_voice_cannot_record_calls() {
    let graph = PatchGraph {
        nodes: vec![recording_calls("pager", "adsb", true)],
        edges: Vec::new(),
    };
    assert_eq!(
        graph.validate_against(&descriptors()),
        Err(PatchError::NodeSettings("pager".to_owned()))
    );
}

#[test]
fn recording_nothing_is_always_allowed() {
    let graph = PatchGraph {
        nodes: vec![recording_calls("pager", "adsb", false)],
        edges: Vec::new(),
    };
    assert!(graph.validate_against(&descriptors()).is_ok());
}

#[test]
fn an_event_filter_passes_events_through_and_bounds_its_lists() {
    let ports = ports_for("event_filter");
    assert_eq!(ports.len(), 2);
    assert!(
        ports
            .iter()
            .any(|p| p.direction == PortDirection::In && p.port_type == PortType::Events)
    );
    assert!(
        ports
            .iter()
            .any(|p| p.direction == PortDirection::Out && p.port_type == PortType::Events)
    );

    let graph = PatchGraph {
        nodes: vec![node(
            "filter",
            NodeBody::EventFilter(EventFilterNode {
                kinds: vec!["call".to_owned()],
                ..EventFilterNode::default()
            }),
        )],
        edges: Vec::new(),
    };
    assert!(graph.validate().is_ok());

    let mut invalid = graph;
    let NodeBody::EventFilter(settings) = &mut invalid.nodes[0].body else {
        panic!("event filter node");
    };
    settings.talkgroups = vec![0; MAX_FILTER_IDS + 1];
    assert_eq!(
        invalid.validate(),
        Err(PatchError::NodeSettings("filter".to_owned()))
    );
}

#[test]
fn a_channel_can_reach_an_event_output_through_a_filter() {
    let graph = PatchGraph {
        nodes: vec![
            recording_calls("dmr", "dmr", true),
            node("filter", NodeBody::EventFilter(EventFilterNode::default())),
            node(
                "output",
                NodeBody::EventOutput(EventOutputNode {
                    target: crate::EventOutputTarget::Webhook {
                        url: "https://discord.com/api/webhooks/1/token".to_owned(),
                        format: crate::WebhookFormat::Discord,
                    },
                }),
            ),
        ],
        edges: vec![
            edge(("dmr", "events"), ("filter", "events")),
            edge(("filter", "events"), ("output", "events")),
        ],
    };
    assert!(graph.validate_against(&descriptors()).is_ok());
}

fn workspace() -> PatchGraph {
    PatchGraph {
        nodes: vec![
            node("dev", NodeBody::Device(DeviceNode::default())),
            node("scope", NodeBody::Scope),
            channel("ch", "nfm"),
            node("spk", NodeBody::Speaker),
        ],
        edges: vec![
            edge(("dev", "iq"), ("scope", "iq")),
            edge(("dev", "iq"), ("ch", "iq")),
            edge(("ch", "audio"), ("spk", "audio")),
        ],
    }
}

#[test]
fn topology_ignores_where_a_face_sits_and_what_it_is_called() {
    let graph = workspace();
    let mut moved = graph.clone();
    moved.nodes[0].position = Position { x: 900.0, y: -40.0 };
    moved.nodes[0].size = Some(Size { w: 500.0, h: 320.0 });
    moved.nodes[1].label = Some("Tower".to_owned());
    assert!(graph.same_topology(&moved));

    let mut rewired = graph.clone();
    rewired.edges.pop();
    assert!(!graph.same_topology(&rewired));

    let mut fewer = graph.clone();
    fewer.nodes.pop();
    assert!(!graph.same_topology(&fewer));

    let mut retyped = graph.clone();
    retyped.nodes[1].body = NodeBody::Channel(ChannelNode {
        channel_type: "am".to_owned(),
        record_calls: false,
    });
    assert!(!graph.same_topology(&retyped));

    let mut renamed = graph.clone();
    renamed.nodes[2].id = "spk2".to_owned();
    assert!(!graph.same_topology(&renamed));
}

#[test]
fn node_body_is_adjacently_tagged_and_flattened_onto_the_node() {
    let json = serde_json::to_value(channel("ch", "nfm")).unwrap();
    assert_eq!(json["id"], "ch");
    assert_eq!(json["kind"], "channel");
    assert_eq!(json["data"]["channel_type"], "nfm");
    assert!(json.get("size").is_none());
    assert!(json.get("label").is_none());

    let scope = serde_json::to_value(node("s", NodeBody::Scope)).unwrap();
    assert_eq!(scope["kind"], "scope");
    assert!(scope.get("data").is_none());

    let back: PatchNode = serde_json::from_value(scope).unwrap();
    assert_eq!(back.body, NodeBody::Scope);
}

#[test]
fn a_workspace_validates_structurally_and_against_the_registry() {
    let graph = workspace();
    graph.validate().expect("structurally valid");
    graph
        .validate_against(&descriptors())
        .expect("valid against the registry");
}

#[test]
fn an_unknown_channel_type_is_refused_only_against_the_registry() {
    let mut graph = workspace();
    graph.nodes[2] = channel("ch", "wefax");
    graph.validate().expect("structure alone cannot know");
    assert_eq!(
        graph.validate_against(&descriptors()),
        Err(PatchError::ChannelType("wefax".to_owned()))
    );
}

#[test]
fn a_dmr_trunk_system_takes_its_own_radio_before_a_control_frequency() {
    let wired = |control_hz| {
        PatchGraph {
            nodes: vec![
                node("radio", NodeBody::Device(DeviceNode::default())),
                node(
                    "system",
                    NodeBody::DmrTrunk(DmrTrunkNode {
                        control_hz,
                        ..DmrTrunkNode::default()
                    }),
                ),
            ],
            edges: vec![edge(("radio", "iq"), ("system", "iq"))],
        }
        .validate()
    };

    wired(Some(451_000_000)).expect("a control channel anyone could tune");
    wired(None).expect("the wire comes first, the control channel is named after it");
    assert_eq!(
        wired(Some(0)),
        Err(PatchError::NodeSettings("system".to_owned())),
        "a control channel at nowhere was accepted"
    );
}

#[test]
fn a_dmr_trunk_system_refuses_a_channel_search_it_cannot_run() {
    let searching = |ranges: Vec<DmrSearchRange>| {
        PatchGraph {
            nodes: vec![node(
                "system",
                NodeBody::DmrTrunk(DmrTrunkNode {
                    discovery: DmrDiscovery {
                        enabled: true,
                        ranges,
                        max_probes: 4,
                    },
                    ..DmrTrunkNode::default()
                }),
            )],
            edges: Vec::new(),
        }
        .validate()
    };

    searching(vec![DmrSearchRange {
        start_hz: 451_000_000,
        end_hz: 451_500_000,
        step_hz: 12_500,
    }])
    .expect("a range the search can hold");
    assert_eq!(
        searching(vec![DmrSearchRange {
            start_hz: 450_000_000,
            end_hz: 460_000_000,
            step_hz: 12_500,
        }]),
        Err(PatchError::NodeSettings("system".to_owned())),
        "a range wider than the search can hold was accepted"
    );
    assert_eq!(
        searching(vec![DmrSearchRange {
            start_hz: 451_500_000,
            end_hz: 451_000_000,
            step_hz: 12_500,
        }]),
        Err(PatchError::NodeSettings("system".to_owned()))
    );
    assert_eq!(
        searching(vec![DmrSearchRange {
            start_hz: 451_000_000,
            end_hz: 451_100_000,
            step_hz: 500,
        }]),
        Err(PatchError::NodeSettings("system".to_owned()))
    );
}

#[test]
fn a_dmr_trunk_system_refuses_a_channel_plan_it_cannot_use() {
    let planned = |channel_map: Vec<DmrChannelEntry>| {
        PatchGraph {
            nodes: vec![node(
                "system",
                NodeBody::DmrTrunk(DmrTrunkNode {
                    channel_map,
                    ..DmrTrunkNode::default()
                }),
            )],
            edges: Vec::new(),
        }
        .validate()
    };

    planned(vec![DmrChannelEntry {
        lcn: 17,
        freq_hz: 451_012_500,
    }])
    .expect("a channel anyone could tune");
    assert_eq!(
        planned(vec![DmrChannelEntry {
            lcn: 17,
            freq_hz: 0,
        }]),
        Err(PatchError::NodeSettings("system".to_owned()))
    );
    assert_eq!(
        planned(vec![DmrChannelEntry {
            lcn: MAX_DMR_LOGICAL_CHANNEL + 1,
            freq_hz: 451_012_500,
        }]),
        Err(PatchError::NodeSettings("system".to_owned()))
    );
}

#[test]
fn an_event_output_accepts_decoder_and_completed_call_events() {
    let output = node(
        "output",
        NodeBody::EventOutput(EventOutputNode {
            target: crate::EventOutputTarget::Webhook {
                url: "https://discord.com/api/webhooks/1/token".to_owned(),
                format: crate::WebhookFormat::Discord,
            },
        }),
    );
    let calls = PatchGraph {
        nodes: vec![
            node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
            output.clone(),
        ],
        edges: vec![edge(("system", "events"), ("output", "events"))],
    };
    calls.validate().expect("completed calls");

    let decoded = PatchGraph {
        nodes: vec![channel("carrier", "dmr"), output],
        edges: vec![edge(("carrier", "events"), ("output", "events"))],
    };
    decoded.validate().expect("decoded events");
}

#[test]
fn a_dmr_trunk_records_calls_by_default_and_can_be_told_not_to() {
    assert!(DmrTrunkNode::default().record_calls);
    let graph = PatchGraph {
        nodes: vec![node(
            "system",
            NodeBody::DmrTrunk(DmrTrunkNode {
                record_calls: false,
                ..DmrTrunkNode::default()
            }),
        )],
        edges: Vec::new(),
    };
    assert!(graph.validate().is_ok());
}

#[test]
fn network_export_requires_a_bounded_host_and_nonzero_port() {
    let mut graph = PatchGraph {
        nodes: vec![node(
            "net",
            NodeBody::NetworkExport(NetworkExportNode::default()),
        )],
        edges: Vec::new(),
    };
    graph.validate().expect("default destination");

    let NodeBody::NetworkExport(export) = &mut graph.nodes[0].body else {
        panic!("network export node");
    };
    export.settings.address = "localhost:0".to_owned();
    assert_eq!(
        graph.validate(),
        Err(PatchError::NodeSettings("net".to_owned()))
    );

    let NodeBody::NetworkExport(export) = &mut graph.nodes[0].body else {
        panic!("network export node");
    };
    export.settings.address = "[::1]:7355".to_owned();
    graph.validate().expect("IPv6 destination");
}

#[test]
fn a_network_sink_takes_a_radio_or_a_channel_but_not_both() {
    let graph = |edges: Vec<PatchEdge>| PatchGraph {
        nodes: vec![
            node("dev", NodeBody::Device(DeviceNode::default())),
            channel("ch", "nfm"),
            node("net", NodeBody::NetworkExport(NetworkExportNode::default())),
        ],
        edges,
    };

    graph(vec![edge(("dev", "iq"), ("net", "iq"))])
        .validate()
        .expect("a radio's IQ");
    graph(vec![edge(("ch", "baseband"), ("net", "baseband"))])
        .validate()
        .expect("a channel's baseband");
    assert_eq!(
        graph(vec![
            edge(("dev", "iq"), ("net", "iq")),
            edge(("ch", "baseband"), ("net", "baseband")),
        ])
        .validate(),
        Err(PatchError::MixedNetworkSource("net".to_owned()))
    );
}

#[test]
fn a_baseband_recorder_takes_every_channel_wired_into_it() {
    let graph = PatchGraph {
        nodes: vec![
            channel("a", "nfm"),
            channel("b", "nfm"),
            node("files", NodeBody::BasebandRecorder),
        ],
        edges: vec![
            edge(("a", "baseband"), ("files", "baseband")),
            edge(("b", "baseband"), ("files", "baseband")),
        ],
    };
    graph
        .validate_against(&descriptors())
        .expect("a baseband recorder fans in");
}

#[test]
fn a_time_machine_holds_a_window_the_engine_can_afford() {
    let graph = |seconds: u32| PatchGraph {
        nodes: vec![
            node("dev", NodeBody::Device(DeviceNode::default())),
            node(
                "history",
                NodeBody::TimeMachine(crate::TimeMachineNode {
                    history_seconds: seconds,
                }),
            ),
        ],
        edges: vec![edge(("dev", "iq"), ("history", "iq"))],
    };
    graph(crate::DEFAULT_TIME_MACHINE_SECONDS)
        .validate()
        .expect("the default window");
    for refused in [0, crate::MAX_TIME_MACHINE_SECONDS + 1] {
        assert_eq!(
            graph(refused).validate(),
            Err(PatchError::NodeSettings("history".to_owned())),
            "{refused} s"
        );
    }
}

#[test]
fn a_dmr_trunk_system_takes_a_radio_and_hands_out_events() {
    let graph = PatchGraph {
        nodes: vec![
            node("radio", NodeBody::Device(DeviceNode::default())),
            node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
            node("log", NodeBody::DecoderLog),
        ],
        edges: vec![
            edge(("radio", "iq"), ("system", "iq")),
            edge(("system", "events"), ("log", "events")),
        ],
    };
    graph.validate().expect("matching wire names");

    let carrier = PatchGraph {
        nodes: vec![
            channel("carrier", "dmr"),
            node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
        ],
        edges: vec![edge(("carrier", "events"), ("system", "events"))],
    };
    assert_eq!(
        carrier.validate(),
        Err(PatchError::Direction(PortRef {
            node: "system".to_owned(),
            port: "events".to_owned()
        })),
        "a system that decodes for itself took a decoder's events"
    );
}

#[test]
fn a_conditional_port_is_refused_on_a_type_that_lacks_it() {
    let mut graph = workspace();
    graph.nodes[2] = channel("ch", "adsb");
    let err = graph.validate_against(&descriptors()).unwrap_err();
    assert_eq!(
        err,
        PatchError::UnknownPort(PortRef {
            node: "ch".to_owned(),
            port: "audio".to_owned()
        })
    );
}

#[test]
fn edges_must_name_real_ports_of_matching_type_and_direction() {
    let mut wrong_type = workspace();
    wrong_type.edges.push(edge(("dev", "iq"), ("spk", "audio")));
    assert_eq!(
        wrong_type.validate(),
        Err(PatchError::TypeMismatch {
            from: PortType::Iq,
            to: PortType::Audio
        })
    );

    let mut backwards = workspace();
    backwards.edges = vec![edge(("scope", "iq"), ("dev", "iq"))];
    assert_eq!(
        backwards.validate(),
        Err(PatchError::Direction(PortRef {
            node: "scope".to_owned(),
            port: "iq".to_owned()
        }))
    );

    let mut unknown = workspace();
    unknown.edges = vec![edge(("dev", "iq"), ("scope", "tap"))];
    assert_eq!(
        unknown.validate(),
        Err(PatchError::UnknownPort(PortRef {
            node: "scope".to_owned(),
            port: "tap".to_owned()
        }))
    );

    let mut missing = workspace();
    missing.edges = vec![edge(("ghost", "iq"), ("scope", "iq"))];
    assert_eq!(
        missing.validate(),
        Err(PatchError::UnknownNode("ghost".to_owned()))
    );
}

#[test]
fn a_single_input_takes_one_wire_and_an_output_fans_out() {
    let mut two_devices = workspace();
    two_devices
        .nodes
        .push(node("dev2", NodeBody::Device(DeviceNode::default())));
    two_devices.edges.push(edge(("dev2", "iq"), ("ch", "iq")));
    assert_eq!(
        two_devices.validate(),
        Err(PatchError::PortOccupied(PortRef {
            node: "ch".to_owned(),
            port: "iq".to_owned()
        }))
    );

    let mut fanned = workspace();
    fanned.nodes.push(node("rec", NodeBody::Recorder));
    fanned.edges.push(edge(("dev", "iq"), ("rec", "iq")));
    fanned.validate().expect("iq fans out");
}

#[test]
fn duplicate_wires_and_self_wires_are_refused() {
    let mut duplicate = workspace();
    duplicate.edges.push(edge(("dev", "iq"), ("scope", "iq")));
    assert_eq!(
        duplicate.validate(),
        Err(PatchError::DuplicateEdge(PortRef {
            node: "scope".to_owned(),
            port: "iq".to_owned()
        }))
    );

    let mut loop_back = workspace();
    loop_back.edges = vec![edge(("ch", "audio"), ("ch", "iq"))];
    assert_eq!(
        loop_back.validate(),
        Err(PatchError::SelfEdge("ch".to_owned()))
    );
}

#[test]
fn the_only_type_level_cycle_is_the_guarded_event_transform() {
    let catalog = PatchCatalog::build();
    let reaches = |from: &NodeTypeInfo, to: &NodeTypeInfo| {
        from.ports
            .iter()
            .filter(|port| port.direction == PortDirection::Out)
            .any(|out| {
                to.ports.iter().any(|input| {
                    input.direction == PortDirection::In && input.port_type == out.port_type
                })
            })
    };
    let count = catalog.nodes.len();
    let mut reachable = vec![vec![false; count]; count];
    for (a, row) in reachable.iter_mut().enumerate() {
        for (b, cell) in row.iter_mut().enumerate() {
            *cell = reaches(&catalog.nodes[a], &catalog.nodes[b]);
        }
    }
    for through in 0..count {
        for from in 0..count {
            for to in 0..count {
                reachable[from][to] |= reachable[from][through] && reachable[through][to];
            }
        }
    }
    let cycle: Vec<&str> = (0..count)
        .filter(|&kind| reachable[kind][kind])
        .map(|kind| catalog.nodes[kind].kind.as_str())
        .collect();
    assert_eq!(cycle, vec!["event_filter"]);
}

#[test]
fn every_node_the_palette_offers_round_trips_and_validates_on_its_own() {
    for entry in PatchCatalog::build().nodes {
        let body = default_body(&entry.kind);
        let json = serde_json::to_string(&body).expect("serialize the body");
        let back: NodeBody = serde_json::from_str(&json).unwrap_or_else(|error| {
            panic!("{} does not survive a round trip: {error}", entry.kind)
        });
        assert_eq!(back.kind(), entry.kind);

        let mut node = node("solo", back);
        if let NodeBody::Channel(channel) = &mut node.body {
            channel.channel_type = "nfm".to_owned();
        }
        let graph = PatchGraph {
            nodes: vec![node],
            edges: Vec::new(),
        };
        graph
            .validate()
            .unwrap_or_else(|error| panic!("a fresh {} is invalid: {error}", entry.kind));
    }
}

fn default_body(kind: &str) -> NodeBody {
    match kind {
        "device" => NodeBody::Device(DeviceNode::default()),
        "gps" => NodeBody::Gps(GpsNode::default()),
        "channel" => NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
            record_calls: false,
        }),
        "scope" => NodeBody::Scope,
        "speaker" => NodeBody::Speaker,
        "map" => NodeBody::Map,
        "signal_map" => NodeBody::SignalMap(SignalMapNode::default()),
        "propagation" => NodeBody::Propagation(PropagationNode::default()),
        "readout" => NodeBody::Readout,
        "decoder_log" => NodeBody::DecoderLog,
        "dmr_trunk" => NodeBody::DmrTrunk(DmrTrunkNode::default()),
        "event_filter" => NodeBody::EventFilter(EventFilterNode::default()),
        "event_output" => NodeBody::EventOutput(EventOutputNode::default()),
        "video" => NodeBody::Video,
        "recorder" => NodeBody::Recorder,
        "audio_recorder" => NodeBody::AudioRecorder,
        "baseband_recorder" => NodeBody::BasebandRecorder,
        "time_machine" => NodeBody::TimeMachine(TimeMachineNode::default()),
        "network_export" => NodeBody::NetworkExport(NetworkExportNode::default()),
        "export" => NodeBody::Export,
        "scanner" => NodeBody::Scanner,
        "hunt" => NodeBody::Hunt(HuntNode::default()),
        other => panic!("the palette offers {other}, which this test does not build"),
    }
}

#[test]
fn the_json_the_editor_sends_for_a_fresh_filter_parses() {
    let sent = r#"{
            "id": "filter",
            "position": { "x": 0.0, "y": 0.0 },
            "kind": "event_filter",
            "data": {
                "kinds": [],
                "stations": [],
                "talkgroups": [],
                "radios": [],
                "min_duration_ms": 0
            }
        }"#;

    let parsed: PatchNode = serde_json::from_str(sent).expect("a fresh filter parses");

    assert!(
        matches!(parsed.body, NodeBody::EventFilter(ref f) if f == &EventFilterNode::default())
    );
}

#[test]
fn a_loop_of_wires_is_refused_however_long_it_is() {
    let graph = PatchGraph {
        nodes: vec![
            node("a", NodeBody::EventFilter(EventFilterNode::default())),
            node("b", NodeBody::EventFilter(EventFilterNode::default())),
            node("c", NodeBody::EventFilter(EventFilterNode::default())),
        ],
        edges: vec![
            edge(("a", "events"), ("b", "events")),
            edge(("b", "events"), ("c", "events")),
            edge(("c", "events"), ("a", "events")),
        ],
    };
    assert!(matches!(graph.validate(), Err(PatchError::Cycle(_))));
}

#[test]
fn a_chain_of_filters_is_not_a_loop() {
    let graph = PatchGraph {
        nodes: vec![
            node("a", NodeBody::EventFilter(EventFilterNode::default())),
            node("b", NodeBody::EventFilter(EventFilterNode::default())),
        ],
        edges: vec![edge(("a", "events"), ("b", "events"))],
    };
    assert!(graph.validate().is_ok());
}

#[test]
fn the_reserved_transmit_input_can_take_no_wire() {
    let catalog = PatchCatalog::build();
    let ports = || catalog.nodes.iter().flat_map(|entry| &entry.ports);
    let reserved = ports()
        .find(|port| port.port_type == PortType::Tx)
        .expect("the device node reserves a transmit input");
    assert_eq!(reserved.direction, PortDirection::In);
    assert!(
        reserved.note.is_some(),
        "a port that refuses everything says why"
    );
    assert!(
        !ports().any(|port| port.port_type == PortType::Tx && port.direction == PortDirection::Out),
        "nothing may emit transmit baseband before  gate exists"
    );

    let mut retransmit = workspace();
    retransmit
        .nodes
        .push(node("dev2", NodeBody::Device(DeviceNode::default())));
    retransmit.edges.push(edge(("dev", "iq"), ("dev2", "tx")));
    assert_eq!(
        retransmit.validate(),
        Err(PatchError::TypeMismatch {
            from: PortType::Iq,
            to: PortType::Tx
        })
    );
}

#[test]
fn only_a_radio_that_can_transmit_shows_a_transmit_input() {
    let transmit = ports_for("device")
        .into_iter()
        .find(|port| port.port_type == PortType::Tx)
        .expect("the device kind can have a transmit input");

    let named =
        |port: &PortSpec, caps: &Capabilities| port.applies_to(Some(PortBacking::Device(caps)));
    assert!(
        !named(&transmit, &capabilities(Duplex::RxOnly, 1, 0)),
        "a receiver"
    );
    assert!(
        named(&transmit, &capabilities(Duplex::Half, 1, 1)),
        "a transceiver"
    );
    assert!(!transmit.applies_to(None), "no radio bound");

    for port in ports_for("device")
        .into_iter()
        .filter(|port| port.port_type != PortType::Tx)
    {
        assert!(port.applies_to(None), "{} is not conditional", port.name);
    }
}

#[test]
fn a_scanner_owns_the_one_radio_its_wire_runs_into() {
    let mut driven = workspace();
    driven.nodes.push(node("scan", NodeBody::Scanner));
    driven
        .edges
        .push(edge(("scan", "control"), ("dev", "control")));
    driven.validate().expect("a scanner drives a radio");

    let mut backwards = driven.clone();
    backwards.edges = vec![edge(("dev", "control"), ("scan", "control"))];
    assert_eq!(
        backwards.validate(),
        Err(PatchError::Direction(PortRef {
            node: "dev".to_owned(),
            port: "control".to_owned()
        }))
    );

    let mut two_radios = driven.clone();
    two_radios
        .nodes
        .push(node("dev2", NodeBody::Device(DeviceNode::default())));
    two_radios
        .edges
        .push(edge(("scan", "control"), ("dev2", "control")));
    assert_eq!(
        two_radios.validate(),
        Err(PatchError::PortOccupied(PortRef {
            node: "scan".to_owned(),
            port: "control".to_owned()
        }))
    );

    let mut two_scanners = driven.clone();
    two_scanners.nodes.push(node("scan2", NodeBody::Scanner));
    two_scanners
        .edges
        .push(edge(("scan2", "control"), ("dev", "control")));
    assert_eq!(
        two_scanners.validate(),
        Err(PatchError::PortOccupied(PortRef {
            node: "dev".to_owned(),
            port: "control".to_owned()
        }))
    );
}

#[test]
fn ids_geometry_and_labels_are_bounded() {
    let mut duplicate = workspace();
    duplicate.nodes.push(node("dev", NodeBody::Scope));
    assert_eq!(
        duplicate.validate(),
        Err(PatchError::DuplicateNode("dev".to_owned()))
    );

    let mut empty_id = workspace();
    empty_id.nodes[1].id = String::new();
    assert!(matches!(empty_id.validate(), Err(PatchError::NodeId(_))));

    let mut far_away = workspace();
    far_away.nodes[1].position.x = f32::INFINITY;
    assert_eq!(
        far_away.validate(),
        Err(PatchError::Geometry("scope".to_owned()))
    );

    let mut flat = workspace();
    flat.nodes[1].size = Some(Size { w: 0.0, h: 100.0 });
    assert_eq!(
        flat.validate(),
        Err(PatchError::Geometry("scope".to_owned()))
    );

    let mut long_label = workspace();
    long_label.nodes[1].label = Some("x".repeat(MAX_NAME_LEN + 1));
    assert_eq!(
        long_label.validate(),
        Err(PatchError::Label("scope".to_owned()))
    );
}

#[test]
fn device_refs_match_by_serial_then_key_then_singleton() {
    let hardware = DeviceInfo {
        driver: "rtlsdr".to_owned(),
        key: "0".to_owned(),
        label: "RTL-SDR".to_owned(),
        serial: Some("00000001".to_owned()),
        profile: None,
    };
    let file = DeviceInfo {
        driver: "virtual".to_owned(),
        key: "file:/rec/capture".to_owned(),
        label: "capture".to_owned(),
        serial: None,
        profile: None,
    };
    let siggen = DeviceInfo {
        driver: "virtual".to_owned(),
        key: "siggen".to_owned(),
        label: "Signal Generator".to_owned(),
        serial: None,
        profile: None,
    };

    let by_serial = DeviceRef::from_info(&hardware);
    assert_eq!(by_serial.key, None, "a serial makes the key redundant");
    assert!(by_serial.matches(&hardware));
    assert!(!by_serial.matches(&DeviceInfo {
        key: "1".to_owned(),
        serial: Some("00000002".to_owned()),
        ..hardware.clone()
    }));
    assert!(by_serial.matches(&DeviceInfo {
        key: "3".to_owned(),
        ..hardware.clone()
    }));

    let duo = DeviceInfo {
        driver: "soapy".to_owned(),
        key: "123456@DT".to_owned(),
        label: "Dual Tuner".to_owned(),
        serial: Some("123456".to_owned()),
        profile: None,
    };
    let by_variant = DeviceRef::from_info(&duo);
    assert_eq!(by_variant.key.as_deref(), Some("123456@DT"));
    assert!(by_variant.matches(&duo));
    assert!(!by_variant.matches(&DeviceInfo {
        key: "123456@ST".to_owned(),
        ..duo
    }));

    let by_key = DeviceRef::from_info(&file);
    assert_eq!(by_key.key.as_deref(), Some("file:/rec/capture"));
    assert!(by_key.matches(&file));
    assert!(
        !by_key.matches(&siggen),
        "two serial-less devices of one backend stay distinct"
    );

    let singleton = DeviceRef {
        backend: "hackrf".to_owned(),
        serial: None,
        key: None,
    };
    assert!(singleton.matches(&DeviceInfo {
        driver: "hackrf".to_owned(),
        key: "0".to_owned(),
        label: "HackRF One".to_owned(),
        serial: None,
        profile: None,
    }));
    assert!(!singleton.matches(&hardware));
}

#[test]
fn channels_of_walks_the_wires_in_stored_order() {
    let mut graph = workspace();
    graph.nodes.push(channel("ch2", "adsb"));
    graph.edges.push(edge(("dev", "iq"), ("ch2", "iq")));
    let bound: Vec<(&str, u32)> = graph
        .channels_of("dev")
        .map(|(n, stream)| (n.id.as_str(), stream))
        .collect();
    assert_eq!(bound, vec![("ch", 0), ("ch2", 0)]);
    assert_eq!(graph.channels_of("scope").count(), 0);
}

#[test]
fn channels_of_reports_the_stream_each_channel_taps() {
    let mut graph = workspace();
    graph.nodes.push(channel("ch2", "adsb"));
    graph.edges.push(edge(("dev", "iq3"), ("ch2", "iq")));
    graph.validate().expect("a stream-3 wire is valid");
    let bound: Vec<(&str, u32)> = graph
        .channels_of("dev")
        .map(|(n, stream)| (n.id.as_str(), stream))
        .collect();
    assert_eq!(bound, vec![("ch", 0), ("ch2", 2)]);
}

#[test]
fn stream_ports_number_from_two_and_round_trip() {
    for base in ["iq", "tx"] {
        assert_eq!(stream_port(base, 0), base, "stream 0 keeps the bare name");
        assert_eq!(stream_port(base, 1), format!("{base}2"));
        for index in 0..MAX_STREAMS {
            assert_eq!(port_stream(base, &stream_port(base, index)), Some(index));
        }
    }
    assert_eq!(port_stream("iq", "iq3"), Some(2));
    assert_eq!(
        port_stream("iq", "iq16"),
        Some(15),
        "the last storable name"
    );
}

#[test]
fn a_name_outside_the_family_addresses_no_stream() {
    assert_eq!(port_stream("iq", "iq1"), None);
    assert_eq!(port_stream("iq", "iq0"), None);
    assert_eq!(port_stream("iq", "iqx"), None);
    assert_eq!(port_stream("iq", "iq17"), None, "over MAX_STREAMS");
    assert_eq!(port_stream("iq", "tx2"), None);
    assert_eq!(port_stream("iq", "iq02"), None);
    assert_eq!(port_stream("iq", "iq+2"), None);
    assert_eq!(port_stream("iq", "iq2 "), None);
    assert_eq!(port_stream("iq", ""), None);
}

#[test]
fn ports_with_expands_a_repeating_port_per_stream() {
    let device = NodeBody::Device(DeviceNode::default());
    let names = |caps: &Capabilities| {
        device
            .ports_with(Some(PortBacking::Device(caps)))
            .into_iter()
            .map(|port| port.name)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        names(&capabilities(Duplex::RxOnly, 4, 0)),
        vec!["control", "iq", "iq2", "iq3", "iq4"]
    );
    assert_eq!(
        names(&capabilities(Duplex::Full, 2, 2)),
        vec!["control", "tx", "tx2", "iq", "iq2"]
    );
    assert_eq!(
        names(&capabilities(Duplex::RxOnly, 1, 0)),
        vec!["control", "iq"],
        "a single-stream radio keeps the table it always had"
    );

    let expanded = device.ports_with(Some(PortBacking::Device(&capabilities(Duplex::Full, 2, 2))));
    for port in &expanded {
        assert_eq!(port.repeat, PortRepeat::Once, "{}", port.name);
    }
    let iq2 = expanded.iter().find(|p| p.name == "iq2").unwrap();
    assert_eq!(iq2.port_type, PortType::Iq);
    assert_eq!(iq2.direction, PortDirection::Out);
    assert!(iq2.multi, "each stream fans out on its own");
    let tx2 = expanded.iter().find(|p| p.name == "tx2").unwrap();
    assert_eq!(tx2.condition, PortCondition::DeviceIsTxCapable);
    assert!(tx2.note.is_some(), "every reserved transmit port says why");
}

#[test]
fn an_unbacked_node_expands_to_stream_zero_only() {
    let device = NodeBody::Device(DeviceNode::default());
    let names: Vec<String> = device
        .ports_with(None)
        .into_iter()
        .map(|port| port.name)
        .collect();
    assert_eq!(names, vec!["control", "iq"]);

    let body = NodeBody::Channel(ChannelNode {
        channel_type: "nfm".to_owned(),
        record_calls: false,
    });
    let nfm = &descriptors()[0];
    let names: Vec<String> = body
        .ports_with(Some(PortBacking::Channel(nfm)))
        .into_iter()
        .map(|port| port.name)
        .collect();
    assert_eq!(names, vec!["iq", "baseband", "audio"]);
}

#[test]
fn a_channels_outputs_follow_what_its_type_produces() {
    let names = |descriptor: &ChannelDescriptor| {
        NodeBody::Channel(ChannelNode {
            channel_type: descriptor.type_id.clone(),
            record_calls: false,
        })
        .ports_with(Some(PortBacking::Channel(descriptor)))
        .into_iter()
        .map(|port| port.name)
        .collect::<Vec<_>>()
    };
    let atv = ChannelDescriptor {
        type_id: "atv".to_owned(),
        name: "ATV".to_owned(),
        has_audio: false,
        has_video: true,
        ..ChannelDescriptor::default()
    };
    assert_eq!(names(&atv), vec!["iq", "baseband", "video"]);
    assert_eq!(names(&descriptors()[1]), vec!["iq", "baseband", "events"]);

    let mut graph = workspace();
    graph.nodes.push(node("vid", NodeBody::Video));
    graph.edges.push(edge(("ch", "audio"), ("vid", "video")));
    assert_eq!(
        graph.validate(),
        Err(PatchError::TypeMismatch {
            from: PortType::Audio,
            to: PortType::Video,
        })
    );
}

#[test]
fn a_channel_tap_cannot_be_wired_where_a_wideband_stream_belongs() {
    let catalog = PatchCatalog::build();
    let ports_of = |kind: &str| {
        catalog
            .nodes
            .iter()
            .find(|entry| entry.kind == kind)
            .map(|entry| entry.ports.clone())
            .unwrap_or_default()
    };
    let takes = |kind: &str, port_type: PortType| {
        ports_of(kind)
            .iter()
            .any(|port| port.direction == PortDirection::In && port.port_type == port_type)
    };

    assert!(
        ports_of("channel").iter().any(
            |port| port.direction == PortDirection::Out && port.port_type == PortType::Baseband
        ),
        "a channel taps out its own passband"
    );
    assert!(!takes("channel", PortType::Baseband));
    assert!(!takes("recorder", PortType::Baseband));
    assert!(takes("recorder", PortType::Iq));
    assert!(!takes("recorder", PortType::Audio));
    assert!(takes("audio_recorder", PortType::Audio));
    assert!(!takes("audio_recorder", PortType::Iq));
    assert!(!takes("signal_map", PortType::Baseband));
    assert!(
        takes("scope", PortType::Baseband),
        "the scope is what reads it"
    );

    let mut graph = workspace();
    graph.edges.push(edge(("ch", "baseband"), ("ch", "iq")));
    assert!(graph.validate().is_err());
}

#[test]
fn a_zero_or_outsize_stream_count_is_clamped() {
    let device = NodeBody::Device(DeviceNode::default());
    let names = |caps: &Capabilities| {
        device
            .ports_with(Some(PortBacking::Device(caps)))
            .into_iter()
            .map(|port| port.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&capabilities(Duplex::RxOnly, 0, 0)),
        vec!["control", "iq"]
    );
    let outsize = names(&capabilities(Duplex::RxOnly, 100, 0));
    assert_eq!(outsize.len() as u32, 1 + MAX_STREAMS);
    assert_eq!(outsize.last().map(String::as_str), Some("iq16"));
}

#[test]
fn validation_resolves_the_bounded_stream_family() {
    let mut streamed = workspace();
    streamed.edges[0] = edge(("dev", "iq3"), ("scope", "iq"));
    streamed.validate().expect("iq3 is within the family");
    streamed
        .validate_against(&descriptors())
        .expect("the registry checks do not disturb the family");

    for bad in ["iq17", "iqx", "iq1", "iq0"] {
        let mut graph = workspace();
        graph.edges[0] = edge(("dev", bad), ("scope", "iq"));
        assert_eq!(
            graph.validate(),
            Err(PatchError::UnknownPort(PortRef {
                node: "dev".to_owned(),
                port: bad.to_owned()
            })),
            "{bad} is outside the family"
        );
    }

    let mut channel_family = workspace();
    channel_family.edges[1] = edge(("dev", "iq"), ("ch", "iq2"));
    assert_eq!(
        channel_family.validate(),
        Err(PatchError::UnknownPort(PortRef {
            node: "ch".to_owned(),
            port: "iq2".to_owned()
        }))
    );
}

#[test]
fn arity_is_checked_per_resolved_stream_port() {
    let mut fanned = workspace();
    fanned.edges[0] = edge(("dev", "iq2"), ("scope", "iq"));
    fanned.edges[1] = edge(("dev", "iq2"), ("ch", "iq"));
    fanned.validate().expect("a stream fans out");

    let mut crossed = workspace();
    crossed.edges.push(edge(("dev", "iq3"), ("scope", "iq")));
    assert_eq!(
        crossed.validate(),
        Err(PatchError::PortOccupied(PortRef {
            node: "scope".to_owned(),
            port: "iq".to_owned()
        }))
    );

    let mut doubled = workspace();
    doubled.edges[0] = edge(("dev", "iq2"), ("scope", "iq"));
    doubled.edges.push(edge(("dev", "iq2"), ("scope", "iq")));
    assert_eq!(
        doubled.validate(),
        Err(PatchError::DuplicateEdge(PortRef {
            node: "scope".to_owned(),
            port: "iq".to_owned()
        }))
    );

    let mut backwards = workspace();
    backwards.edges = vec![edge(("ch", "audio"), ("dev", "iq2"))];
    assert_eq!(
        backwards.validate(),
        Err(PatchError::Direction(PortRef {
            node: "dev".to_owned(),
            port: "iq2".to_owned()
        }))
    );
}

#[test]
fn the_rack_is_a_grid_with_no_two_faces_in_one_cell() {
    let graph = workspace();
    let slot = |node: &str, x, y, w, h| RackSlot {
        node: node.to_owned(),
        cell: RackCell { x, y, w, h },
    };

    RackLayout {
        slots: vec![slot("scope", 0, 0, 6, 4), slot("ch", 6, 0, 6, 4)],
    }
    .validate(&graph)
    .expect("side by side");

    assert_eq!(
        RackLayout {
            slots: vec![slot("scope", 0, 0, 6, 4), slot("ch", 3, 2, 6, 4)],
        }
        .validate(&graph),
        Err(PatchError::RackOverlap("ch".to_owned()))
    );

    assert_eq!(
        RackLayout {
            slots: vec![slot("scope", RACK_COLS - 1, 0, 2, 2)],
        }
        .validate(&graph),
        Err(PatchError::RackCell("scope".to_owned()))
    );

    assert_eq!(
        RackLayout {
            slots: vec![slot("ghost", 0, 0, 1, 1)],
        }
        .validate(&graph),
        Err(PatchError::UnknownNode("ghost".to_owned()))
    );

    assert_eq!(
        RackLayout {
            slots: vec![slot("scope", 0, 0, 2, 2), slot("scope", 4, 0, 2, 2)],
        }
        .validate(&graph),
        Err(PatchError::DuplicateRackSlot("scope".to_owned()))
    );
}

#[test]
fn the_catalog_describes_every_node_kind_once() {
    let catalog = PatchCatalog::build();
    for node in &catalog.nodes {
        for port in &node.ports {
            assert_eq!(port.name, port.port_type.as_str());
        }
    }
    let mut kinds: Vec<&str> = catalog.nodes.iter().map(|n| n.kind.as_str()).collect();
    let total = kinds.len();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), total, "a kind is listed twice");

    let channel = catalog
        .nodes
        .iter()
        .find(|n| n.kind == "channel")
        .expect("channel in the palette");
    assert!(channel.needs_channel_type);
    assert_eq!(
        channel
            .ports
            .iter()
            .find(|p| p.name == "audio")
            .map(|p| p.condition),
        Some(PortCondition::ChannelHasAudio)
    );

    let json = serde_json::to_value(&catalog).unwrap();
    assert_eq!(json["nodes"][0]["kind"], "device");
    assert_eq!(json["nodes"][0]["name"], "Device");
    assert_eq!(json["nodes"][0]["category"], "source");
    let ports = &json["nodes"][0]["ports"];
    assert_eq!(ports[0]["port_type"], "control");
    assert_eq!(ports[0]["direction"], "in");
    assert!(
        ports[0].get("repeat").is_none(),
        "the common case stays off the wire"
    );
    assert_eq!(ports[1]["port_type"], "tx");
    assert_eq!(ports[1]["condition"], "device_is_tx_capable");
    assert_eq!(ports[1]["repeat"], "per_tx_stream");
    assert!(ports[1]["note"].is_string(), "the reserved port says why");
    assert_eq!(ports[2]["port_type"], "iq");
    assert_eq!(ports[2]["direction"], "out");
    assert_eq!(ports[2]["repeat"], "per_rx_stream");
    assert!(
        ports[2].get("condition").is_none() && ports[2].get("note").is_none(),
        "the common case stays off the wire"
    );

    let back: PatchCatalog = serde_json::from_value(json).unwrap();
    assert_eq!(back, catalog);
    let bare: PortSpec =
        serde_json::from_str(r#"{"name":"iq","port_type":"iq","direction":"in","multi":false}"#)
            .unwrap();
    assert_eq!(bare.repeat, PortRepeat::Once);

    let gps = catalog
        .nodes
        .iter()
        .find(|node| node.kind == "gps")
        .expect("GPS source in the palette");
    assert_eq!(gps.ports[0].port_type, PortType::Position);
    let position = channel
        .ports
        .iter()
        .find(|port| port.name == "position")
        .expect("position input");
    assert_eq!(position.condition, PortCondition::ChannelNeedsPosition);

    let signal_map = catalog
        .nodes
        .iter()
        .find(|node| node.kind == "signal_map")
        .expect("signal survey in the palette");
    assert_eq!(signal_map.category, NodeCategory::Display);
    assert_eq!(
        signal_map
            .ports
            .iter()
            .map(|port| port.port_type)
            .collect::<Vec<_>>(),
        [PortType::Iq, PortType::Position]
    );
}

#[test]
fn signal_map_settings_are_bounded() {
    let mut graph = PatchGraph {
        nodes: vec![node(
            "survey",
            NodeBody::SignalMap(SignalMapNode::default()),
        )],
        edges: Vec::new(),
    };
    assert_eq!(graph.validate(), Ok(()));

    let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
        panic!("signal map");
    };
    settings.offset_hz = MAX_SIGNAL_MAP_OFFSET_HZ + 1;
    assert_eq!(
        graph.validate(),
        Err(PatchError::NodeSettings("survey".to_owned()))
    );

    let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
        panic!("signal map");
    };
    settings.offset_hz = -(MAX_SIGNAL_MAP_OFFSET_HZ + 1);
    assert_eq!(
        graph.validate(),
        Err(PatchError::NodeSettings("survey".to_owned()))
    );

    let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
        panic!("signal map");
    };
    settings.offset_hz = DEFAULT_SIGNAL_MAP_OFFSET_HZ;
    settings.bandwidth_hz = 0;
    assert_eq!(
        graph.validate(),
        Err(PatchError::NodeSettings("survey".to_owned()))
    );
}

#[test]
fn propagation_takes_decoder_events_and_a_station_position() {
    let catalog = PatchCatalog::build();
    let propagation = catalog
        .nodes
        .iter()
        .find(|node| node.kind == "propagation")
        .expect("propagation map in the palette");
    assert_eq!(propagation.category, NodeCategory::Display);
    assert_eq!(
        propagation
            .ports
            .iter()
            .map(|port| (port.port_type, port.direction))
            .collect::<Vec<_>>(),
        [
            (PortType::Events, PortDirection::In),
            (PortType::Position, PortDirection::In),
        ]
    );
}

#[test]
fn propagation_settings_are_bounded() {
    let mut graph = PatchGraph {
        nodes: vec![node(
            "prop",
            NodeBody::Propagation(crate::propagation::PropagationNode::default()),
        )],
        edges: Vec::new(),
    };
    assert_eq!(graph.validate(), Ok(()));

    for broken in [
        crate::propagation::PropagationNode {
            half_life_minutes: 0,
            ..Default::default()
        },
        crate::propagation::PropagationNode {
            reflection_height_km: 1_000,
            ..Default::default()
        },
    ] {
        graph.nodes[0].body = NodeBody::Propagation(broken);
        assert_eq!(
            graph.validate(),
            Err(PatchError::NodeSettings("prop".to_owned()))
        );
    }
}

#[test]
fn gps_source_settings_are_structurally_bounded() {
    let mut graph = PatchGraph {
        nodes: vec![node(
            "gps",
            NodeBody::Gps(GpsNode {
                source: PositionSource::Nmea {
                    device: "/dev/ttyUSB0".to_owned(),
                    baud: 9_600,
                    update_interval_ms: 1_000,
                },
            }),
        )],
        edges: Vec::new(),
    };
    assert_eq!(graph.validate(), Ok(()));
    if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
        gps.source = PositionSource::Nmea {
            device: "/dev/ttyUSB0".to_owned(),
            baud: 9_600,
            update_interval_ms: 49,
        };
    }
    assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
    if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
        gps.source = PositionSource::Gpsd {
            address: String::new(),
        };
    }
    assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
    if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
        gps.source = PositionSource::Gpsd {
            address: "not-an-endpoint".to_owned(),
        };
    }
    assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
    for address in [
        "localhost:0",
        "localhost:gps",
        "[::1:2947",
        "[::1]]:2947",
        "bad host:2947",
    ] {
        if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
            gps.source = PositionSource::Gpsd {
                address: address.to_owned(),
            };
        }
        assert!(
            matches!(graph.validate(), Err(PatchError::Gps(_))),
            "accepted invalid GPSD endpoint {address}"
        );
    }
    if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
        gps.source = PositionSource::Gpsd {
            address: "[::1]:2947".to_owned(),
        };
    }
    assert_eq!(graph.validate(), Ok(()));
}

#[test]
fn default_params_come_from_the_type_id() {
    let params = ChannelParams::default_for("ssb").expect("ssb is a channel type");
    assert_eq!(params.type_id(), "ssb");
    assert_eq!(
        ChannelSettings::default_for("ssb").expect("ssb is a channel type"),
        serde_json::from_str(r#"{"params":{"type":"ssb","settings":{}}}"#).unwrap()
    );
    assert_eq!(ChannelParams::default_for("wefax"), None);
    assert_eq!(ChannelSettings::default_for("wefax"), None);
}
