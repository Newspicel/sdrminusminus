use sdrmm_wire::{HuntStatus, channel::ChannelInfo, patch::PatchGraph, state::DeviceSet};

use crate::binding;

pub const HUNT_INTERVAL_MS: u32 = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct HuntTarget {
    pub set: DeviceSet,
    pub channel: ChannelInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bearing {
    Waiting,
    Closing,
    Leaving,
    Steady,
}

impl Bearing {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Waiting => "listening",
            Self::Closing => "warmer",
            Self::Leaving => "colder",
            Self::Steady => "on top of it",
        }
    }
}

#[must_use]
pub fn hunt_target(graph: &PatchGraph, sets: &[DeviceSet], node: &str) -> Option<HuntTarget> {
    let decoder = binding::controlled_node_of(graph, node)?;
    let devices = binding::device_sets(graph, sets);
    let channel = binding::channels(graph, sets, &devices).remove(&decoder)?;
    let owner = binding::device_node_of(graph, &decoder)?;
    let set_id = devices.get(&owner)?;
    let set = sets.iter().find(|set| set.id == *set_id)?.clone();
    Some(HuntTarget { set, channel })
}

#[must_use]
pub fn live_hunt(
    set: Option<&DeviceSet>,
    channel: Option<u32>,
    pushed: Option<&HuntStatus>,
) -> Option<HuntStatus> {
    let listed = set?
        .hunts
        .iter()
        .find(|hunt| Some(hunt.settings.channel) == channel)?;
    Some(pushed.unwrap_or(listed).clone())
}

#[must_use]
pub fn hunt_refusal(target: Option<&HuntTarget>) -> Option<&'static str> {
    let target = target?;
    if target
        .set
        .scanners
        .iter()
        .any(|scanner| scanner.settings.channel == target.channel.id)
    {
        return Some("This decoder is scanning. Stop the scan to hunt on one frequency.");
    }
    if target.channel.out_of_band {
        return Some(
            "The radio is tuned away from this decoder. Unlock its tuning or move it there.",
        );
    }
    None
}

#[must_use]
pub fn hunted_hz(status: Option<&HuntStatus>, channel: Option<&ChannelInfo>) -> Option<f64> {
    match status {
        Some(status) if status.freq_hz > 0.0 => Some(status.freq_hz),
        _ => channel.map(|channel| channel.settings.frequency_hz),
    }
}

#[must_use]
pub fn bearing(status: Option<&HuntStatus>) -> Bearing {
    let Some(status) = status else {
        return Bearing::Waiting;
    };
    if status.readings < 2 || status.smooth_db.is_none() {
        return Bearing::Waiting;
    }
    if status.closing {
        Bearing::Closing
    } else if status.strength >= 0.9 {
        Bearing::Steady
    } else {
        Bearing::Leaving
    }
}

#[must_use]
pub fn format_strength(status: Option<&HuntStatus>) -> String {
    match status {
        Some(status) if status.readings > 0 => format!("{:.0}%", status.strength * 100.0),
        _ => String::from("-"),
    }
}

#[must_use]
pub fn format_hunt_db(db: Option<f32>) -> String {
    match db {
        Some(db) if db.is_finite() => format!("{:.1} dB", (db * 10.0).round() / 10.0),
        _ => String::from("-"),
    }
}

#[must_use]
pub fn format_mhz(hz: Option<f64>) -> String {
    hz.map_or_else(|| String::from("-"), |hz| format!("{:.4} MHz", hz / 1e6))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{
        HuntSettings,
        patch::{
            ChannelNode, DeviceNode, DeviceRef, HuntNode, NodeBody, PatchEdge, PatchNode, PortRef,
            Position,
        },
    };

    use super::*;

    fn hunt() -> HuntStatus {
        HuntStatus {
            settings: HuntSettings::for_channel(9),
            freq_hz: 433_920_000.0,
            bw_hz: 12_500.0,
            level_db: Some(-60.0),
            smooth_db: Some(-61.0),
            floor_db: Some(-90.0),
            best_db: Some(-40.0),
            strength: 0.5,
            closing: false,
            readings: 10,
            error: None,
        }
    }

    fn channel(out_of_band: bool) -> ChannelInfo {
        serde_json::from_value(serde_json::json!({
            "id": 9,
            "stream": 0,
            "settings": { "frequency_hz": 433_920_000.0, "params": { "type": "nfm", "settings": {} } },
            "out_of_band": out_of_band
        }))
        .expect("a channel")
    }

    fn set(hunts: Vec<HuntStatus>, channels: Vec<ChannelInfo>, scanning: Option<u32>) -> DeviceSet {
        let mut set: DeviceSet = serde_json::from_value(serde_json::json!({
            "id": 1,
            "device": { "driver": "virtual", "key": "siggen", "label": "Signal generator" },
            "capabilities": { "freq_ranges": [], "sample_rates": [], "gains": [], "antennas": [], "bandwidths": [] },
            "settings": {},
            "status": "running",
            "channels": []
        }))
        .expect("a set");
        set.hunts = hunts;
        set.channels = channels;
        if let Some(channel) = scanning {
            set.scanners = vec![
                serde_json::from_value(serde_json::json!({
                    "state": "scanning",
                    "settings": { "channel": channel, "ranges": [] },
                    "targets": 1,
                    "current_hz": 1.0,
                    "sweeps": 0,
                    "hits": 0
                }))
                .expect("a scanner"),
            ];
        }
        set
    }

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
        PatchEdge {
            from: PortRef {
                node: from.0.to_owned(),
                port: from.1.to_owned(),
            },
            to: PortRef {
                node: to.0.to_owned(),
                port: to.1.to_owned(),
            },
        }
    }

    fn graph() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node(
                    "dev",
                    NodeBody::Device(DeviceNode {
                        device: Some(DeviceRef {
                            backend: "virtual".to_owned(),
                            serial: None,
                            key: Some("siggen".to_owned()),
                        }),
                        locked_streams: Vec::new(),
                    }),
                ),
                node(
                    "nfm",
                    NodeBody::Channel(ChannelNode {
                        channel_type: "nfm".to_owned(),
                        record_calls: false,
                        tuning_locked: false,
                    }),
                ),
                node("hunt", NodeBody::Hunt(HuntNode { clicks: false })),
                node("bare", NodeBody::Hunt(HuntNode { clicks: true })),
            ],
            edges: vec![
                edge(("dev", "iq"), ("nfm", "iq")),
                edge(("hunt", "control"), ("nfm", "control")),
            ],
        }
    }

    #[test]
    fn prefers_the_pushed_reading_over_the_snapshot() {
        let listed = set(vec![hunt()], Vec::new(), None);
        assert_eq!(live_hunt(Some(&listed), Some(9), None), Some(hunt()));
        let fresher = HuntStatus {
            readings: 99,
            ..hunt()
        };
        assert_eq!(
            live_hunt(Some(&listed), Some(9), Some(&fresher)).map(|status| status.readings),
            Some(99)
        );
    }

    #[test]
    fn reports_nothing_when_the_decoder_is_not_hunted() {
        let listed = set(vec![hunt()], Vec::new(), None);
        assert_eq!(live_hunt(Some(&listed), Some(3), Some(&hunt())), None);
        assert_eq!(
            live_hunt(
                Some(&set(Vec::new(), Vec::new(), None)),
                Some(9),
                Some(&hunt())
            ),
            None
        );
        assert_eq!(live_hunt(None, Some(9), Some(&hunt())), None);
    }

    #[test]
    fn says_why_a_hunt_cannot_start_rather_than_failing_at_the_server() {
        assert_eq!(hunt_refusal(None), None);
        let plain = HuntTarget {
            set: set(Vec::new(), Vec::new(), None),
            channel: channel(false),
        };
        assert_eq!(hunt_refusal(Some(&plain)), None);
        let away = HuntTarget {
            channel: channel(true),
            ..plain.clone()
        };
        assert!(hunt_refusal(Some(&away)).is_some_and(|said| said.contains("tuned away")));
        let scanning = HuntTarget {
            set: set(Vec::new(), Vec::new(), Some(9)),
            ..plain.clone()
        };
        assert!(hunt_refusal(Some(&scanning)).is_some_and(|said| said.contains("scanning")));
        let elsewhere = HuntTarget {
            set: set(Vec::new(), Vec::new(), Some(2)),
            ..plain
        };
        assert_eq!(hunt_refusal(Some(&elsewhere)), None);
    }

    #[test]
    fn shows_the_decoder_frequency_until_a_reading_says_otherwise() {
        let decoder = channel(false);
        assert_eq!(hunted_hz(None, None), None);
        assert_eq!(hunted_hz(None, Some(&decoder)), Some(433_920_000.0));
        let moved = HuntStatus {
            freq_hz: 145_500_000.0,
            ..hunt()
        };
        assert_eq!(hunted_hz(Some(&moved), Some(&decoder)), Some(145_500_000.0));
        let unread = HuntStatus {
            freq_hz: 0.0,
            ..hunt()
        };
        assert_eq!(
            hunted_hz(Some(&unread), Some(&decoder)),
            Some(433_920_000.0)
        );
    }

    #[test]
    fn waits_for_enough_readings_before_pointing_anywhere() {
        assert_eq!(bearing(None), Bearing::Waiting);
        assert_eq!(
            bearing(Some(&HuntStatus {
                readings: 1,
                ..hunt()
            })),
            Bearing::Waiting
        );
        assert_eq!(
            bearing(Some(&HuntStatus {
                smooth_db: None,
                ..hunt()
            })),
            Bearing::Waiting
        );
    }

    #[test]
    fn calls_warmer_colder_and_on_top_of_it() {
        assert_eq!(
            bearing(Some(&HuntStatus {
                closing: true,
                ..hunt()
            })),
            Bearing::Closing
        );
        assert_eq!(
            bearing(Some(&HuntStatus {
                strength: 0.3,
                ..hunt()
            })),
            Bearing::Leaving
        );
        assert_eq!(
            bearing(Some(&HuntStatus {
                strength: 0.95,
                ..hunt()
            })),
            Bearing::Steady
        );
        assert_eq!(Bearing::Steady.label(), "on top of it");
    }

    #[test]
    fn shows_a_dash_rather_than_a_number_nobody_measured() {
        assert_eq!(format_strength(None), "-");
        assert_eq!(
            format_strength(Some(&HuntStatus {
                readings: 0,
                ..hunt()
            })),
            "-"
        );
        assert_eq!(format_strength(Some(&hunt())), "50%");
        assert_eq!(format_hunt_db(None), "-");
        assert_eq!(format_hunt_db(Some(f32::NAN)), "-");
        assert_eq!(format_hunt_db(Some(-61.25)), "-61.3 dB");
        assert_eq!(format_mhz(Some(433_920_000.0)), "433.9200 MHz");
        assert_eq!(format_mhz(None), "-");
    }

    #[test]
    fn finds_the_decoder_a_hunt_drives_and_its_radio() {
        let open = set(Vec::new(), vec![channel(false)], None);
        let found = hunt_target(&graph(), std::slice::from_ref(&open), "hunt");
        assert_eq!(
            found.map(|target| (target.set.id, target.channel.id)),
            Some((1, 9))
        );
        assert_eq!(
            hunt_target(&graph(), std::slice::from_ref(&open), "bare"),
            None
        );
        assert_eq!(hunt_target(&graph(), &[], "hunt"), None);
        assert_eq!(
            hunt_target(&graph(), &[set(Vec::new(), Vec::new(), None)], "hunt"),
            None
        );
    }
}
