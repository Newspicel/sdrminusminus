use sdrmm_wire::patch::{ChannelNode, DeviceNode, NodeBody, NodeCategory, PatchNode, Position};
use zgui::prelude::*;

use crate::{store::Store, ui::node};

const SPAWN_STEP: f32 = 36.0;

pub fn sheet(store: Store) -> impl IntoView {
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
            .map(|(kind, name, category)| {
                let category_label = node::category_class(category).to_uppercase();
                let chosen = kind.clone();
                view! {
                    control(
                        class = "pal__row",
                        on:click:stop = move |_| {
                            add(store, &chosen);
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
            column(class = "pal__list") {{rows}}
        }
    }
}

pub fn body_for(kind: &str) -> Option<NodeBody> {
    if let Some(type_id) = kind.strip_prefix("channel:") {
        return Some(NodeBody::Channel(ChannelNode {
            channel_type: type_id.to_owned(),
            record_calls: false,
            tuning_locked: false,
        }));
    }
    Some(match kind {
        "device" => NodeBody::Device(DeviceNode::default()),
        "scope" => NodeBody::Scope,
        "speaker" => NodeBody::Speaker,
        "map" => NodeBody::Map,
        "readout" => NodeBody::Readout,
        "decoder_log" => NodeBody::DecoderLog,
        "video" => NodeBody::Video,
        "recorder" => NodeBody::Recorder,
        "audio_recorder" => NodeBody::AudioRecorder,
        "baseband_recorder" => NodeBody::BasebandRecorder,
        "export" => NodeBody::Export,
        "scanner" => NodeBody::Scanner,
        "triangulation" => NodeBody::Triangulation,
        _ => return None,
    })
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

fn add(store: Store, kind: &str) {
    let Some(body) = body_for(kind) else {
        store.say(format!("{kind} nodes are not built in this experiment"));
        return;
    };
    let kind = kind.to_owned();
    store.edit_graph(move |graph| {
        let taken: Vec<String> = graph.nodes.iter().map(|node| node.id.clone()).collect();
        let step = graph.nodes.len() as f32 * SPAWN_STEP;
        graph.nodes.push(PatchNode {
            id: free_id(&taken, &kind),
            body,
            position: Position {
                x: 80.0 + step,
                y: 80.0 + step,
            },
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
    fn a_kind_this_experiment_does_not_build_is_refused_rather_than_guessed() {
        assert!(body_for("passive_radar").is_none());
        assert!(body_for("scope").is_some());
    }

    #[test]
    fn a_new_node_takes_the_first_name_nothing_else_holds() {
        let taken = vec!["scope".to_owned(), "scope2".to_owned()];
        assert_eq!(free_id(&taken, "scope"), "scope3");
        assert_eq!(free_id(&taken, "channel:nfm"), "nfm");
        assert_eq!(free_id(&[], "speaker"), "speaker");
    }
}
