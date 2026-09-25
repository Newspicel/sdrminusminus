use std::collections::BTreeSet;

use sdrmm_wire::{
    device::Coherence,
    patch::{ArrayNode, NodeBody, PatchGraph},
};
use zgui::prelude::*;

use super::{
    device::{
        actions::edit_body,
        lanes::{ref_label, tune_delta, tuner_dials},
        radio::{Radio, radio_settings},
        tuner::reach_of,
    },
    set_signal,
};
use crate::{
    store::Store,
    ui::{
        kit_sources::{DialSpec, frequency_dial, install},
        widgets::{check, pick, row_field},
    },
};

const TIERS: [(Coherence, &str); 2] = [
    (Coherence::TimeSync, "Shared clock"),
    (Coherence::PhaseCoherent, "Shared clock and LO"),
];

#[must_use]
pub fn element_labels(graph: &PatchGraph, node: &str) -> Vec<(String, String)> {
    graph
        .array_members(node)
        .into_iter()
        .map(|member| {
            let label = graph
                .node(member)
                .and_then(|found| match &found.body {
                    NodeBody::Device(device) => device.device.as_ref().map(ref_label),
                    _ => None,
                })
                .unwrap_or_else(|| "no radio picked".to_owned());
            (member.to_owned(), label)
        })
        .collect()
}

fn settings_of(store: Store, node: &str) -> Option<ArrayNode> {
    store.graph.with(|graph| {
        graph.node(node).and_then(|found| match &found.body {
            NodeBody::Array(array) => Some(*array),
            _ => None,
        })
    })
}

fn update(store: Store, node: String, change: impl FnOnce(&mut ArrayNode) + 'static) {
    edit_body(store, node, move |body| {
        if let NodeBody::Array(array) = body {
            change(array);
        }
    });
}

pub fn face(store: Store, node: String) -> impl IntoView {
    install();
    let set = set_signal(store, node.clone());
    let radio = Radio { store, set };
    let open = Memo::new(move |_| set.with(Option::is_some));
    let members = {
        let node = node.clone();
        Memo::new(move |_| store.graph.with(|graph| element_labels(graph, &node)))
    };
    let settings = {
        let node = node.clone();
        Signal::derive(move || settings_of(store, &node).unwrap_or_default())
    };
    let dials = {
        let node = node.clone();
        move || {
            open.get()
                .then(|| AnyView::new(array_dials(store, node.clone(), radio)))
        }
    };
    let elements = move || {
        members
            .get()
            .into_iter()
            .enumerate()
            .map(|(index, (_, label))| {
                AnyView::new(row_field(
                    format!("Element {}", index + 1),
                    view! { text(class = "kit-mono") {{label}} },
                ))
            })
            .collect::<Vec<_>>()
    };
    let coherence = Signal::derive(move || {
        Some(match settings.get().coherence {
            Coherence::None => Coherence::TimeSync,
            tier => tier,
        })
    });
    let tiers: Vec<(Coherence, String)> = TIERS
        .iter()
        .map(|(tier, label)| (*tier, (*label).to_owned()))
        .collect();
    let shared = Signal::derive(move || settings.get().shared_tuning);
    let (for_tier, for_shared) = (node.clone(), node);
    view! {
        column(class = "face") {
            row(class = "kit-head", hidden = move || open.get() || members.with(|m| m.len() >= 2)) {
                spacer() {}
                text(class = "kit-legend") {{move || format!("{} of 2 radios", members.with(Vec::len))}}
            }
            {dials}
            {elements}
            {row_field("Wired as", pick(tiers, coherence, move |tier| update(store, for_tier.clone(), move |array| array.coherence = tier)))}
            {row_field("Tuned together", check(shared, move |on| update(store, for_shared.clone(), move |array| array.shared_tuning = on)))}
            {move || open.get().then(|| AnyView::new(radio_settings(radio, false, Signal::stored(BTreeSet::new()))))}
        }
    }
}

fn array_dials(store: Store, node: String, radio: Radio) -> impl IntoView {
    let streams = Memo::new(move |_| {
        radio
            .read(|set| {
                tuner_dials(set)
                    .into_iter()
                    .map(|dial| dial.stream)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    });
    let reach = reach_of(radio);
    let selected = Signal::derive(move || store.selected.get().as_deref() == Some(node.as_str()));
    move || {
        streams
            .get()
            .into_iter()
            .map(|stream| {
                let hz = Signal::derive(move || {
                    radio
                        .read(|set| {
                            tuner_dials(set)
                                .into_iter()
                                .find(|dial| dial.stream == stream)
                                .map_or(0.0, |dial| dial.hz)
                        })
                        .unwrap_or(0.0)
                });
                let spec = DialSpec {
                    hz,
                    reach,
                    disabled: Signal::stored(false),
                    wheel: selected,
                };
                let tune = move |value: f64| {
                    if let Some(delta) =
                        radio.read_untracked(|set| tune_delta(&set.capabilities, stream, value))
                    {
                        radio.patch(delta);
                    }
                };
                AnyView::new(view! { row(class = "kit-dial-row") {{frequency_dial(spec, tune)}} })
            })
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn graph(members: u32, wires: &[(&str, &str)]) -> PatchGraph {
        let edges: Vec<_> = wires
            .iter()
            .map(|(from, port)| json!({ "from": { "node": from, "port": "iq" }, "to": { "node": "bench", "port": port } }))
            .collect();
        serde_json::from_value(json!({
            "nodes": [
                { "id": "one", "kind": "device", "data": { "device": { "backend": "rtlsdr", "key": "0001" } }, "position": { "x": 0, "y": 0 } },
                { "id": "two", "kind": "device", "data": { "device": { "backend": "rtlsdr", "key": "0002" } }, "position": { "x": 0, "y": 0 } },
                { "id": "bench", "kind": "array", "data": { "members": members, "coherence": "time_sync", "shared_tuning": true }, "position": { "x": 0, "y": 0 } }
            ],
            "edges": edges
        }))
        .expect("graph")
    }

    #[test]
    fn the_radios_are_read_off_the_wires_in_the_order_they_arrive() {
        let wired = graph(2, &[("two", "iq2"), ("one", "iq")]);
        assert_eq!(wired.array_members("bench"), vec!["one", "two"]);
        assert_eq!(
            element_labels(&wired, "bench"),
            vec![
                ("one".to_owned(), "rtlsdr · 0001".to_owned()),
                ("two".to_owned(), "rtlsdr · 0002".to_owned())
            ]
        );
    }

    #[test]
    fn a_node_that_is_not_an_array_has_no_elements() {
        assert!(graph(1, &[("one", "iq")]).array_members("one").is_empty());
    }

    #[test]
    fn the_array_that_took_a_radio_is_named() {
        let wired = graph(1, &[("one", "iq")]);
        assert_eq!(wired.array_holding("one"), Some("bench"));
        assert_eq!(wired.array_holding("two"), None);
    }
}
