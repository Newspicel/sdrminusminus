use sdrmm_wire::patch::{NodeBody, PatchGraph};

use crate::decoders::filter::WiredSource;

const MAX_FILTER_DEPTH: usize = 16;

#[must_use]
pub fn event_sources_of(graph: &PatchGraph, node: &str) -> Vec<String> {
    let mut seen = Vec::new();
    walk(graph, node, 0, &mut seen);
    seen
}

fn walk(graph: &PatchGraph, node: &str, depth: usize, seen: &mut Vec<String>) {
    if depth > MAX_FILTER_DEPTH {
        return;
    }
    for source in graph.sources_of(node, "events") {
        let filter = graph
            .node(source)
            .is_some_and(|found| matches!(found.body, NodeBody::EventFilter(_)));
        if filter {
            walk(graph, source, depth + 1, seen);
        } else if !seen.iter().any(|held| held == source) {
            seen.push(source.to_owned());
        }
    }
}

#[must_use]
pub fn wired_sources_of(graph: &PatchGraph, node: &str) -> Vec<WiredSource> {
    event_sources_of(graph, node)
        .iter()
        .map(|id| match graph.node(id).map(|found| &found.body) {
            Some(NodeBody::Channel(channel)) => WiredSource {
                channel_type: Some(channel.channel_type.clone()),
                records_calls: channel.record_calls,
                ..WiredSource::default()
            },
            Some(NodeBody::SpectrumMonitor(_)) => WiredSource {
                monitor: true,
                ..WiredSource::default()
            },
            Some(NodeBody::DmrTrunk(trunk)) => WiredSource {
                records_calls: trunk.record_calls,
                trunk: true,
                ..WiredSource::default()
            },
            _ => WiredSource::default(),
        })
        .collect()
}

#[must_use]
pub fn hears_monitor(graph: &PatchGraph, node: &str) -> bool {
    event_sources_of(graph, node).iter().any(|source| {
        graph
            .node(source)
            .is_some_and(|found| matches!(found.body, NodeBody::SpectrumMonitor(_)))
    })
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        filter::EventFilterNode,
        patch::{ChannelNode, PatchEdge, PatchNode, PortRef, Position},
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

    fn events(from: &str, to: &str) -> PatchEdge {
        let port = |node: &str| PortRef {
            node: node.to_owned(),
            port: "events".to_owned(),
        };
        PatchEdge {
            from: port(from),
            to: port(to),
        }
    }

    #[test]
    fn a_filter_is_seen_through_to_the_decoders_behind_it() {
        let channel = NodeBody::Channel(ChannelNode {
            channel_type: "adsb".to_owned(),
            record_calls: false,
            tuning_locked: false,
        });
        let graph = PatchGraph {
            nodes: vec![
                node("adsb", channel),
                node("filter", NodeBody::EventFilter(EventFilterNode::default())),
                node("log", NodeBody::DecoderLog),
            ],
            edges: vec![events("adsb", "filter"), events("filter", "log")],
        };
        assert_eq!(event_sources_of(&graph, "log"), ["adsb"]);
        assert_eq!(
            wired_sources_of(&graph, "log"),
            [WiredSource {
                channel_type: Some("adsb".to_owned()),
                ..WiredSource::default()
            }]
        );
        assert!(!hears_monitor(&graph, "log"));
    }
}
