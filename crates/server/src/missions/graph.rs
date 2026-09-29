use sdrmm_wire::{
    CONTROL_PORT, EVENTS_PORT, GpsNode, MissionProblem, NodeBody, POSITION_PORT, PatchGraph,
    PatchNode, PortRef, PositionLink, PositionSource,
};

use crate::phones::Phones;

pub(crate) fn source_of<'a>(graph: &'a PatchGraph, node: &str, port: &str) -> Option<&'a PortRef> {
    graph
        .edges
        .iter()
        .find(|edge| edge.to.node == node && edge.to.port == port)
        .map(|edge| &edge.from)
}

pub(crate) fn event_targets(graph: &PatchGraph, node: &str, kind: &str) -> Vec<String> {
    events(graph, node, Direction::Downstream)
        .into_iter()
        .filter(|target| target.body.kind() == kind)
        .map(|target| target.id.clone())
        .collect()
}

pub(crate) fn event_sources(graph: &PatchGraph, node: &str) -> Vec<String> {
    events(graph, node, Direction::Upstream)
        .into_iter()
        .map(|source| source.id.clone())
        .collect()
}

pub(crate) fn position_link(
    graph: &PatchGraph,
    phones: &Phones,
    node: &str,
) -> (Option<PositionLink>, Option<MissionProblem>) {
    link(graph, phones, node, POSITION_PORT)
}

pub(crate) fn link(
    graph: &PatchGraph,
    phones: &Phones,
    node: &str,
    port: &str,
) -> (Option<PositionLink>, Option<MissionProblem>) {
    let Some(source) = source_of(graph, node, port) else {
        return (None, None);
    };
    let phone = phone_of(graph, &source.node);
    let problem = phone
        .as_deref()
        .and_then(|phone| phone_problem(phones, phone));
    (
        Some(PositionLink {
            node: source.node.clone(),
            phone,
        }),
        problem,
    )
}

pub(crate) fn controlled_channel(graph: &PatchGraph, hunt: &str) -> Option<String> {
    graph
        .targets_of(hunt, CONTROL_PORT)
        .find(|target| {
            graph
                .node(target)
                .is_some_and(|node| matches!(node.body, NodeBody::Channel(_)))
        })
        .map(str::to_owned)
}

fn phone_of(graph: &PatchGraph, gps: &str) -> Option<String> {
    match &graph.node(gps)?.body {
        NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Phone { phone }),
        }) => Some(phone.clone()),
        _ => None,
    }
}

fn phone_problem(phones: &Phones, phone: &str) -> Option<MissionProblem> {
    if !phones.known(phone) {
        return Some(MissionProblem::PhoneNotPaired {
            phone: phone.to_owned(),
        });
    }
    (!phones.online(phone)).then(|| MissionProblem::PhoneOffline {
        phone: phone.to_owned(),
    })
}

#[derive(Clone, Copy)]
enum Direction {
    Downstream,
    Upstream,
}

fn events<'a>(graph: &'a PatchGraph, node: &str, direction: Direction) -> Vec<&'a PatchNode> {
    let mut found: Vec<&PatchNode> = Vec::new();
    let mut walked = vec![node.to_owned()];
    let mut next = 0;
    while let Some(from) = walked.get(next).cloned() {
        next += 1;
        for edge in graph
            .edges
            .iter()
            .filter(|edge| edge.from.port == EVENTS_PORT && edge.to.port == EVENTS_PORT)
        {
            let (near, far) = match direction {
                Direction::Downstream => (&edge.from.node, &edge.to.node),
                Direction::Upstream => (&edge.to.node, &edge.from.node),
            };
            if *near != from {
                continue;
            }
            let Some(reached) = graph.node(far) else {
                continue;
            };
            if matches!(reached.body, NodeBody::EventFilter(_)) {
                if !walked.contains(far) {
                    walked.push(far.clone());
                }
            } else if !found.iter().any(|seen| seen.id == reached.id) {
                found.push(reached);
            }
        }
    }
    found
}
