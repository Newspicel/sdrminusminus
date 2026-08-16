use std::sync::LazyLock;

use sdrmm_wire::{
    AcarsParams, AdsbParams, AisParams, AmParams, AprsParams, AudioProcessing, ChannelNode,
    ChannelParams, ChannelSettings, DabParams, DeviceNode, DmrParams, DstarParams, ErmesParams,
    FlexParams, GnssParams, IdentParams, M17Params, MorseParams, NavtexParams, NfmParams, NodeBody,
    PatchEdge, PatchGraph, PatchNode, PocsagParams, PortRef, Position, PskParams, RadioClockParams,
    RttyParams, SsbParams, SstvParams, SubghzParams, TemplateInfo, WfmParams, WsjtParams,
    WsprParams, YsfParams,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    Readout,
    Speaker,
    Video,
    Map,
    Log,
}

impl Sink {
    const fn node(self) -> &'static str {
        match self {
            Self::Readout => "readout",
            Self::Speaker => "speaker",
            Self::Video => "video",
            Self::Map => "map",
            Self::Log => "log",
        }
    }

    const fn body(self) -> NodeBody {
        match self {
            Self::Readout => NodeBody::Readout,
            Self::Speaker => NodeBody::Speaker,
            Self::Video => NodeBody::Video,
            Self::Map => NodeBody::Map,
            Self::Log => NodeBody::DecoderLog,
        }
    }

    const fn port(self) -> &'static str {
        match self {
            Self::Speaker => "audio",
            Self::Video => "video",
            Self::Readout | Self::Map | Self::Log => "events",
        }
    }
}

const ORDER: &[Sink] = &[
    Sink::Readout,
    Sink::Speaker,
    Sink::Video,
    Sink::Map,
    Sink::Log,
];

const LISTEN: &[Sink] = &[Sink::Speaker];
const LISTEN_READ: &[Sink] = &[Sink::Speaker, Sink::Readout];
const LISTEN_LOG: &[Sink] = &[Sink::Speaker, Sink::Log];
const TRACK: &[Sink] = &[Sink::Map, Sink::Log];
const TRACK_READ: &[Sink] = &[Sink::Map, Sink::Log, Sink::Readout];
const LOG: &[Sink] = &[Sink::Log];
const LOG_READ: &[Sink] = &[Sink::Log, Sink::Readout];
const WATCH: &[Sink] = &[Sink::Video, Sink::Log];

struct Channel {
    freq_hz: f64,
    params: fn() -> ChannelParams,
    sinks: &'static [Sink],
    squelch_auto_db: Option<f32>,
}

impl Channel {
    const fn at(freq_hz: f64, params: fn() -> ChannelParams, sinks: &'static [Sink]) -> Self {
        Self {
            freq_hz,
            params,
            sinks,
            squelch_auto_db: None,
        }
    }

    const fn squelched(self, margin_db: f32) -> Self {
        Self {
            squelch_auto_db: Some(margin_db),
            ..self
        }
    }
}

struct Entry {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    explainer: &'static str,
    center_hz: f64,
    sample_rate: f64,
    channels: &'static [Channel],
    exact_rate: bool,
}

const COLUMN: f32 = 400.0;
const ROW: f32 = 240.0;
const SINK_ROW: f32 = 420.0;

fn patch(channels: &[Channel]) -> PatchGraph {
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

    for (index, channel) in channels.iter().enumerate() {
        let id = format!("ch{index}");
        #[expect(
            clippy::cast_precision_loss,
            reason = "a template has a handful of channels; the row index is exact in f32"
        )]
        let y = 260.0 + index as f32 * ROW;
        nodes.push(node(
            &id,
            NodeBody::Channel(ChannelNode {
                channel_type: (channel.params)().type_id().to_string(),
            }),
            COLUMN,
            y,
        ));
        edges.push(wire(("dev", "iq"), (&id, "iq")));
        for sink in channel.sinks {
            edges.push(wire((&id, sink.port()), (sink.node(), sink.port())));
        }
    }

    let used = ORDER
        .iter()
        .filter(|sink| channels.iter().any(|c| c.sinks.contains(sink)));
    for (index, sink) in used.enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a template draws at most five sinks; the row index is exact in f32"
        )]
        let y = -40.0 + index as f32 * SINK_ROW;
        nodes.push(node(sink.node(), sink.body(), COLUMN * 2.0, y));
    }
    PatchGraph { nodes, edges }
}

fn acars() -> ChannelParams {
    ChannelParams::Acars(AcarsParams::default())
}

fn fm_voice() -> ChannelParams {
    ChannelParams::Nfm(NfmParams::default())
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
        channels: &[Channel::at(
            98_000_000.0,
            || ChannelParams::Wfm(WfmParams::default()),
            LISTEN_READ,
        )],
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
        channels: &[Channel::at(
            118_100_000.0,
            || ChannelParams::Am(AmParams::default()),
            LISTEN,
        )],
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
        channels: &[Channel::at(
            1_090_000_000.0,
            || ChannelParams::Adsb(AdsbParams::default()),
            TRACK_READ,
        )],
        exact_rate: false,
    },
    Entry {
        id: "acars",
        name: "Aircraft data (ACARS)",
        description: "Airline datalink text on the VHF ACARS channels.",
        explainer: "ACARS carries short text between aircraft and their airline: position \
                    reports, weather, link tests, gate changes and maintenance messages. The \
                    allocation is regional, so five channels are decoded at once — 131.550 MHz \
                    is the worldwide primary, 131.725 MHz serves Europe and 131.425 MHz the \
                    Asia-Pacific region. Traffic is bursty: near a busy airport expect a \
                    message every few seconds, elsewhere a handful per hour.",
        center_hz: 131_637_500.0,
        sample_rate: 1_024_000.0,
        channels: &[
            Channel::at(131_425_000.0, acars, LOG),
            Channel::at(131_525_000.0, acars, LOG),
            Channel::at(131_550_000.0, acars, LOG),
            Channel::at(131_725_000.0, acars, LOG),
            Channel::at(131_850_000.0, acars, LOG),
        ],
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
            Channel::at(
                161_975_000.0,
                || ChannelParams::Ais(AisParams::default()),
                TRACK_READ,
            ),
            Channel::at(
                162_025_000.0,
                || {
                    ChannelParams::Ais(AisParams {
                        ais_channel: sdrmm_wire::AisChannel::B,
                    })
                },
                TRACK_READ,
            ),
        ],
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
        channels: &[Channel::at(
            144_800_000.0,
            || ChannelParams::Aprs(AprsParams::default()),
            TRACK,
        )],
        exact_rate: false,
    },
    Entry {
        id: "pagers",
        name: "Pagers",
        description: "POCSAG, FLEX and ERMES pager messages.",
        explainer: "Pager traffic still carries hospital, industrial and emergency dispatch \
                    messages. The three decoders cover POCSAG at every standard baud rate, \
                    all four FLEX modes and European ERMES. Frequencies are regional; check \
                    your national allocation before tuning.",
        center_hz: 466_075_000.0,
        sample_rate: 1_024_000.0,
        channels: &[
            Channel::at(
                466_075_000.0,
                || ChannelParams::Pocsag(PocsagParams::default()),
                LOG,
            ),
            Channel::at(
                466_075_000.0,
                || ChannelParams::Flex(FlexParams::default()),
                LOG,
            ),
            Channel::at(
                466_075_000.0,
                || ChannelParams::Ermes(ErmesParams::default()),
                LOG,
            ),
        ],
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
        sample_rate: 250_000.0,
        channels: &[Channel::at(
            77_500.0,
            || ChannelParams::RadioClock(RadioClockParams::default()),
            LOG,
        )],
        exact_rate: false,
    },
    Entry {
        id: "navtex",
        name: "NAVTEX",
        description: "Coastal navigation warnings on 490 and 518 kHz.",
        explainer: "NAVTEX broadcasts navigational and meteorological warnings to ships as \
                    100 baud FSK teleprinter text. 518 kHz carries the international English \
                    service, 490 kHz the national-language one, and both are decoded here. \
                    Stations transmit on a schedule a few times a day, so leave it running; \
                    reception is far better after dark.",
        center_hz: 504_000.0,
        sample_rate: 250_000.0,
        channels: &[
            Channel::at(
                490_000.0,
                || ChannelParams::Navtex(NavtexParams::default()),
                LOG,
            ),
            Channel::at(
                518_000.0,
                || ChannelParams::Navtex(NavtexParams::default()),
                LOG,
            ),
        ],
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
        channels: &[Channel::at(
            1_575_420_000.0,
            || ChannelParams::Gnss(GnssParams::default()),
            LOG,
        )],
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
        channels: &[Channel::at(145_700_000.0, fm_voice, LISTEN)],
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
        channels: &[Channel::at(156_800_000.0, fm_voice, LISTEN)],
        exact_rate: false,
    },
    Entry {
        id: "pmr446",
        name: "PMR446 walkie-talkies",
        description: "The eight classic licence-free FM channels at 446 MHz.",
        explainer: "PMR446 handsets are sold without a licence across Europe, so this is the \
                    band with the most everyday traffic: markets, building sites, ski slopes \
                    and events. Channels 1–8 sit 12.5 kHz apart from 446.00625 MHz and all \
                    eight are open at once, each with automatic squelch so only a real \
                    transmission reaches the speaker. The readout shows the CTCSS or DCS code \
                    a handset sends, which is what its 'privacy code' setting really is. \
                    Channels 9–16 continue to 446.19375 MHz and are visible on the scope.",
        center_hz: 446_100_000.0,
        sample_rate: 1_024_000.0,
        channels: &[
            Channel::at(446_006_250.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_018_750.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_031_250.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_043_750.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_056_250.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_068_750.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_081_250.0, fm_voice, LISTEN_READ).squelched(8.0),
            Channel::at(446_093_750.0, fm_voice, LISTEN_READ).squelched(8.0),
        ],
        exact_rate: false,
    },
    Entry {
        id: "digital-voice",
        name: "Digital voice (70 cm)",
        description: "DMR, D-STAR, System Fusion and M17 decoded side by side.",
        explainer: "Amateur repeaters carry four incompatible digital voice systems that all \
                    look alike on a waterfall. Running one decoder per system on the same \
                    frequency answers which is in use: whichever locks is the mode, and the \
                    log names the talkgroup, callsign or reflector it carries while the \
                    speaker plays the decoded voice. Retune the receiver to a repeater output \
                    near you — 439 MHz is only the middle of the segment.",
        center_hz: 439_000_000.0,
        sample_rate: 1_024_000.0,
        channels: &[
            Channel::at(
                439_000_000.0,
                || ChannelParams::Dmr(DmrParams::default()),
                LISTEN_LOG,
            ),
            Channel::at(
                439_000_000.0,
                || ChannelParams::Dstar(DstarParams::default()),
                LISTEN_LOG,
            ),
            Channel::at(
                439_000_000.0,
                || ChannelParams::Ysf(YsfParams::default()),
                LISTEN_LOG,
            ),
            Channel::at(
                439_000_000.0,
                || ChannelParams::M17(M17Params::default()),
                LISTEN_LOG,
            ),
        ],
        exact_rate: false,
    },
    Entry {
        id: "ism-433",
        name: "ISM 433 MHz",
        description: "Remotes, sensors and doorbells, plus a classifier for the rest.",
        explainer: "433.05–434.79 MHz is licence-free for short-range devices, so it is full \
                    of weather sensors, car remotes, tyre-pressure monitors and doorbells \
                    sending a few bytes at a time. The sub-GHz decoder reports the raw pulse \
                    train of on-off keyed bursts. Alongside it, the identifier watches the \
                    same span and names the modulation of anything it sees, which is the \
                    quickest way to tell an unknown burst apart from noise.",
        center_hz: 433_920_000.0,
        sample_rate: 1_024_000.0,
        channels: &[
            Channel::at(
                433_920_000.0,
                || ChannelParams::Subghz(SubghzParams::default()),
                LOG,
            ),
            Channel::at(
                433_920_000.0,
                || ChannelParams::Ident(IdentParams::default()),
                LOG_READ,
            ),
        ],
        exact_rate: false,
    },
    Entry {
        id: "dab-check",
        name: "DAB ensemble check",
        description: "Confirm a DAB block is on air and measure its lock.",
        explainer: "A DAB ensemble is one 1.536 MHz wide OFDM block carrying a bundle of \
                    stations. This template locks to the block's transmission frame and \
                    reports lock, signal-to-noise and frequency error, which is what an \
                    antenna or filter change has to improve before anything else matters. It \
                    does not play DAB audio. 178.352 MHz is block 5C; the channel settings \
                    list the other blocks.",
        center_hz: 178_352_000.0,
        sample_rate: 2_048_000.0,
        channels: &[Channel::at(
            178_352_000.0,
            || ChannelParams::Dab(DabParams::default()),
            LOG,
        )],
        exact_rate: false,
    },
    Entry {
        id: "hf-digital",
        name: "HF digital (20 m)",
        description: "FT8, FT4 and WSPR decoded together on 20 metres.",
        explainer: "These three modes trade speed for sensitivity and decode signals well \
                    below the noise floor, which is why 20 m stays busy even when the band \
                    sounds empty. FT8 (14.074 MHz) and FT4 (14.080 MHz) are contacts on a \
                    15 and 7.5 second clock; WSPR (14.0956 MHz) is beacons on a two-minute \
                    one. Decoding depends on the computer clock being right — an error over \
                    about a second costs you every FT8 slot.",
        center_hz: 14_084_800.0,
        sample_rate: 250_000.0,
        channels: &[
            Channel::at(
                14_074_000.0,
                || ChannelParams::Ft8(WsjtParams::default()),
                LOG,
            ),
            Channel::at(
                14_080_000.0,
                || ChannelParams::Ft4(WsjtParams::default()),
                LOG,
            ),
            Channel::at(
                14_095_600.0,
                || ChannelParams::Wspr(WsprParams::default()),
                LOG,
            ),
        ],
        exact_rate: false,
    },
    Entry {
        id: "hf-keyboard",
        name: "HF keyboard modes (20 m)",
        description: "CW, RTTY and PSK31 read as text, with the audio to match.",
        explainer: "The oldest live modes on the band are still hand-tuned: each decoder is \
                    only a few hundred hertz wide, so drag its marker onto a trace on the \
                    scope until the text comes out clean. 14.100 MHz carries the international \
                    beacon project, whose eighteen stations transmit their callsign in Morse \
                    in turn, so it is the one frequency here that is busy on a schedule. The \
                    SSB channel demodulates the same segment so you hear what the decoders \
                    are reading.",
        center_hz: 14_085_000.0,
        sample_rate: 250_000.0,
        channels: &[
            Channel::at(
                14_070_150.0,
                || ChannelParams::Psk31(PskParams::default()),
                LOG_READ,
            ),
            Channel::at(
                14_083_000.0,
                || ChannelParams::Rtty(RttyParams::default()),
                LOG_READ,
            ),
            Channel::at(
                14_100_000.0,
                || ChannelParams::Morse(MorseParams::default()),
                LOG_READ,
            ),
            Channel::at(
                14_099_500.0,
                || ChannelParams::Ssb(SsbParams::default()),
                LISTEN,
            ),
        ],
        exact_rate: false,
    },
    Entry {
        id: "hf-sstv",
        name: "SSTV pictures (20 m)",
        description: "Slow-scan television on 14.230 MHz, picture and sound.",
        explainer: "SSTV sends a still picture as audio, one line at a time, in one to two \
                    minutes. The mode is announced by a VIS header at the start of a \
                    transmission and detected for you, so a picture starts building as soon \
                    as one is received. 14.230 MHz is the worldwide calling frequency; the \
                    SSB channel plays the warble the decoder is turning into the image.",
        center_hz: 14_230_000.0,
        sample_rate: 250_000.0,
        channels: &[
            Channel::at(
                14_230_000.0,
                || ChannelParams::Sstv(SstvParams::default()),
                WATCH,
            ),
            Channel::at(
                14_230_000.0,
                || ChannelParams::Ssb(SsbParams::default()),
                LISTEN,
            ),
        ],
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
                    .map(|channel| {
                        let params = (channel.params)();
                        ChannelSettings {
                            offset_hz: channel.freq_hz - entry.center_hz,
                            squelch_db: None,
                            squelch_auto_db: channel.squelch_auto_db,
                            audio: AudioProcessing::default_for(params.type_id()),
                            params,
                        }
                    })
                    .collect();
                let freqs = entry.channels.iter().map(|c| c.freq_hz);
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
                    patch: Some(patch(entry.channels)),
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
    use sdrmm_wire::{MAX_SQUELCH_AUTO_MARGIN_DB, MIN_SQUELCH_AUTO_MARGIN_DB, WorkspaceSnapshot};

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
    fn every_channel_gets_a_rate_its_decoder_can_use() {
        let descriptors = sdrmm_engine::Engine::new(None).channel_types();
        for template in all() {
            for channel in &template.channels {
                let type_id = channel.params.type_id();
                let descriptor = descriptors
                    .iter()
                    .find(|d| d.type_id == type_id)
                    .unwrap_or_else(|| panic!("{}: unknown channel {type_id}", template.id));
                assert!(
                    template.sample_rate >= descriptor.input_rate_hz,
                    "{}: {type_id} needs {} Hz, template runs at {} Hz",
                    template.id,
                    descriptor.input_rate_hz,
                    template.sample_rate
                );
                if let Some((_, high)) = descriptor.native_rate_range() {
                    assert!(
                        template.sample_rate <= high,
                        "{}: {type_id} reads raw samples up to {high} Hz",
                        template.id
                    );
                }
            }
        }
    }

    #[test]
    fn automatic_squelch_margins_are_inside_the_accepted_range() {
        for template in all() {
            for channel in &template.channels {
                let Some(margin) = channel.squelch_auto_db else {
                    continue;
                };
                assert!(
                    (MIN_SQUELCH_AUTO_MARGIN_DB..=MAX_SQUELCH_AUTO_MARGIN_DB).contains(&margin),
                    "{}: squelch margin {margin} dB",
                    template.id
                );
            }
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
    fn a_channel_reaches_every_sink_it_asks_for_and_no_other() {
        for entry in TEMPLATES {
            let patch = patch(entry.channels);
            for sink in ORDER {
                let wanted = entry.channels.iter().any(|c| c.sinks.contains(sink));
                let drawn = patch.nodes.iter().any(|node| node.id == sink.node());
                assert_eq!(drawn, wanted, "{}: {}", entry.id, sink.node());
            }
            for (index, channel) in entry.channels.iter().enumerate() {
                let id = format!("ch{index}");
                let wired: Vec<&str> = patch
                    .edges
                    .iter()
                    .filter(|edge| edge.from.node == id)
                    .map(|edge| edge.to.node.as_str())
                    .collect();
                let wanted: Vec<&str> = channel.sinks.iter().map(|sink| sink.node()).collect();
                assert_eq!(wired, wanted, "{}: {id}", entry.id);
            }
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
