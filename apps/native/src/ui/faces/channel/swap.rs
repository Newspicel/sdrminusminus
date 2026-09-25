use std::collections::HashSet;

use sdrmm_wire::{
    channel::{ChannelDescriptor, ChannelSettings},
    patch::{NodeBody, PatchGraph, PortBacking},
};

use super::settings::keeps_calls;
use zgui::prelude::*;

use crate::store::Store;

pub const ANALOG_MODES: [&str; 4] = ["nfm", "wfm", "am", "ssb"];

#[must_use]
pub fn next_analog_mode(current: &str, direction: i32) -> &'static str {
    let Some(at) = ANALOG_MODES.iter().position(|mode| *mode == current) else {
        return ANALOG_MODES[0];
    };
    let length = ANALOG_MODES.len() as i32;
    let next = (at as i32 + direction).rem_euclid(length) as usize;
    ANALOG_MODES.get(next).copied().unwrap_or(ANALOG_MODES[0])
}

#[must_use]
pub fn retype_channel(graph: &PatchGraph, id: &str, descriptor: &ChannelDescriptor) -> PatchGraph {
    let mut retyped = graph.clone();
    let Some(node) = retyped.nodes.iter_mut().find(|node| node.id == id) else {
        return retyped;
    };
    let NodeBody::Channel(channel) = &mut node.body else {
        return retyped;
    };
    channel.channel_type.clone_from(&descriptor.type_id);
    if !keeps_calls(Some(descriptor)) {
        channel.record_calls = false;
    }
    let kept: HashSet<String> = node
        .body
        .ports_with(Some(PortBacking::Channel(descriptor)))
        .into_iter()
        .map(|port| port.name)
        .collect();
    retyped.edges.retain(|edge| {
        (edge.from.node != id || kept.contains(&edge.from.port))
            && (edge.to.node != id || kept.contains(&edge.to.port))
    });
    retyped
}

#[must_use]
pub fn retyped_settings(
    current: Option<&ChannelSettings>,
    descriptor: &ChannelDescriptor,
) -> Option<ChannelSettings> {
    let mut defaults = descriptor.defaults.clone()?;
    if let Some(current) = current {
        defaults.frequency_hz = current.frequency_hz;
        defaults.squelch = current.squelch;
    }
    Some(defaults)
}

#[must_use]
pub fn current_type(graph: &PatchGraph, node: &str) -> Option<String> {
    match &graph.node(node)?.body {
        NodeBody::Channel(channel) => Some(channel.channel_type.clone()),
        _ => None,
    }
}

pub fn swap_decoder(store: Store, node: &str, type_id: &str) -> bool {
    let graph = store.graph.get_untracked();
    if current_type(&graph, node).is_none_or(|current| current == type_id) {
        return false;
    }
    let Some(descriptor) = store
        .channel_types
        .get_untracked()
        .iter()
        .find(|descriptor| descriptor.type_id == type_id)
        .cloned()
    else {
        store.say(format!("Unknown decoder: {type_id}"));
        return false;
    };
    let current = store.channel_settings_untracked(node);
    let Some(settings) = retyped_settings(current.as_ref(), &descriptor) else {
        store.say(format!("{} has no default settings", descriptor.name));
        return false;
    };
    store.retype_channel_settings(node, settings);
    let id = node.to_owned();
    store.edit_graph(move |graph| *graph = retype_channel(graph, &id, &descriptor));
    true
}

pub fn cycle_analog(store: Store, node: &str, direction: i32) -> bool {
    let Some(current) = current_type(&store.graph.get_untracked(), node) else {
        return false;
    };
    swap_decoder(store, node, next_analog_mode(&current, direction))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        channel::{ChannelParams, Squelch},
        patch::{ChannelNode, DeviceNode, PatchEdge, PatchNode, PortRef, Position},
    };

    use super::*;

    fn descriptor(type_id: &str, has_audio: bool, decoder_kind: Option<&str>) -> ChannelDescriptor {
        ChannelDescriptor {
            type_id: type_id.into(),
            name: type_id.to_uppercase(),
            bandwidth_hz: 12_500.0,
            input_rate_hz: 48_000.0,
            has_audio,
            decoder_kind: decoder_kind.map(str::to_owned),
            defaults: ChannelSettings::default_for(type_id),
            ..ChannelDescriptor::default()
        }
    }

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.into(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.into(),
                port: from.1.into(),
            },
            to: PortRef {
                node: to.0.into(),
                port: to.1.into(),
            },
        }
    }

    fn graph() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node("dev", NodeBody::Device(DeviceNode::default())),
                node(
                    "ch",
                    NodeBody::Channel(ChannelNode {
                        channel_type: "nfm".into(),
                        record_calls: true,
                        tuning_locked: true,
                    }),
                ),
                node("spk", NodeBody::Speaker),
                node("log", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge(("dev", "iq"), ("ch", "iq")),
                edge(("ch", "audio"), ("spk", "audio")),
                edge(("ch", "events"), ("log", "events")),
            ],
        }
    }

    fn channel_of(graph: &PatchGraph) -> ChannelNode {
        match graph.node("ch").map(|node| node.body.clone()) {
            Some(NodeBody::Channel(channel)) => channel,
            other => panic!("not a channel: {other:?}"),
        }
    }

    #[test]
    fn the_analog_ring_turns_both_ways() {
        assert_eq!(next_analog_mode("nfm", 1), "wfm");
        assert_eq!(next_analog_mode("nfm", -1), "ssb");
        assert_eq!(next_analog_mode("ssb", 1), "nfm");
    }

    #[test]
    fn a_decoder_off_the_ring_lands_on_its_first_mode() {
        assert_eq!(next_analog_mode("adsb", 1), ANALOG_MODES[0]);
        assert_eq!(next_analog_mode("adsb", -1), ANALOG_MODES[0]);
    }

    #[test]
    fn retyping_keeps_what_the_node_was_set_to_apart_from_its_type() {
        let retyped = retype_channel(&graph(), "ch", &descriptor("am", true, None));
        assert_eq!(
            channel_of(&retyped),
            ChannelNode {
                channel_type: "am".into(),
                record_calls: false,
                tuning_locked: true,
            }
        );
    }

    #[test]
    fn retyping_drops_the_wires_the_new_decoder_has_no_port_for() {
        let retyped = retype_channel(&graph(), "ch", &descriptor("am", true, None));
        assert_eq!(
            retyped.edges,
            vec![
                edge(("dev", "iq"), ("ch", "iq")),
                edge(("ch", "audio"), ("spk", "audio")),
            ]
        );
    }

    #[test]
    fn retyping_leaves_a_wire_the_new_decoder_still_carries() {
        let retyped = retype_channel(&graph(), "ch", &descriptor("dmr", true, Some("dv")));
        assert_eq!(retyped.edges.len(), 3);
        assert!(channel_of(&retyped).record_calls);
    }

    #[test]
    fn a_silent_decoder_loses_its_audio_wire() {
        let retyped = retype_channel(&graph(), "ch", &descriptor("adsb", false, Some("adsb")));
        assert_eq!(
            retyped.edges,
            vec![
                edge(("dev", "iq"), ("ch", "iq")),
                edge(("ch", "events"), ("log", "events")),
            ]
        );
    }

    #[test]
    fn retyped_settings_keep_where_the_decoder_listens_and_its_squelch() {
        let mut current = ChannelSettings::default_for("nfm").expect("nfm");
        current.frequency_hz = 145.5e6;
        current.squelch = Squelch::Manual { level_db: -50.0 };
        let am = descriptor("am", true, None);
        let settings = retyped_settings(Some(&current), &am).expect("settings");
        assert_eq!(settings.frequency_hz, 145.5e6);
        assert_eq!(settings.squelch, Squelch::Manual { level_db: -50.0 });
        assert!(matches!(settings.params, ChannelParams::Am(_)));
    }

    #[test]
    fn with_nothing_held_the_new_decoder_starts_on_its_defaults() {
        let am = descriptor("am", true, None);
        assert_eq!(retyped_settings(None, &am), am.defaults.clone());
        let bare = ChannelDescriptor {
            defaults: None,
            ..am
        };
        assert_eq!(retyped_settings(None, &bare), None);
    }

    #[test]
    fn the_current_type_is_read_off_the_channel_node() {
        assert_eq!(current_type(&graph(), "ch").as_deref(), Some("nfm"));
        assert_eq!(current_type(&graph(), "spk"), None);
        assert_eq!(current_type(&graph(), "gone"), None);
    }
}
