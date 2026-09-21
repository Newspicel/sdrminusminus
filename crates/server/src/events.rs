use std::collections::HashMap;

use sdrmm_wire::{
    DecodedRecord, DecoderEvent, EventFilterNode, NodeBody, PatchGraph, StateSnapshot,
};

const MAX_FILTER_DEPTH: usize = 16;

pub(crate) fn decoder_nodes(
    graph: &PatchGraph,
    state: &StateSnapshot,
) -> HashMap<(u32, u32), String> {
    let mut sources: HashMap<(u32, u32), String> = crate::workspace::bind(graph, state)
        .into_iter()
        .flat_map(|binding| {
            let device_set = binding.device_set;
            binding
                .channels
                .into_iter()
                .map(move |(node, channel)| ((device_set, channel), node))
        })
        .collect();
    for system in &state.trunk_systems {
        for follower in &system.followers {
            sources
                .entry((follower.device_set, follower.channel))
                .or_insert_with(|| system.node.clone());
        }
        if let Some(control) = &system.control {
            sources
                .entry((control.device_set, control.channel))
                .or_insert_with(|| system.node.clone());
        }
    }
    sources
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EventPath {
    pub source: String,
    pub filters: Vec<EventFilterNode>,
}

impl EventPath {
    pub(crate) fn passes(&self, event: &DecoderEvent) -> bool {
        self.filters.iter().all(|filter| filter.passes(event))
    }
}

pub(crate) fn paths_into(graph: &PatchGraph, sink: &str) -> Vec<EventPath> {
    let mut paths = Vec::new();
    walk(graph, sink, &mut Vec::new(), &mut paths, 0);
    paths
}

fn walk(
    graph: &PatchGraph,
    node: &str,
    filters: &mut Vec<EventFilterNode>,
    paths: &mut Vec<EventPath>,
    depth: usize,
) {
    if depth > MAX_FILTER_DEPTH {
        return;
    }
    for source in graph.sources_of(node, "events") {
        match graph.node(source).map(|found| &found.body) {
            Some(NodeBody::EventFilter(settings)) => {
                filters.push(settings.clone());
                walk(graph, source, filters, paths, depth + 1);
                filters.pop();
            }
            _ => paths.push(EventPath {
                source: source.to_owned(),
                filters: filters.clone(),
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sink {
    pub node: String,
    pub paths: Vec<EventPath>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Routes {
    pub workspace: Option<i64>,
    pub sinks: Vec<Sink>,
    pub sources: HashMap<(u32, u32), String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Routed {
    pub record: DecodedRecord,
    pub source: Option<String>,
    pub workspace: Option<i64>,
}

#[cfg(test)]
impl Routed {
    pub(crate) fn unattributed(record: DecodedRecord) -> Self {
        Self {
            record,
            source: None,
            workspace: None,
        }
    }
}

impl Routes {
    pub(crate) fn resolve(workspace: i64, graph: &PatchGraph, state: &StateSnapshot) -> Self {
        let sinks = graph
            .nodes
            .iter()
            .filter(|node| !matches!(node.body, NodeBody::EventFilter(_)))
            .filter_map(|node| {
                let paths = paths_into(graph, &node.id);
                (!paths.is_empty()).then(|| Sink {
                    node: node.id.clone(),
                    paths,
                })
            })
            .collect();
        Self {
            workspace: Some(workspace),
            sinks,
            sources: decoder_nodes(graph, state),
        }
    }

    pub(crate) fn route(&self, mut record: DecodedRecord) -> Routed {
        let source = record
            .origin
            .as_ref()
            .map(|origin| origin.node.clone())
            .or_else(|| {
                self.sources
                    .get(&(record.device_set, record.channel))
                    .cloned()
            });
        record.sinks = source
            .as_deref()
            .map_or_else(Vec::new, |source| self.reached(source, &record.event));
        Routed {
            record,
            source,
            workspace: self.workspace,
        }
    }

    fn reached(&self, source: &str, event: &DecoderEvent) -> Vec<String> {
        self.sinks
            .iter()
            .filter(|sink| {
                sink.paths
                    .iter()
                    .any(|path| path.source == source && path.passes(event))
            })
            .map(|sink| sink.node.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        ChannelNode, PatchEdge, PatchNode, PortRef, Position, RttyText, TrunkControl,
        TrunkFollower, TrunkSystemStatus,
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

    fn channel(id: &str) -> PatchNode {
        node(
            id,
            NodeBody::Channel(ChannelNode {
                channel_type: "dmr".to_owned(),
                record_calls: true,
                tuning_locked: false,
            }),
        )
    }

    fn filter(id: &str, settings: EventFilterNode) -> PatchNode {
        node(id, NodeBody::EventFilter(settings))
    }

    fn edge(from: &str, to: &str) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.to_owned(),
                port: "events".to_owned(),
            },
            to: PortRef {
                node: to.to_owned(),
                port: "events".to_owned(),
            },
        }
    }

    fn only(kind: &str) -> EventFilterNode {
        EventFilterNode {
            kinds: vec![kind.to_owned()],
            ..EventFilterNode::default()
        }
    }

    fn rtty() -> DecoderEvent {
        DecoderEvent::Rtty(RttyText {
            text: "CQ".to_owned(),
        })
    }

    #[test]
    fn a_direct_wire_carries_no_filter() {
        let graph = PatchGraph {
            nodes: vec![channel("dmr"), node("chat", NodeBody::DecoderLog)],
            edges: vec![edge("dmr", "chat")],
        };

        let paths = paths_into(&graph, "chat");

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].source, "dmr");
        assert!(paths[0].filters.is_empty());
        assert!(paths[0].passes(&rtty()));
    }

    #[test]
    fn a_filter_in_the_middle_names_the_decoder_behind_it() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![edge("dmr", "only-calls"), edge("only-calls", "chat")],
        };

        let paths = paths_into(&graph, "chat");

        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0].source, "dmr",
            "the decoder, not the filter, is the source"
        );
        assert!(!paths[0].passes(&rtty()));
    }

    #[test]
    fn a_chain_of_filters_all_have_to_agree() {
        let strict = EventFilterNode {
            talkgroups: vec![505],
            ..EventFilterNode::default()
        };
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("kind", only("call")),
                filter("group", strict),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge("dmr", "kind"),
                edge("kind", "group"),
                edge("group", "chat"),
            ],
        };

        let paths = paths_into(&graph, "chat");

        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].filters.len(), 2);
        assert!(!paths[0].passes(&rtty()));
    }

    #[test]
    fn one_filter_can_serve_several_decoders() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                channel("p25"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge("dmr", "only-calls"),
                edge("p25", "only-calls"),
                edge("only-calls", "chat"),
            ],
        };

        let mut sources: Vec<String> = paths_into(&graph, "chat")
            .into_iter()
            .map(|path| path.source)
            .collect();
        sources.sort();

        assert_eq!(sources, vec!["dmr".to_owned(), "p25".to_owned()]);
    }

    #[test]
    fn a_decoder_can_reach_one_sink_filtered_and_unfiltered() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge("dmr", "only-calls"),
                edge("only-calls", "chat"),
                edge("dmr", "chat"),
            ],
        };

        let paths = paths_into(&graph, "chat");

        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.passes(&rtty())));
        assert!(paths.iter().any(|path| !path.passes(&rtty())));
    }

    #[test]
    fn a_trunk_system_owns_the_control_channel_it_opened_as_well_as_its_traffic() {
        let graph = PatchGraph {
            nodes: vec![node("trunk", NodeBody::DmrTrunk(Default::default()))],
            edges: Vec::new(),
        };
        let state = StateSnapshot {
            trunk_systems: vec![TrunkSystemStatus {
                node: "trunk".to_owned(),
                detected: None,
                carriers: 1,
                control: Some(TrunkControl {
                    device_set: 1,
                    channel: 9,
                    freq_hz: 451_012_500,
                }),
                followers: vec![TrunkFollower {
                    device_set: 1,
                    channel: 10,
                    logical_channel: Some(22),
                    slot: 2,
                    freq_hz: 451_125_000,
                }],
                problems: Vec::new(),
                channel_map: Vec::new(),
                probes: Vec::new(),
                searching: 0,
                candidates: 0,
                other_control_hz: Vec::new(),
                color_code: None,
            }],
            ..StateSnapshot::default()
        };

        let sources = decoder_nodes(&graph, &state);

        assert_eq!(sources.get(&(1, 9)).map(String::as_str), Some("trunk"));
        assert_eq!(sources.get(&(1, 10)).map(String::as_str), Some("trunk"));
    }

    #[test]
    fn a_sink_with_nothing_wired_in_has_no_path() {
        let graph = PatchGraph {
            nodes: vec![node("chat", NodeBody::DecoderLog)],
            edges: Vec::new(),
        };
        assert!(paths_into(&graph, "chat").is_empty());
    }

    #[test]
    fn a_loop_that_slipped_past_validation_still_terminates() {
        let graph = PatchGraph {
            nodes: vec![
                filter("a", EventFilterNode::default()),
                filter("b", EventFilterNode::default()),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![edge("a", "b"), edge("b", "a"), edge("b", "chat")],
        };

        let paths = paths_into(&graph, "chat");

        assert!(paths.len() <= MAX_FILTER_DEPTH + 1);
    }

    fn decoded(device_set: u32, channel: u32, event: DecoderEvent) -> DecodedRecord {
        DecodedRecord {
            origin: None,
            device_set,
            channel,
            at: "2026-09-21T10:00:00Z".to_owned(),
            freq_hz: 14_080_000.0,
            event,
            sinks: Vec::new(),
        }
    }

    fn routes_over(graph: &PatchGraph) -> Routes {
        Routes {
            sources: HashMap::from([((1, 2), "dmr".to_owned())]),
            ..Routes::resolve(7, graph, &StateSnapshot::default())
        }
    }

    #[test]
    fn every_node_fed_by_an_events_wire_is_a_sink_but_a_filter_is_not() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
                node("export", NodeBody::Export),
                node("idle", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge("dmr", "only-calls"),
                edge("only-calls", "chat"),
                edge("dmr", "export"),
            ],
        };

        let routes = routes_over(&graph);
        let mut sinks: Vec<&str> = routes.sinks.iter().map(|sink| sink.node.as_str()).collect();
        sinks.sort_unstable();

        assert_eq!(sinks, vec!["chat", "export"]);
        assert_eq!(routes.workspace, Some(7));
    }

    #[test]
    fn a_routed_record_names_the_sinks_it_reached_after_every_filter() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
                node("export", NodeBody::Export),
            ],
            edges: vec![
                edge("dmr", "only-calls"),
                edge("only-calls", "chat"),
                edge("dmr", "export"),
            ],
        };
        let routes = routes_over(&graph);

        let routed = routes.route(decoded(1, 2, rtty()));

        assert_eq!(routed.source.as_deref(), Some("dmr"));
        assert_eq!(routed.workspace, Some(7));
        assert_eq!(routed.record.sinks, vec!["export".to_owned()]);
    }

    #[test]
    fn a_drop_filter_on_the_wire_removes_what_it_names() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter(
                    "no-cq",
                    EventFilterNode {
                        mode: sdrmm_wire::FilterMode::Drop,
                        contains: Some("cq".to_owned()),
                        ..EventFilterNode::default()
                    },
                ),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![edge("dmr", "no-cq"), edge("no-cq", "chat")],
        };
        let routes = routes_over(&graph);

        assert!(routes.route(decoded(1, 2, rtty())).record.sinks.is_empty());
        let other = DecoderEvent::Rtty(RttyText {
            text: "73".to_owned(),
        });
        assert_eq!(
            routes.route(decoded(1, 2, other)).record.sinks,
            vec!["chat".to_owned()]
        );
    }

    #[test]
    fn a_record_from_an_unbound_channel_reaches_nothing() {
        let graph = PatchGraph {
            nodes: vec![channel("dmr"), node("chat", NodeBody::DecoderLog)],
            edges: vec![edge("dmr", "chat")],
        };
        let routes = routes_over(&graph);

        let routed = routes.route(decoded(9, 9, rtty()));

        assert_eq!(routed.source, None);
        assert!(routed.record.sinks.is_empty());
    }

    #[test]
    fn one_open_wire_is_enough_to_reach_a_sink_once() {
        let graph = PatchGraph {
            nodes: vec![
                channel("dmr"),
                filter("only-calls", only("call")),
                node("chat", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge("dmr", "only-calls"),
                edge("only-calls", "chat"),
                edge("dmr", "chat"),
            ],
        };
        let routes = routes_over(&graph);

        assert_eq!(
            routes.route(decoded(1, 2, rtty())).record.sinks,
            vec!["chat".to_owned()]
        );
    }
}

#[cfg(test)]
mod monitor_tests {
    use super::*;

    #[test]
    fn monitor_origin_routes_through_filters_without_claiming_a_manual_channel() {
        let mut routes = Routes::default();
        routes.sources.insert((1, 0), "manual".to_owned());
        routes.sinks.push(Sink {
            node: "export".to_owned(),
            paths: vec![EventPath {
                source: "monitor".to_owned(),
                filters: vec![EventFilterNode {
                    kinds: vec!["pocsag".to_owned()],
                    ..Default::default()
                }],
            }],
        });
        let mut record = DecodedRecord {
            origin: Some(sdrmm_wire::EventOrigin {
                node: "monitor".to_owned(),
                transmission: 7,
            }),
            device_set: 1,
            channel: 0,
            at: "2026-09-21T00:00:00Z".to_owned(),
            freq_hz: 145_000_000.0,
            event: DecoderEvent::Pocsag(sdrmm_wire::PocsagMessage {
                address: 42,
                function: 3,
                baud: 1200,
                payload: sdrmm_wire::PocsagPayload::Alpha,
                text: "hello".to_owned(),
                errors_corrected: 0,
            }),
            sinks: Vec::new(),
        };
        let routed = routes.route(record.clone());
        assert_eq!(routed.source.as_deref(), Some("monitor"));
        assert_eq!(routed.record.sinks, ["export"]);
        record.event = DecoderEvent::Rtty(sdrmm_wire::RttyText {
            text: "blocked".to_owned(),
        });
        assert!(routes.route(record).record.sinks.is_empty());
    }
}
