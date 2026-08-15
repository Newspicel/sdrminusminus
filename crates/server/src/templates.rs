use std::sync::LazyLock;

use sdrmm_wire::{
    AdsbParams, AisParams, AmParams, AprsParams, AudioProcessing, ChannelNode, ChannelParams,
    ChannelSettings, DeviceNode, GnssParams, NfmParams, NodeBody, PatchEdge, PatchGraph, PatchNode,
    PocsagParams, PortRef, Position, RadioClockParams, TemplateInfo, WfmParams,
};

type Channel = (f64, fn() -> ChannelParams);

struct Entry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    explainer: &'static str,
    center_hz: f64,
    sample_rate: f64,
    channels: &'static [Channel],
    shape: Shape,
    readout: bool,
    exact_rate: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Listen,
    Map,
    Log,
}

const COLUMN: f32 = 400.0;
const ROW: f32 = 240.0;

fn patch(shape: Shape, readout: bool, channels: &[Channel]) -> PatchGraph {
    let node = |id: &str, body: NodeBody, x: f32, y: f32| PatchNode {
        id: id.to_string(),
        body,
        position: Position { x, y },
        size: None,
        label: None,
    };
    let wire = |from: (&str, &str), to: (&str, &str)| PatchEdge {
        from: PortRef {
            node: from.0.to_string(),
            port: from.1.to_string(),
        },
        to: PortRef {
            node: to.0.to_string(),
            port: to.1.to_string(),
        },
    };

    let mut nodes = vec![
        node("dev", NodeBody::Device(DeviceNode::default()), 0.0, 0.0),
        node("scope", NodeBody::Scope, COLUMN, -40.0),
    ];
    let mut edges = vec![wire(("dev", "iq"), ("scope", "iq"))];

    for (index, (_, params)) in channels.iter().enumerate() {
        let id = format!("ch{index}");
        #[expect(
            clippy::cast_precision_loss,
            reason = "a template has a handful of channels; the row index is exact in f32"
        )]
        let y = 260.0 + index as f32 * ROW;
        nodes.push(node(
            &id,
            NodeBody::Channel(ChannelNode {
                channel_type: params().type_id().to_string(),
            }),
            COLUMN,
            y,
        ));
        edges.push(wire(("dev", "iq"), (&id, "iq")));
        match shape {
            Shape::Listen => edges.push(wire((&id, "audio"), ("speaker", "audio"))),
            Shape::Map => {
                edges.push(wire((&id, "events"), ("map", "events")));
                edges.push(wire((&id, "events"), ("log", "events")));
            }
            Shape::Log => edges.push(wire((&id, "events"), ("log", "events"))),
        }
        if readout {
            edges.push(wire((&id, "events"), ("readout", "events")));
        }
    }

    match shape {
        Shape::Listen => nodes.push(node("speaker", NodeBody::Speaker, COLUMN * 2.0, 260.0)),
        Shape::Map => {
            nodes.push(node("map", NodeBody::Map, COLUMN * 2.0, -40.0));
            nodes.push(node("log", NodeBody::DecoderLog, COLUMN * 2.0, 380.0));
        }
        Shape::Log => nodes.push(node("log", NodeBody::DecoderLog, COLUMN * 2.0, 260.0)),
    }
    if readout {
        let x = if shape == Shape::Listen {
            COLUMN * 2.0
        } else {
            COLUMN * 3.0
        };
        nodes.push(node("readout", NodeBody::Readout, x, -40.0));
    }
    PatchGraph { nodes, edges }
}

static TEMPLATES: &[Entry] = &[
    Entry {
        id: "fm-radio",
        name: "FM radio",
        description: "Wideband FM broadcast with RDS station text.",
        explainer: "The wide humps across the display are FM stations, each about 200 kHz \
                    across. The channel is tuned to one of them; RDS decodes the station name \
                    and radio text from a subcarrier you cannot hear. Drag the channel marker \
                    or edit its offset to move between stations.",
        center_hz: 98_000_000.0,
        sample_rate: 2_400_000.0,
        channels: &[(98_000_000.0, || ChannelParams::Wfm(WfmParams::default()))],
        shape: Shape::Listen,
        readout: true,
        exact_rate: false,
    },
    Entry {
        id: "airband",
        name: "Airband",
        description: "AM aircraft voice — tower, approach and ground.",
        explainer: "Civil aviation voice is AM between 118 and 137 MHz, 25 kHz apart. \
                    Transmissions are short and sporadic: leave the squelch on and wait. \
                    The 2.4 MHz window covers roughly 117–119.3 MHz, so tower, ground and \
                    approach are usually all reachable by moving the channel offset alone. \
                    121.5 MHz is the emergency frequency and is normally silent.",
        center_hz: 118_100_000.0,
        sample_rate: 2_400_000.0,
        channels: &[(118_100_000.0, || ChannelParams::Am(AmParams::default()))],
        shape: Shape::Listen,
        readout: false,
        exact_rate: false,
    },
    Entry {
        id: "adsb",
        name: "Aircraft (ADS-B)",
        description: "1090 MHz aircraft positions on the map.",
        explainer: "Aircraft broadcast their identity, altitude, speed and position on \
                    1090 MHz. Reception is line-of-sight, so range depends on antenna height \
                    far more than on gain. ADS-B is handed the device's own samples, so the \
                    radio has to run at 2 Msps or above.",
        center_hz: 1_090_000_000.0,
        sample_rate: 2_000_000.0,
        channels: &[(1_090_000_000.0, || {
            ChannelParams::Adsb(AdsbParams::default())
        })],
        shape: Shape::Map,
        readout: true,
        exact_rate: false,
    },
    Entry {
        id: "ais",
        name: "Ships (AIS)",
        description: "Marine AIS positions on the map.",
        explainer: "Ships broadcast position and identity on two VHF channels, 161.975 and \
                    162.025 MHz. Both channels are decoded at once. Coverage is line-of-sight \
                    over water, which is usually far better than over land.",
        center_hz: 162_000_000.0,
        sample_rate: 2_400_000.0,
        channels: &[
            (161_975_000.0, || ChannelParams::Ais(AisParams::default())),
            (162_025_000.0, || {
                ChannelParams::Ais(AisParams {
                    ais_channel: sdrmm_wire::AisChannel::B,
                })
            }),
        ],
        shape: Shape::Map,
        readout: true,
        exact_rate: false,
    },
    Entry {
        id: "aprs",
        name: "APRS",
        description: "Amateur position and message packets (144.800 MHz).",
        explainer: "APRS is AX.25 packet radio at 1200 baud AFSK. Each burst is a short \
                    chirp; stations report position, weather or messages. 144.800 MHz is the \
                    European calling frequency — North America uses 144.390 MHz.",
        center_hz: 144_800_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(144_800_000.0, || ChannelParams::Aprs(AprsParams::default()))],
        shape: Shape::Map,
        readout: false,
        exact_rate: false,
    },
    Entry {
        id: "pagers",
        name: "Pagers (POCSAG)",
        description: "POCSAG pager messages, 512/1200/2400 baud.",
        explainer: "Pager traffic still carries hospital, industrial and emergency dispatch \
                    messages. The decoder locks onto whichever of the three baud rates a \
                    transmission uses. Frequencies are regional — 466 MHz is common in \
                    Europe; check your national allocation.",
        center_hz: 466_075_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(466_075_000.0, || {
            ChannelParams::Pocsag(PocsagParams::default())
        })],
        shape: Shape::Log,
        readout: false,
        exact_rate: false,
    },
    Entry {
        id: "radio-clock",
        name: "Radio clock",
        description: "DCF77 civil time and minute-code inspection.",
        explainer: "DCF77 lowers its 77.5 kHz carrier once per second. The pulse width carries \
                    the next minute's time and date, with separate parity checks for minute, \
                    hour and calendar fields. Select WWVB, MSF or JJY in the channel settings \
                    and retune to the service used in your region.",
        center_hz: 77_500.0,
        sample_rate: 240_000.0,
        channels: &[(77_500.0, || {
            ChannelParams::RadioClock(RadioClockParams::default())
        })],
        shape: Shape::Log,
        readout: false,
        exact_rate: false,
    },
    Entry {
        id: "gnss-lab",
        name: "GNSS lab",
        description: "Inspect GPS L1 C/A acquisition and NAV subframes.",
        explainer: "This educational receiver searches one GPS PRN at a time so the code \
                    phase, Doppler and correlation strength stay visible. After acquisition \
                    it checks the 50 bit/s navigation-word parity and reports subframe ID, \
                    time of week and the truncated GPS week from subframe 1. It is not a \
                    navigation-grade position solution.",
        center_hz: 1_575_420_000.0,
        sample_rate: 2_048_000.0,
        channels: &[(1_575_420_000.0, || {
            ChannelParams::Gnss(GnssParams::default())
        })],
        shape: Shape::Log,
        readout: false,
        exact_rate: true,
    },
    Entry {
        id: "ham-2m",
        name: "Ham 2 m",
        description: "The 2 metre amateur band, FM repeater output segment.",
        explainer: "145.600–145.800 MHz carries FM repeater outputs, 12.5 kHz apart. Point \
                    the scanner at this range to find the active ones: a repeater is silent \
                    between overs, so a sweep finds far more than sitting on one channel.",
        center_hz: 145_700_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(145_700_000.0, || ChannelParams::Nfm(NfmParams::default()))],
        shape: Shape::Listen,
        readout: false,
        exact_rate: false,
    },
    Entry {
        id: "marine-vhf",
        name: "Marine VHF",
        description: "Marine voice channels around 156–162 MHz.",
        explainer: "Channel 16 (156.800 MHz) is the international distress and calling \
                    channel and is monitored everywhere; working traffic moves off it. Marine \
                    VHF is narrowband FM, 25 kHz apart.",
        center_hz: 156_800_000.0,
        sample_rate: 1_024_000.0,
        channels: &[(156_800_000.0, || ChannelParams::Nfm(NfmParams::default()))],
        shape: Shape::Listen,
        readout: false,
        exact_rate: false,
    },
];

#[must_use]
pub(crate) fn all() -> &'static [TemplateInfo] {
    static BUILT: LazyLock<Vec<TemplateInfo>> = LazyLock::new(|| {
        TEMPLATES
            .iter()
            .map(|entry| {
                let channels: Vec<ChannelSettings> = entry
                    .channels
                    .iter()
                    .map(|(freq_hz, params)| {
                        let params = params();
                        ChannelSettings {
                            offset_hz: freq_hz - entry.center_hz,
                            squelch_db: None,
                            squelch_auto_db: None,
                            audio: AudioProcessing::default_for(params.type_id()),
                            params,
                        }
                    })
                    .collect();
                let freqs = entry.channels.iter().map(|(f, _)| *f);
                let min = freqs.clone().fold(f64::INFINITY, f64::min);
                let max = freqs.fold(f64::NEG_INFINITY, f64::max);
                TemplateInfo {
                    id: entry.id.to_string(),
                    name: entry.name.to_string(),
                    description: entry.description.to_string(),
                    explainer: entry.explainer.to_string(),
                    center_hz: entry.center_hz,
                    sample_rate: entry.sample_rate,
                    channels,
                    min_freq_hz: min.min(entry.center_hz),
                    max_freq_hz: max.max(entry.center_hz),
                    patch: Some(patch(entry.shape, entry.readout, entry.channels)),
                    direction: sdrmm_wire::Direction::Rx,
                    exact_rate: entry.exact_rate,
                    supported_devices: Vec::new(),
                }
            })
            .collect()
    });
    &BUILT
}

#[must_use]
pub(crate) fn get(id: &str) -> Option<&'static TemplateInfo> {
    all().iter().find(|t| t.id == id)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::WorkspaceSnapshot;

    use super::*;

    #[test]
    fn ids_are_unique_and_slug_shaped() {
        let mut seen = std::collections::HashSet::new();
        for template in all() {
            assert!(seen.insert(&template.id), "duplicate id {}", template.id);
            assert!(
                template
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "unslugly id {}",
                template.id
            );
            assert!(!template.name.is_empty());
            assert!(!template.explainer.is_empty());
        }
    }

    #[test]
    fn every_channel_fits_its_templates_passband() {
        for template in all() {
            let usable = template.sample_rate * 0.4;
            for channel in &template.channels {
                assert!(
                    channel.offset_hz.abs() < usable,
                    "{}: channel at {} Hz is outside the flat ±{usable} Hz",
                    template.id,
                    channel.offset_hz
                );
            }
            assert!(template.min_freq_hz <= template.max_freq_hz);
        }
    }

    #[test]
    fn adsb_template_runs_the_device_at_the_channel_rate() {
        let adsb = get("adsb").expect("adsb template");
        assert_eq!(adsb.sample_rate, 2_000_000.0);
        assert!(!adsb.exact_rate);
        assert_eq!(adsb.channels.len(), 1);
        assert_eq!(adsb.channels[0].offset_hz, 0.0);
    }

    #[test]
    fn every_template_patch_is_a_valid_workspace() {
        let descriptors = sdrmm_engine::Engine::new(None).channel_types();
        for template in all() {
            let patch = template.patch.clone().expect("templates carry a patch");
            patch
                .validate_against(&descriptors)
                .unwrap_or_else(|e| panic!("{}: {e}", template.id));

            let mut workspace = WorkspaceSnapshot::starter();
            workspace.merge_patch(&patch, &format!("template:{}:", template.id), None);
            workspace
                .validate()
                .unwrap_or_else(|e| panic!("{} merged: {e}", template.id));
        }
    }

    #[test]
    fn a_readout_template_wires_every_channel_into_the_readout() {
        for entry in TEMPLATES {
            let patch = patch(entry.shape, entry.readout, entry.channels);
            let drawn = patch
                .nodes
                .iter()
                .any(|node| matches!(node.body, NodeBody::Readout));
            assert_eq!(drawn, entry.readout, "{}", entry.id);

            let wired = patch
                .edges
                .iter()
                .filter(|edge| edge.to.node == "readout" && edge.to.port == "events")
                .count();
            assert_eq!(
                wired,
                if entry.readout {
                    entry.channels.len()
                } else {
                    0
                },
                "{}",
                entry.id
            );
        }
    }

    #[test]
    fn every_patch_draws_the_channels_the_template_creates() {
        for template in all() {
            let patch = template.patch.clone().expect("templates carry a patch");
            let drawn: Vec<&str> = patch
                .channels_of("dev")
                .filter_map(|(node, stream)| match &node.body {
                    NodeBody::Channel(channel) => {
                        assert_eq!(stream, 0, "{} wires a non-zero stream", template.id);
                        Some(channel.channel_type.as_str())
                    }
                    _ => None,
                })
                .collect();
            let created: Vec<&str> = template
                .channels
                .iter()
                .map(|c| c.params.type_id())
                .collect();
            assert_eq!(drawn, created, "{}", template.id);
        }
    }

    #[test]
    fn unknown_ids_are_none() {
        assert!(get("nope").is_none());
        assert!(get("").is_none());
    }
}
