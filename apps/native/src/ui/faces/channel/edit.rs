use std::sync::Arc;

use sdrmm_wire::{
    channel::{ChannelDescriptor, ChannelInfo, ChannelSettings},
    patch::{NodeBody, PatchGraph},
    state::StateSnapshot,
    workspace_state::WorkspaceChannel,
};

use zgui::prelude::*;

use crate::{binding, store::Store};

#[derive(Clone, Debug, PartialEq)]
pub struct LiveChannel {
    pub device_set: u32,
    pub channel: ChannelInfo,
}

#[must_use]
pub fn descriptor_for<'a>(
    graph: &PatchGraph,
    types: &'a [ChannelDescriptor],
    node: &str,
) -> Option<&'a ChannelDescriptor> {
    let NodeBody::Channel(channel) = &graph.node(node)?.body else {
        return None;
    };
    types
        .iter()
        .find(|descriptor| descriptor.type_id == channel.channel_type)
}

#[must_use]
pub fn live_in(graph: &PatchGraph, state: &StateSnapshot, node: &str) -> Option<LiveChannel> {
    let devices = binding::device_sets(graph, &state.device_sets);
    let owner = binding::device_node_of(graph, node)?;
    let device_set = *devices.get(&owner)?;
    let channel = binding::channels(graph, &state.device_sets, &devices).remove(node)?;
    Some(LiveChannel {
        device_set,
        channel,
    })
}

#[must_use]
pub fn resolve_settings(
    live: Option<&ChannelInfo>,
    saved: &[WorkspaceChannel],
    node: &str,
    defaults: Option<&ChannelDescriptor>,
) -> Option<ChannelSettings> {
    live.map(|channel| channel.settings.clone())
        .or_else(|| {
            saved
                .iter()
                .find(|held| held.node == node)
                .map(|held| held.settings.clone())
        })
        .or_else(|| defaults.and_then(|descriptor| descriptor.defaults.clone()))
}

#[must_use]
pub fn with_saved(
    saved: &[WorkspaceChannel],
    node: &str,
    settings: ChannelSettings,
) -> Vec<WorkspaceChannel> {
    let mut next = saved.to_vec();
    match next.iter_mut().find(|held| held.node == node) {
        Some(held) => held.settings = settings,
        None => next.push(WorkspaceChannel {
            node: node.to_owned(),
            settings,
        }),
    }
    next
}

impl Store {
    pub fn channel_descriptor(self, node: &str) -> Option<ChannelDescriptor> {
        let graph = self.graph.get();
        let types = self.channel_types.get();
        descriptor_for(&graph, &types, node).cloned()
    }

    pub fn live_channel(self, node: &str) -> Option<LiveChannel> {
        let device_set = self.device_set_of(node)?;
        let channel = self.channel_of(node)?;
        Some(LiveChannel {
            device_set,
            channel,
        })
    }

    pub fn channel_settings(self, node: &str) -> Option<ChannelSettings> {
        let live = self.channel_of(node);
        let saved = self.saved_channels.get();
        let descriptor = self.channel_descriptor(node);
        resolve_settings(live.as_ref(), &saved, node, descriptor.as_ref())
    }

    pub fn live_channel_untracked(self, node: &str) -> Option<LiveChannel> {
        live_in(
            &self.graph.get_untracked(),
            &self.state.get_untracked(),
            node,
        )
    }

    pub fn channel_settings_untracked(self, node: &str) -> Option<ChannelSettings> {
        let graph = self.graph.get_untracked();
        let live = live_in(&graph, &self.state.get_untracked(), node);
        let types = self.channel_types.get_untracked();
        resolve_settings(
            live.as_ref().map(|live| &live.channel),
            &self.saved_channels.get_untracked(),
            node,
            descriptor_for(&graph, &types, node),
        )
    }

    pub fn edit_channel(self, node: &str, edit: impl FnOnce(&mut ChannelSettings)) {
        let Some(mut settings) = self.channel_settings_untracked(node) else {
            self.say("This decoder has no settings yet");
            return;
        };
        edit(&mut settings);
        self.retype_channel_settings(node, settings);
    }

    pub fn retype_channel_settings(self, node: &str, settings: ChannelSettings) {
        if self.live_channel_untracked(node).is_some() {
            self.set_channel(node.to_owned(), settings);
        } else {
            self.save_channel(node, settings);
        }
    }

    pub fn save_channel(self, node: &str, settings: ChannelSettings) {
        let Some(editor) = self.editor() else {
            self.say("Workspace is still loading");
            return;
        };
        let next = with_saved(&self.saved_channels.get_untracked(), node, settings.clone());
        self.saved_channels.set(Arc::new(next));
        self.receive_settings(editor.saved_channel(node.to_owned(), settings));
    }

    pub fn edit_node(self, node: &str, edit: impl FnOnce(&mut NodeBody) + 'static) {
        let id = node.to_owned();
        self.edit_graph(move |graph| {
            if let Some(found) = graph.nodes.iter_mut().find(|found| found.id == id) {
                edit(&mut found.body);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(hz: f64) -> ChannelSettings {
        let mut settings = ChannelSettings::default_for("nfm").expect("nfm");
        settings.frequency_hz = hz;
        settings
    }

    fn info(hz: f64) -> ChannelInfo {
        ChannelInfo {
            id: 1,
            stream: 0,
            node: None,
            settings: settings(hz),
            out_of_band: false,
            audio_recordings: Vec::new(),
            baseband_recording: None,
            network_export: None,
        }
    }

    fn descriptor() -> ChannelDescriptor {
        ChannelDescriptor {
            type_id: "nfm".into(),
            defaults: Some(settings(1.0)),
            ..ChannelDescriptor::default()
        }
    }

    #[test]
    fn a_live_channel_wins_then_what_was_saved_then_the_defaults() {
        let saved = vec![WorkspaceChannel {
            node: "ch".into(),
            settings: settings(2.0),
        }];
        let live = info(3.0);
        let with = |live, saved: &[WorkspaceChannel], node| {
            resolve_settings(live, saved, node, Some(&descriptor())).map(|s| s.frequency_hz)
        };
        assert_eq!(with(Some(&live), &saved, "ch"), Some(3.0));
        assert_eq!(with(None, &saved, "ch"), Some(2.0));
        assert_eq!(with(None, &saved, "other"), Some(1.0));
        assert_eq!(resolve_settings(None, &[], "ch", None), None);
    }

    #[test]
    fn saving_replaces_the_node_held_or_adds_it() {
        let first = with_saved(&[], "ch", settings(1.0));
        assert_eq!(first.len(), 1);
        let second = with_saved(&first, "ch", settings(2.0));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].settings.frequency_hz, 2.0);
        assert_eq!(with_saved(&second, "other", settings(3.0)).len(), 2);
    }
}
