use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::{channel::ChannelSettings, device::DeviceSettings, patch::DmrChannelEntry};

pub const WORKSPACE_STATE_VERSION: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct WorkspaceState {
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<WorkspaceDevice>,
    #[serde(default)]
    pub channels: Vec<WorkspaceChannel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trunks: Vec<WorkspaceTrunk>,
}

impl<'de> Deserialize<'de> for WorkspaceState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stated {
            version: u32,
            #[serde(default)]
            devices: Vec<WorkspaceDevice>,
            #[serde(default)]
            channels: Vec<WorkspaceChannel>,
            #[serde(default)]
            trunks: Vec<WorkspaceTrunk>,
        }
        let mut document = Value::deserialize(deserializer)?;
        lift_channels_to_the_top(&mut document);
        let stated = Stated::deserialize(document).map_err(serde::de::Error::custom)?;
        Ok(Self {
            version: stated.version,
            devices: stated.devices,
            channels: stated.channels,
            trunks: stated.trunks,
        })
    }
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
            channels: Vec::new(),
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
        self.channels.iter().find(|channel| channel.node == node)
    }

    pub fn merge(&mut self, captured: Vec<WorkspaceDevice>) {
        for device in captured {
            match self
                .devices
                .iter_mut()
                .find(|existing| existing.node == device.node)
            {
                Some(existing) => existing.settings = device.settings,
                None => self.devices.push(device),
            }
        }
    }

    pub fn merge_channels(&mut self, captured: Vec<WorkspaceChannel>) {
        for channel in captured {
            self.put_channel(&channel.node.clone(), channel.settings);
        }
    }

    /// Records what a channel node is set to. It hangs off the node and nothing else, so a decoder
    /// keeps its frequency while no radio is wired into it and while none can reach it.
    pub fn put_channel(&mut self, node: &str, settings: ChannelSettings) {
        match self.channels.iter_mut().find(|held| held.node == node) {
            Some(held) => held.settings = settings,
            None => self.channels.push(WorkspaceChannel {
                node: node.to_string(),
                settings,
            }),
        }
    }

    pub fn retain_nodes(&mut self, present: impl Fn(&str) -> bool) {
        self.devices.retain(|device| present(&device.node));
        self.channels.retain(|channel| present(&channel.node));
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

/// Version 1 hung a channel's settings off the radio that carried it and tuned it by an offset
/// from that radio's centre. Lifts each one onto the node itself, resolving the offset against the
/// centre it was taken from so the decoder comes back on the frequency it was actually hearing.
fn lift_channels_to_the_top(document: &mut Value) {
    if document.get("version").and_then(Value::as_u64) != Some(1) {
        return;
    }
    let mut lifted = Vec::new();
    if let Some(devices) = document.get_mut("devices").and_then(Value::as_array_mut) {
        for device in devices {
            let center_hz = device
                .get("settings")
                .and_then(|settings| settings.get("center_hz"))
                .and_then(Value::as_f64)
                .unwrap_or(crate::channel::DEFAULT_FREQUENCY_HZ);
            let Some(channels) = device
                .as_object_mut()
                .and_then(|device| device.remove("channels"))
            else {
                continue;
            };
            let Value::Array(channels) = channels else {
                continue;
            };
            for mut channel in channels {
                let offset_hz = channel
                    .get("settings")
                    .and_then(|settings| settings.get("offset_hz"))
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                if let Some(settings) = channel.get_mut("settings").and_then(Value::as_object_mut) {
                    settings.remove("offset_hz");
                    settings.insert("frequency_hz".to_string(), (center_hz + offset_hz).into());
                }
                lifted.push(channel);
            }
        }
    }
    let Some(document) = document.as_object_mut() else {
        return;
    };
    document.insert("channels".to_string(), Value::Array(lifted));
    document.insert("version".to_string(), WORKSPACE_STATE_VERSION.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::{ChannelParams, NfmParams};

    fn channel(node: &str, frequency_hz: f64) -> WorkspaceChannel {
        WorkspaceChannel {
            node: node.to_string(),
            settings: ChannelSettings {
                frequency_hz,
                squelch: crate::Squelch::Off,
                params: ChannelParams::Nfm(NfmParams::default()),
                audio: crate::audio::AudioProcessing::default(),
            },
        }
    }

    fn device(node: &str, center_hz: f64) -> WorkspaceDevice {
        WorkspaceDevice {
            node: node.to_string(),
            settings: DeviceSettings {
                center_hz: Some(center_hz),
                ..DeviceSettings::default()
            },
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
        state.merge(vec![device("a", 100.0), device("b", 200.0)]);
        state.merge_channels(vec![channel("a1", 145_000_000.0)]);

        state.merge(vec![device("a", 101.0)]);
        state.merge_channels(vec![channel("a1", 146_000_000.0)]);

        assert_eq!(state.devices.len(), 2);
        assert_eq!(state.device("a").unwrap().settings.center_hz, Some(101.0));
        assert_eq!(
            state.channel("a1").unwrap().settings.frequency_hz,
            146_000_000.0
        );
        assert_eq!(state.device("b").unwrap().settings.center_hz, Some(200.0));
    }

    #[test]
    fn a_channel_is_held_whether_or_not_a_radio_carries_it() {
        let mut state = WorkspaceState::new();
        state.put_channel("a1", channel("a1", 433_920_000.0).settings);

        assert!(state.devices.is_empty());
        assert_eq!(
            state.channel("a1").unwrap().settings.frequency_hz,
            433_920_000.0
        );
    }

    #[test]
    fn retain_nodes_forgets_deleted_ones() {
        let mut state = WorkspaceState::new();
        state.merge(vec![device("a", 100.0), device("b", 200.0)]);
        state.merge_channels(vec![
            channel("a1", 145_000_000.0),
            channel("a2", 146_000_000.0),
        ]);

        state.retain_nodes(|node| node != "b" && node != "a2");

        assert_eq!(state.devices.len(), 1);
        assert_eq!(state.channels.len(), 1);
        assert!(state.channel("a2").is_none());
    }

    #[test]
    fn a_foreign_version_reads_as_empty() {
        let mut state = WorkspaceState::new();
        state.merge(vec![device("a", 100.0)]);
        state.version = WORKSPACE_STATE_VERSION + 1;

        assert!(state.current().devices.is_empty());
    }

    #[test]
    fn an_offset_from_the_old_shape_comes_back_as_the_frequency_it_was_hearing() {
        let stored = r#"{
            "version": 1,
            "devices": [{
                "node": "radio",
                "settings": {"center_hz": 145000000.0},
                "channels": [{
                    "node": "voice",
                    "settings": {
                        "offset_hz": 500000.0,
                        "params": {"type": "nfm", "settings": {}}
                    }
                }]
            }]
        }"#;

        let state: WorkspaceState = serde_json::from_str(stored).expect("the stored state");

        assert_eq!(state.version, WORKSPACE_STATE_VERSION);
        assert_eq!(
            state.channel("voice").unwrap().settings.frequency_hz,
            145_500_000.0
        );
        assert_eq!(
            state.device("radio").unwrap().settings.center_hz,
            Some(145_000_000.0)
        );
    }

    #[test]
    fn a_current_document_is_read_as_it_stands() {
        let mut state = WorkspaceState::new();
        state.put_channel("voice", channel("voice", 433_920_000.0).settings);
        let json = serde_json::to_string(&state).expect("serialised state");

        let read: WorkspaceState = serde_json::from_str(&json).expect("the stored state");

        assert_eq!(read, state);
    }
}
