use std::collections::{HashMap, HashSet};

use sdrmm_wire::{
    channel::{ChannelInfo, ChannelParams},
    device::{Capabilities, DeviceSettings, StreamSettings, Tuning},
    patch::{NodeBody, PatchGraph},
    state::{DeviceSet, TrunkSystemStatus},
};

use super::{
    pick::format_mhz,
    view::{SpectrumView, offset_to_span},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    pub set: u32,
    pub stream: u32,
    pub device: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrunkOwner {
    pub node: String,
    pub role: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Radio {
    pub set: Option<DeviceSet>,
    pub channels: Vec<ChannelInfo>,
    pub faces: HashMap<u32, String>,
    pub owners: HashMap<u32, TrunkOwner>,
    pub locked: HashSet<u32>,
    pub centre_held: bool,
    pub on_auto: bool,
}

impl Radio {
    #[must_use]
    pub fn held(&self, channel: u32) -> bool {
        self.owners.contains_key(&channel) || self.locked.contains(&channel)
    }

    #[must_use]
    pub fn channel(&self, id: u32) -> Option<&ChannelInfo> {
        self.channels.iter().find(|channel| channel.id == id)
    }

    #[must_use]
    pub fn face_channel(&self, node: Option<&str>) -> Option<u32> {
        let node = node?;
        self.faces
            .iter()
            .find(|(_, face)| face.as_str() == node)
            .map(|(channel, _)| *channel)
    }
}

#[must_use]
pub fn trunk_roles(trunks: &[TrunkSystemStatus], set: u32) -> HashMap<u32, TrunkOwner> {
    let mut owners = HashMap::new();
    let mut own = |channel: u32, node: &str, role: &'static str| {
        owners.insert(
            channel,
            TrunkOwner {
                node: node.to_owned(),
                role,
            },
        );
    };
    for system in trunks {
        if let Some(control) = system
            .control
            .as_ref()
            .filter(|control| control.device_set == set)
        {
            own(control.channel, &system.node, "control");
        }
        for follower in system
            .followers
            .iter()
            .filter(|follower| follower.device_set == set)
        {
            own(follower.channel, &system.node, "call");
        }
        for probe in system.probes.iter().filter(|probe| probe.device_set == set) {
            own(probe.channel, &system.node, "search");
        }
    }
    owners
}

#[must_use]
pub fn tuning_locked(graph: &PatchGraph, node: &str, stream: u32) -> bool {
    graph.node(node).is_some_and(|found| match &found.body {
        NodeBody::Device(device) => device.locked_streams.contains(&stream),
        NodeBody::Channel(channel) => channel.tuning_locked,
        _ => false,
    })
}

#[must_use]
pub fn auto_tuning(set: &DeviceSet, stream: u32) -> bool {
    set.settings
        .for_stream(stream, &set.capabilities.per_stream)
        .tunes_itself()
}

#[must_use]
pub fn reachable_hz(capabilities: &Capabilities, hz: f64) -> f64 {
    let Some(first) = capabilities.freq_ranges.first() else {
        return hz.max(0.0);
    };
    capabilities
        .freq_ranges
        .iter()
        .map(|range| hz.clamp(range.min, range.max.max(range.min)))
        .fold(
            hz.clamp(first.min, first.max.max(first.min)),
            |best, held| {
                if (held - hz).abs() < (best - hz).abs() {
                    held
                } else {
                    best
                }
            },
        )
}

#[must_use]
pub fn tune_delta(capabilities: &Capabilities, stream: u32, hz: f64) -> DeviceSettings {
    let centre_hz = Some(reachable_hz(capabilities, hz));
    if capabilities.per_stream.tuning {
        return DeviceSettings {
            streams: vec![StreamSettings {
                stream,
                center_hz: centre_hz,
                tuning: Some(Tuning::Manual),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
    }
    DeviceSettings {
        center_hz: centre_hz,
        tuning: Some(Tuning::Manual),
        ..DeviceSettings::default()
    }
}

#[must_use]
pub fn bandwidth_hz(params: &ChannelParams) -> Option<f64> {
    serde_json::to_value(params)
        .ok()?
        .get("settings")?
        .get("bandwidth_hz")?
        .as_f64()
}

#[must_use]
pub fn marker_name(channel: &ChannelInfo, owner: Option<&TrunkOwner>) -> String {
    owner
        .map_or(channel.settings.params.type_id(), |owner| owner.role)
        .to_uppercase()
}

#[must_use]
pub fn marker_hint(
    channel: &ChannelInfo,
    hz: f64,
    owner: Option<&TrunkOwner>,
    locked: bool,
) -> String {
    let type_id = channel.settings.params.type_id();
    match owner {
        Some(owner) => format!(
            "trunk {} channel {}, tuned by its system",
            owner.role,
            format_mhz(hz)
        ),
        None if locked => format!("{type_id} channel {}, held", format_mhz(hz)),
        None => format!("{type_id} channel {}", format_mhz(hz)),
    }
}

#[must_use]
pub fn marker_at(
    channels: &[ChannelInfo],
    view: SpectrumView,
    centre_hz: f64,
    span_hz: f64,
    at: f64,
    tolerance: f64,
) -> Option<u32> {
    let mut best = None;
    let mut best_distance = tolerance;
    for channel in channels {
        let position = view.place(offset_to_span(
            channel.settings.frequency_hz - centre_hz,
            span_hz,
        ));
        let distance = (position - at).abs();
        if distance <= best_distance {
            best = Some(channel.id);
            best_distance = distance;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        device::{Range, StreamScope},
        patch::{ChannelNode, DeviceNode, PatchNode, Position},
    };

    use super::{super::view::FULL_VIEW, *};

    fn channel(id: u32, hz: f64, type_id: &str) -> ChannelInfo {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "stream": 0,
            "settings": {
                "frequency_hz": hz,
                "params": { "type": type_id, "settings": {} }
            },
            "out_of_band": false
        }))
        .expect("a channel")
    }

    fn capabilities(ranges: &[(f64, f64)], per_stream: bool) -> Capabilities {
        let mut caps: Capabilities = serde_json::from_value(serde_json::json!({
            "freq_ranges": [],
            "sample_rates": [],
            "gains": [],
            "antennas": [],
            "bandwidths": []
        }))
        .expect("capabilities");
        caps.freq_ranges = ranges
            .iter()
            .map(|(min, max)| Range {
                min: *min,
                max: *max,
                step: None,
            })
            .collect();
        caps.per_stream = StreamScope {
            tuning: per_stream,
            ..StreamScope::default()
        };
        caps
    }

    #[test]
    fn a_tune_lands_on_the_nearest_reachable_frequency() {
        let caps = capabilities(&[(24e6, 1.7e9), (2e9, 3e9)], false);
        assert_eq!(reachable_hz(&caps, 100e6), 100e6);
        assert_eq!(reachable_hz(&caps, 1e6), 24e6);
        assert_eq!(reachable_hz(&caps, 1.9e9), 2e9);
        assert_eq!(reachable_hz(&capabilities(&[], false), -5.0), 0.0);
    }

    #[test]
    fn a_tune_goes_to_the_lane_when_lanes_tune_apart() {
        let shared = tune_delta(&capabilities(&[], false), 1, 100e6);
        assert_eq!(shared.center_hz, Some(100e6));
        assert_eq!(shared.tuning, Some(Tuning::Manual));
        let apart = tune_delta(&capabilities(&[], true), 1, 100e6);
        assert_eq!(apart.center_hz, None);
        assert_eq!(apart.streams[0].stream, 1);
        assert_eq!(apart.streams[0].center_hz, Some(100e6));
    }

    #[test]
    fn a_marker_is_grabbed_within_reach_and_the_nearest_wins() {
        let listed = [channel(1, 100.2e6, "nfm"), channel(2, 100.25e6, "am")];
        let grab = |at| marker_at(&listed, FULL_VIEW, 100e6, 2e6, at, 0.03);
        assert_eq!(grab(0.6), Some(1));
        assert_eq!(grab(0.63), Some(2));
        assert_eq!(grab(0.9), None);
    }

    #[test]
    fn a_marker_names_its_mode_or_its_trunk_role() {
        let nfm = channel(1, 100e6, "nfm");
        let owner = TrunkOwner {
            node: String::from("trunk"),
            role: "control",
        };
        assert_eq!(marker_name(&nfm, None), "NFM");
        assert_eq!(marker_name(&nfm, Some(&owner)), "CONTROL");
        assert_eq!(
            marker_hint(&nfm, 100e6, None, true),
            "nfm channel 100.0000 MHz, held"
        );
    }

    #[test]
    fn a_channel_reports_the_bandwidth_its_settings_carry() {
        let nfm: ChannelParams =
            serde_json::from_value(serde_json::json!({ "type": "nfm", "settings": {} }))
                .expect("nfm");
        assert!(bandwidth_hz(&nfm).is_some_and(|hz| hz > 0.0));
    }

    #[test]
    fn locks_are_read_off_the_device_lane_and_the_channel() {
        let node = |id: &str, body| PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        };
        let graph = PatchGraph {
            nodes: vec![
                node(
                    "dev",
                    NodeBody::Device(DeviceNode {
                        device: None,
                        locked_streams: vec![1],
                    }),
                ),
                node(
                    "nfm",
                    NodeBody::Channel(ChannelNode {
                        channel_type: String::from("nfm"),
                        record_calls: false,
                        tuning_locked: true,
                    }),
                ),
            ],
            edges: Vec::new(),
        };
        assert!(!tuning_locked(&graph, "dev", 0));
        assert!(tuning_locked(&graph, "dev", 1));
        assert!(tuning_locked(&graph, "nfm", 0));
        assert!(!tuning_locked(&graph, "gone", 0));
    }
}
