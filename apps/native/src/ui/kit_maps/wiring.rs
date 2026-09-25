use sdrmm_wire::patch::{NodeBody, PatchGraph};

const MAX_FILTER_DEPTH: usize = 8;

#[must_use]
pub fn event_sources(graph: &PatchGraph, node: &str) -> Vec<String> {
    let mut seen = Vec::new();
    walk(graph, node, 0, &mut seen);
    seen
}

fn walk(graph: &PatchGraph, node: &str, depth: usize, seen: &mut Vec<String>) {
    if depth > MAX_FILTER_DEPTH {
        return;
    }
    for edge in graph
        .edges
        .iter()
        .filter(|edge| edge.to.node == node && edge.to.port == "events")
    {
        let source = edge.from.node.as_str();
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
pub fn channel_type_of(graph: &PatchGraph, node: &str) -> Option<String> {
    match &graph.node(node)?.body {
        NodeBody::Channel(channel) => Some(channel.channel_type.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::patch::{PatchEdge, PatchNode, PortRef, Position};

    use super::*;

    fn node(id: &str, kind: &str) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body: NodeBody::default_for(kind).expect("a node kind"),
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn events(from: &str, to: &str) -> PatchEdge {
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

    #[test]
    fn event_sources_are_found_through_filters() {
        let graph = PatchGraph {
            nodes: vec![node("filter", "event_filter"), node("map", "map")],
            edges: vec![
                events("adsb", "filter"),
                events("ais", "map"),
                events("filter", "map"),
                events("ais", "map"),
            ],
        };
        assert_eq!(event_sources(&graph, "map"), ["ais", "adsb"]);
    }
}
