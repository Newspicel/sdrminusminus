use sdrmm_wire::patch::{ChannelNode, NodeBody, NodeCategory, PatchNode, Position};
use zgui::prelude::*;
use zgui_ui::prelude::*;

use crate::ui::patch::Canvas;
use crate::{
    store::Store,
    ui::{node, widgets::segments},
};

const SPAWN_STEP: f32 = 36.0;

pub fn sheet(store: Store, canvas: Canvas) -> impl IntoView {
    let search = RwSignal::new_local(String::new());
    let category = RwSignal::new(None::<NodeCategory>);
    let rows = move || {
        let catalog = store.catalog.get();
        let mut entries: Vec<(String, String, NodeCategory)> = catalog
            .nodes
            .iter()
            .filter(|info| !info.needs_channel_type)
            .map(|info| (info.kind.clone(), info.name.clone(), info.category))
            .collect();
        for descriptor in store.channel_types.get().iter() {
            entries.push((
                format!("channel:{}", descriptor.type_id),
                descriptor.name.clone(),
                NodeCategory::Channel,
            ));
        }
        entries
            .into_iter()
            .filter(|(kind, name, group)| {
                matches_search(kind, name, *group, &search.get(), category.get())
            })
            .map(|(kind, name, category)| {
                let category_label = node::category_class(category).to_uppercase();
                let chosen = kind.clone();
                view! {
                    control(
                        class = "pal__row",
                        a11y:role = Role::Button,
                        tabindex = Focus::Sequential,
                        on:click:stop = move |_| {
                            add(store, canvas, &chosen);
                            store.palette.set(false);
                        }
                    ) {
                        text {{name}}
                        text(class = "pal__cat") {{category_label}}
                    }
                }
            })
            .collect::<Vec<_>>()
    };

    view! {
        column(class = "pal") {
            row(class = "pal__head") {
                text(class = "legend") {"Add a node"}
                spacer()
                control(class = "node__shut", on:click:stop = move |_| store.palette.set(false)) {"x"}
            }
            box(class = "pal__search") {
                Input(class = "native-input", value = search, label = "Search nodes", placeholder = "Search nodes…")
            }
            {segments(
                vec![(None, "All"), (Some(NodeCategory::Source), "Sources"), (Some(NodeCategory::Channel), "Channels"), (Some(NodeCategory::Tool), "Tools"), (Some(NodeCategory::Output), "Outputs")],
                category.into(), move |chosen| category.set(chosen),
            )}
            column(class = "pal__list") {{rows}}
        }
    }
}

fn matches_search(
    kind: &str,
    name: &str,
    category: NodeCategory,
    search: &str,
    selected: Option<NodeCategory>,
) -> bool {
    if selected.is_some_and(|selected| selected != category) {
        return false;
    }
    let haystack = format!("{kind} {name}").to_lowercase();
    search
        .split_whitespace()
        .all(|term| haystack.contains(&term.to_lowercase()))
}

pub fn body_for(kind: &str) -> Option<NodeBody> {
    if let Some(type_id) = kind.strip_prefix("channel:") {
        return Some(NodeBody::Channel(ChannelNode {
            channel_type: type_id.to_owned(),
            record_calls: false,
            tuning_locked: false,
        }));
    }
    NodeBody::default_for(kind)
}

pub fn free_id(taken: &[String], kind: &str) -> String {
    let stem = kind.strip_prefix("channel:").unwrap_or(kind);
    let mut at = 1;
    loop {
        let candidate = if at == 1 {
            stem.to_owned()
        } else {
            format!("{stem}{at}")
        };
        if !taken.iter().any(|held| held == &candidate) {
            return candidate;
        }
        at += 1;
    }
}

fn add(store: Store, canvas: Canvas, kind: &str) {
    let Some(body) = body_for(kind) else {
        store.say(format!("Unknown node kind: {kind}"));
        return;
    };
    let kind = kind.to_owned();
    let at = store.palette_at.get_untracked().unwrap_or_else(|| {
        let centre = canvas.visible_centre();
        (
            centre.x as f32 - node::DEFAULT_WIDTH / 2.0,
            centre.y as f32 - 80.0,
        )
    });
    store.palette_at.set(None);
    store.edit_graph(move |graph| {
        let taken: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let step = graph
            .nodes
            .iter()
            .filter(|node| {
                (node.position.x - at.0).abs() < 1.0 && (node.position.y - at.1).abs() < 1.0
            })
            .count() as f32
            * SPAWN_STEP;
        let (x, y) = (at.0 + step, at.1 + step);
        graph.nodes.push(PatchNode {
            id: free_id(&taken, &kind),
            body,
            position: Position { x, y },
            size: None,
            label: None,
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_entry_carries_its_decoder_into_the_node() {
        let NodeBody::Channel(channel) = body_for("channel:wfm").expect("a channel") else {
            panic!("a channel node");
        };
        assert_eq!(channel.channel_type, "wfm");
    }

    #[test]
    fn every_catalog_entry_has_a_typed_body() {
        for entry in sdrmm_wire::patch::PatchCatalog::build().nodes {
            if entry.needs_channel_type {
                continue;
            }
            let body = body_for(&entry.kind).unwrap_or_else(|| panic!("missing {}", entry.kind));
            assert_eq!(body.kind(), entry.kind);
            assert_eq!(body.category(), entry.category);
            let encoded = serde_json::to_value(&body).expect("encode");
            assert_eq!(
                serde_json::from_value::<NodeBody>(encoded).expect("decode"),
                body
            );
        }
        assert!(body_for("unknown_node").is_none());
    }

    #[test]
    fn search_combines_words_and_category_without_case_sensitivity() {
        assert!(matches_search(
            "channel:nfm",
            "Narrow FM",
            NodeCategory::Channel,
            "FM narrow",
            None
        ));
        assert!(!matches_search(
            "channel:nfm",
            "Narrow FM",
            NodeCategory::Channel,
            "FM",
            Some(NodeCategory::Tool)
        ));
        assert!(!matches_search(
            "channel:nfm",
            "Narrow FM",
            NodeCategory::Channel,
            "FM wide",
            None
        ));
    }

    #[test]
    fn a_new_node_takes_the_first_name_nothing_else_holds() {
        let taken = vec!["scope".to_owned(), "scope2".to_owned()];
        assert_eq!(free_id(&taken, "scope"), "scope3");
        assert_eq!(free_id(&taken, "channel:nfm"), "nfm");
        assert_eq!(free_id(&[], "speaker"), "speaker");
    }
}
