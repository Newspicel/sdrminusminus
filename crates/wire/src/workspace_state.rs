use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{channel::ChannelSettings, device::DeviceSettings, patch::DmrChannelEntry};

pub const WORKSPACE_STATE_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceState {
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<WorkspaceDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunks: Vec<WorkspaceTrunk>,
}

/// A trunk system's channel plan as the server worked it out, kept apart from the patch so a
/// frequency the search confirmed is not an edit to what the operator drew.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceTrunk {
    pub node: String,
    /// Which site the plan belongs to. Neighbouring sites of one system repeat logical channel
    /// numbers on different frequencies, so a plan learned from one site is wrong for the next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u8>,
    #[serde(default)]
    pub channels: Vec<DmrChannelEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceDevice {
    pub node: String,
    pub settings: DeviceSettings,
    #[serde(default)]
    pub channels: Vec<WorkspaceChannel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceChannel {
    pub node: String,
    pub settings: ChannelSettings,
}

impl WorkspaceState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: WORKSPACE_STATE_VERSION,
            devices: Vec::new(),
            trunks: Vec::new(),
        }
    }

    #[must_use]
    pub fn current(self) -> Self {
        if self.version == WORKSPACE_STATE_VERSION {
            self
        } else {
            Self::new()
        }
    }

    #[must_use]
    pub fn device(&self, node: &str) -> Option<&WorkspaceDevice> {
        self.devices.iter().find(|device| device.node == node)
    }

    #[must_use]
    pub fn channel(&self, node: &str) -> Option<&WorkspaceChannel> {
        self.devices
            .iter()
            .flat_map(|device| &device.channels)
            .find(|channel| channel.node == node)
    }

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

    pub fn retain_nodes(&mut self, present: impl Fn(&str) -> bool) {
        self.devices.retain(|device| present(&device.node));
        for device in &mut self.devices {
            device.channels.retain(|channel| present(&channel.node));
        }
        self.trunks.retain(|trunk| present(&trunk.node));
    }

    #[must_use]
    pub fn trunk(&self, node: &str, color_code: Option<u8>) -> Option<&WorkspaceTrunk> {
        self.trunks
            .iter()
            .find(|trunk| trunk.node == node && trunk.color_code == color_code)
    }

    pub fn merge_trunks(&mut self, learned: Vec<WorkspaceTrunk>) {
        for trunk in learned {
            if trunk.channels.is_empty() {
                continue;
            }
            match self
                .trunks
                .iter_mut()
                .find(|held| held.node == trunk.node && held.color_code == trunk.color_code)
            {
                Some(held) => {
                    for channel in trunk.channels {
                        match held
                            .channels
                            .iter_mut()
                            .find(|kept| kept.lcn == channel.lcn)
                        {
                            Some(kept) => *kept = channel,
                            None => held.channels.push(channel),
                        }
                    }
                    held.channels.sort_unstable_by_key(|channel| channel.lcn);
                }
                None => self.trunks.push(trunk),
            }
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

    fn trunk(node: &str, color_code: Option<u8>, channels: &[(u16, u64)]) -> WorkspaceTrunk {
        WorkspaceTrunk {
            node: node.to_string(),
            color_code,
            channels: channels
                .iter()
                .map(|(lcn, freq_hz)| DmrChannelEntry {
                    lcn: *lcn,
                    freq_hz: *freq_hz,
                })
                .collect(),
        }
    }

    #[test]
    fn a_learned_channel_plan_survives_a_restart() {
        let mut state = WorkspaceState::new();
        state.merge_trunks(vec![trunk("sys", Some(3), &[(17, 451_012_500)])]);

        assert_eq!(
            state.trunk("sys", Some(3)).map(|held| held.channels.len()),
            Some(1)
        );
    }

    #[test]
    fn each_site_keeps_its_own_channel_plan() {
        let mut state = WorkspaceState::new();
        state.merge_trunks(vec![
            trunk("sys", Some(3), &[(17, 451_012_500)]),
            trunk("sys", Some(7), &[(17, 452_500_000)]),
        ]);

        assert_eq!(
            state
                .trunk("sys", Some(3))
                .map(|held| held.channels[0].freq_hz),
            Some(451_012_500)
        );
        assert_eq!(
            state
                .trunk("sys", Some(7))
                .map(|held| held.channels[0].freq_hz),
            Some(452_500_000),
            "a neighbouring site overwrote this one's plan"
        );
    }

    #[test]
    fn a_replanned_channel_replaces_the_frequency_it_used_to_have() {
        let mut state = WorkspaceState::new();
        state.merge_trunks(vec![trunk("sys", Some(3), &[(17, 451_012_500)])]);
        state.merge_trunks(vec![trunk(
            "sys",
            Some(3),
            &[(17, 451_050_000), (2, 451_000_000)],
        )]);

        let held = state.trunk("sys", Some(3)).expect("the plan");
        assert_eq!(held.channels.len(), 2);
        assert_eq!(held.channels[0].lcn, 2, "the plan was left unsorted");
        assert_eq!(held.channels[1].freq_hz, 451_050_000);
    }

    #[test]
    fn a_system_that_learned_nothing_is_not_written_down() {
        let mut state = WorkspaceState::new();
        state.merge_trunks(vec![trunk("sys", Some(3), &[])]);

        assert!(state.trunks.is_empty());
    }

    #[test]
    fn a_deleted_system_takes_its_channel_plan_with_it() {
        let mut state = WorkspaceState::new();
        state.merge_trunks(vec![trunk("sys", Some(3), &[(17, 451_012_500)])]);

        state.retain_nodes(|node| node != "sys");

        assert!(state.trunks.is_empty());
    }

    #[test]
    fn merge_keeps_unobserved_nodes() {
        let mut state = WorkspaceState::new();
        state.merge(vec![
            device("a", 100.0, vec![channel("a1", 1000.0)]),
            device("b", 200.0, vec![]),
        ]);

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
