use std::collections::HashMap;

use sdrmm_wire::{DecoderEvent, EventFilterNode, NodeBody, PatchGraph, StateSnapshot};

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

#[cfg(test)]
mod tests {
    use sdrmm_wire::{ChannelNode, PatchEdge, PatchNode, PortRef, Position, RttyText};

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
}
