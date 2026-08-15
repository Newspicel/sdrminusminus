use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{channel::ChannelSettings, device::DeviceSettings};

/// Shape version of a stored [`WorkspaceState`]. Read like [`crate::WORKSPACE_SNAPSHOT_VERSION`]:
/// a row this build did not produce is discarded rather than guessed at. Discarding is safe here
/// in a way it is not for a workspace — the workspace still opens, just at defaults, which is
/// exactly the behavior that predates this file.
pub const WORKSPACE_STATE_VERSION: u32 = 1;

/// Settings for one workspace's nodes, as last observed on the engine.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceState {
    /// [`WORKSPACE_STATE_VERSION`] at the time of writing.
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<WorkspaceDevice>,
}

/// One device node's radio settings, and the channels hanging off it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceDevice {
    /// [`crate::PatchNode::id`] of the device node these settings belong to.
    pub node: String,
    pub settings: DeviceSettings,
    #[serde(default)]
    pub channels: Vec<WorkspaceChannel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceChannel {
    /// [`crate::PatchNode::id`] of the channel node these settings belong to.
    pub node: String,
    pub settings: ChannelSettings,
}

impl WorkspaceState {
    /// An empty state at the current version.
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: WORKSPACE_STATE_VERSION,
            devices: Vec::new(),
        }
    }

    /// The stored state, or an empty one if it was written by a different build.
    #[must_use]
    pub fn current(self) -> Self {
        if self.version == WORKSPACE_STATE_VERSION {
            self
        } else {
            Self::new()
        }
    }

    /// The device entry for a node, if one was saved.
    #[must_use]
    pub fn device(&self, node: &str) -> Option<&WorkspaceDevice> {
        self.devices.iter().find(|device| device.node == node)
    }

    /// The channel entry for a node, if one was saved.
    #[must_use]
    pub fn channel(&self, node: &str) -> Option<&WorkspaceChannel> {
        self.devices
            .iter()
            .flat_map(|device| &device.channels)
            .find(|channel| channel.node == node)
    }

    /// Replace the entries for `captured`'s nodes, keeping every other entry untouched.
    ///
    /// A capture only sees the nodes bound *this run*: a radio that is unplugged today contributes
    /// nothing, and overwriting wholesale would erase where it was tuned yesterday. Absence of a
    /// node from a capture means "not observed", never "reset it".
    pub fn merge(&mut self, captured: Vec<WorkspaceDevice>) {
        for device in captured {
            match self
                .devices
                .iter_mut()
                .find(|existing| existing.node == device.node)
            {
                Some(existing) => {
                    for channel in device.channels {
                        match existing
                            .channels
                            .iter_mut()
                            .find(|held| held.node == channel.node)
                        {
                            Some(held) => *held = channel,
                            None => existing.channels.push(channel),
                        }
                    }
                    existing.settings = device.settings;
                }
                None => self.devices.push(device),
            }
        }
    }

    /// Drop entries for nodes the graph no longer contains, so deleting a node eventually forgets
    /// its settings instead of holding them for the lifetime of the workspace.
    pub fn retain_nodes(&mut self, present: impl Fn(&str) -> bool) {
        self.devices.retain(|device| present(&device.node));
        for device in &mut self.devices {
            device.channels.retain(|channel| present(&channel.node));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{ChannelParams, NfmParams};

    fn channel(node: &str, offset_hz: f64) -> WorkspaceChannel {
        WorkspaceChannel {
            node: node.to_string(),
            settings: ChannelSettings {
                offset_hz,
                squelch_db: None,
                squelch_auto_db: None,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: crate::audio::AudioProcessing::default(),
            },
        }
    }

    fn device(node: &str, center_hz: f64, channels: Vec<WorkspaceChannel>) -> WorkspaceDevice {
        WorkspaceDevice {
            node: node.to_string(),
            settings: DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
            channels,
        }
    }

    #[test]
    fn merge_keeps_unobserved_nodes() {
        let mut state = WorkspaceState::new();
        state.merge(vec![
            device("a", 100.0, vec![channel("a1", 1000.0)]),
            device("b", 200.0, vec![]),
        ]);

        // Only "a" was bound this run; "b" is an unplugged radio.
        state.merge(vec![device("a", 101.0, vec![channel("a1", 2000.0)])]);

        assert_eq!(state.devices.len(), 2);
        assert_eq!(state.device("a").unwrap().settings.center_hz, Some(101.0));
        assert_eq!(state.channel("a1").unwrap().settings.offset_hz, 2000.0);
        assert_eq!(state.device("b").unwrap().settings.center_hz, Some(200.0));
    }

    #[test]
    fn merge_adds_new_channels_to_a_known_device() {
        let mut state = WorkspaceState::new();
        state.merge(vec![device("a", 100.0, vec![channel("a1", 1000.0)])]);
        state.merge(vec![device("a", 100.0, vec![channel("a2", 3000.0)])]);

        let saved = state.device("a").unwrap();
        assert_eq!(saved.channels.len(), 2);
        assert_eq!(state.channel("a2").unwrap().settings.offset_hz, 3000.0);
    }

    #[test]
    fn retain_nodes_forgets_deleted_ones() {
        let mut state = WorkspaceState::new();
        state.merge(vec![
            device(
                "a",
                100.0,
                vec![channel("a1", 1000.0), channel("a2", 2000.0)],
            ),
            device("b", 200.0, vec![]),
        ]);

        state.retain_nodes(|node| node != "b" && node != "a2");

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.device("a").unwrap().channels.len(), 1);
        assert!(state.channel("a2").is_none());
    }

    #[test]
    fn a_foreign_version_reads_as_empty() {
        let mut state = WorkspaceState::new();
        state.merge(vec![device("a", 100.0, vec![])]);
        state.version = WORKSPACE_STATE_VERSION + 1;

        assert!(state.current().devices.is_empty());
    }
}
