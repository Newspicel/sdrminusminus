use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    EventOutputNode, GpsNode, MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS,
    MAX_POSITION_ENDPOINT_LEN, MIN_NMEA_BAUD, MIN_NMEA_UPDATE_INTERVAL_MS, PositionSource,
    channel::{ChannelDescriptor, ChannelParams},
    coherent::{DfParams, PassiveRadarParams},
    device::{Capabilities, DeviceInfo, Direction},
    filter::EventFilterNode,
    network::{MAX_NETWORK_ADDRESS_LEN, NetworkExportNode},
    propagation::PropagationNode,
    timemachine::TimeMachineNode,
    workspace::MAX_NAME_LEN,
};

pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 256;
pub const MAX_NODE_ID_LEN: usize = 64;
pub const MAX_COORD: f32 = 100_000.0;
pub const MAX_NODE_SIZE: f32 = 10_000.0;
pub const RACK_COLS: u16 = 12;
pub const RACK_ROWS: u16 = 8;
pub const MAX_STREAMS: u32 = 16;

#[must_use]
pub fn stream_port(base: &str, index: u32) -> String {
    if index == 0 {
        base.to_owned()
    } else {
        format!("{base}{}", index + 1)
    }
}

#[must_use]
pub fn port_stream(base: &str, name: &str) -> Option<u32> {
    if name == base {
        return Some(0);
    }
    let suffix = name.strip_prefix(base)?;
    let n: u32 = suffix.parse().ok()?;
    if !(2..=MAX_STREAMS).contains(&n) || suffix != n.to_string() {
        return None;
    }
    Some(n - 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    Iq,
    Baseband,
    Audio,
    Events,
    Video,
    Control,
    Position,
    Tx,
}

impl PortType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Iq => "iq",
            Self::Baseband => "baseband",
            Self::Audio => "audio",
            Self::Events => "events",
            Self::Video => "video",
            Self::Control => "control",
            Self::Position => "position",
            Self::Tx => "tx",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortCondition {
    #[default]
    Always,
    ChannelHasAudio,
    ChannelIsDecoder,
    ChannelHasVideo,
    ChannelNeedsPosition,
    DeviceIsTxCapable,
}

#[derive(Clone, Copy, Debug)]
pub enum PortBacking<'a> {
    Channel(&'a ChannelDescriptor),
    Device(&'a Capabilities),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortRepeat {
    #[default]
    Once,
    PerRxStream,
    PerTxStream,
}

impl PortRepeat {
    fn count(self, backing: Option<PortBacking<'_>>) -> u32 {
        match (self, backing) {
            (Self::Once, _) => 1,
            (Self::PerRxStream, Some(PortBacking::Device(caps))) => {
                caps.rx_streams.clamp(1, MAX_STREAMS)
            }
            (Self::PerTxStream, Some(PortBacking::Device(caps))) => {
                caps.tx_streams.clamp(1, MAX_STREAMS)
            }
            (Self::PerRxStream | Self::PerTxStream, _) => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PortSpec {
    pub name: String,
    pub port_type: PortType,
    pub direction: PortDirection,
    pub multi: bool,
    #[serde(default, skip_serializing_if = "is_always")]
    pub condition: PortCondition,
    #[serde(default, skip_serializing_if = "is_once")]
    pub repeat: PortRepeat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn is_always(condition: &PortCondition) -> bool {
    *condition == PortCondition::Always
}

fn is_once(repeat: &PortRepeat) -> bool {
    *repeat == PortRepeat::Once
}

impl PortSpec {
    fn new(
        port_type: PortType,
        direction: PortDirection,
        multi: bool,
        condition: PortCondition,
    ) -> Self {
        Self {
            name: port_type.as_str().to_owned(),
            port_type,
            direction,
            multi,
            condition,
            repeat: PortRepeat::Once,
            note: None,
        }
    }

    #[must_use]
    fn noted(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    #[must_use]
    fn repeated(mut self, repeat: PortRepeat) -> Self {
        self.repeat = repeat;
        self
    }

    fn named(
        name: &str,
        port_type: PortType,
        direction: PortDirection,
        multi: bool,
        condition: PortCondition,
    ) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::new(port_type, direction, multi, condition)
        }
    }

    #[must_use]
    pub fn applies_to(&self, backing: Option<PortBacking<'_>>) -> bool {
        match (self.condition, backing) {
            (PortCondition::Always, _) => true,
            (PortCondition::ChannelHasAudio, Some(PortBacking::Channel(channel))) => {
                channel.has_audio
            }
            (PortCondition::ChannelIsDecoder, Some(PortBacking::Channel(channel))) => {
                channel.decoder_kind.is_some()
            }
            (PortCondition::ChannelHasVideo, Some(PortBacking::Channel(channel))) => {
                channel.has_video
            }
            (PortCondition::ChannelNeedsPosition, Some(PortBacking::Channel(channel))) => {
                channel.needs_position
            }
            (PortCondition::DeviceIsTxCapable, Some(PortBacking::Device(device))) => {
                device.duplex.supports(Direction::Tx)
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    Source,
    Channel,
    Display,
    Feature,
    Sink,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceRef {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl DeviceRef {
    #[must_use]
    pub fn from_info(info: &DeviceInfo) -> Self {
        let variant = info.serial.as_ref().is_some_and(|serial| {
            info.key
                .strip_prefix(serial)
                .is_some_and(|suffix| suffix.starts_with('@'))
        });
        Self {
            backend: info.driver.clone(),
            serial: info.serial.clone(),
            key: (info.serial.is_none() || variant).then(|| info.key.clone()),
        }
    }

    #[must_use]
    pub fn matches(&self, info: &DeviceInfo) -> bool {
        if self.backend != info.driver {
            return false;
        }
        match (&self.serial, &info.serial) {
            (Some(want), Some(have)) => {
                want == have && self.key.as_ref().is_none_or(|key| *key == info.key)
            }
            (Some(_), None) => false,
            (None, _) => match &self.key {
                Some(key) => *key == info.key,
                None => true,
            },
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChannelNode {
    pub channel_type: String,
    #[serde(default)]
    pub record_calls: bool,
}

pub const DV_DECODER_KIND: &str = "dv";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DmrTrunkProtocol {
    #[default]
    Auto,
    CapacityPlus,
    HyteraXpt,
    TierThree,
}

pub const MAX_DMR_SEARCH_RANGES: usize = 8;
pub const MAX_DMR_SEARCH_CANDIDATES: usize = 512;
pub const MIN_DMR_SEARCH_STEP_HZ: u64 = 1_250;
pub const MAX_DMR_CHANNEL_MAP: usize = 512;
pub const MAX_DMR_PROBES: u8 = 8;
pub const DEFAULT_DMR_PROBES: u8 = 4;
pub const MAX_DMR_LOGICAL_CHANNEL: u16 = 4095;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrSearchRange {
    pub start_hz: u64,
    pub end_hz: u64,
    pub step_hz: u64,
}

impl DmrSearchRange {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.step_hz >= MIN_DMR_SEARCH_STEP_HZ
            && self.start_hz > 0
            && self.end_hz >= self.start_hz
            && self.candidates() <= MAX_DMR_SEARCH_CANDIDATES
    }

    #[must_use]
    pub fn candidates(&self) -> usize {
        if self.step_hz == 0 || self.end_hz < self.start_hz {
            return 0;
        }
        ((self.end_hz - self.start_hz) / self.step_hz) as usize + 1
    }

    pub fn frequencies(&self) -> impl Iterator<Item = u64> + '_ {
        (0..self.candidates() as u64).map(|step| self.start_hz + step * self.step_hz)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrChannelEntry {
    pub lcn: u16,
    pub freq_hz: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrDiscovery {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<DmrSearchRange>,
    #[serde(default)]
    pub max_probes: u8,
}

impl DmrDiscovery {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.ranges.len() <= MAX_DMR_SEARCH_RANGES
            && self.ranges.iter().all(DmrSearchRange::valid)
            && self.candidates() <= MAX_DMR_SEARCH_CANDIDATES
            && self.max_probes <= MAX_DMR_PROBES
    }

    #[must_use]
    pub fn candidates(&self) -> usize {
        self.ranges.iter().map(DmrSearchRange::candidates).sum()
    }

    #[must_use]
    pub fn probes(&self) -> u8 {
        if self.max_probes == 0 {
            DEFAULT_DMR_PROBES
        } else {
            self.max_probes.min(MAX_DMR_PROBES)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrTrunkNode {
    #[serde(default)]
    pub protocol: DmrTrunkProtocol,
    #[serde(default)]
    pub record_calls: bool,
    #[serde(default)]
    pub discovery: DmrDiscovery,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_map: Vec<DmrChannelEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_hz: Option<u64>,
    #[serde(default)]
    pub ignore_crc: bool,
}

impl DmrTrunkNode {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.discovery.valid()
            && self.channel_map.len() <= MAX_DMR_CHANNEL_MAP
            && self
                .channel_map
                .iter()
                .all(|entry| entry.lcn <= MAX_DMR_LOGICAL_CHANNEL && entry.freq_hz > 0)
            && self.control_hz.is_none_or(|hz| hz > 0)
    }
}

pub const DEFAULT_SIGNAL_MAP_OFFSET_HZ: i64 = 0;
pub const DEFAULT_SIGNAL_MAP_BANDWIDTH_HZ: u64 = 12_500;
pub const MAX_SIGNAL_MAP_OFFSET_HZ: i64 = 1_000_000_000_000;
pub const MAX_SIGNAL_MAP_BANDWIDTH_HZ: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct SignalMapNode {
    pub offset_hz: i64,
    pub bandwidth_hz: u64,
}

impl Default for SignalMapNode {
    fn default() -> Self {
        Self {
            offset_hz: DEFAULT_SIGNAL_MAP_OFFSET_HZ,
            bandwidth_hz: DEFAULT_SIGNAL_MAP_BANDWIDTH_HZ,
        }
    }
}

impl Default for DmrTrunkNode {
    fn default() -> Self {
        Self {
            protocol: DmrTrunkProtocol::Auto,
            record_calls: true,
            discovery: DmrDiscovery::default(),
            channel_map: Vec::new(),
            control_hz: None,
            ignore_crc: false,
        }
    }
}

/// What a hunt node remembers between sessions: where it was last pointed and how loud a click
/// track the operator wanted.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct HuntNode {
    #[serde(default)]
    pub settings: crate::hunt::HuntSettings,
    #[serde(default = "default_clicks")]
    pub clicks: bool,
}

const fn default_clicks() -> bool {
    true
}

impl Default for HuntNode {
    fn default() -> Self {
        Self {
            settings: crate::hunt::HuntSettings::default(),
            clicks: default_clicks(),
        }
    }
}

/// A direction finder bound to every lane of one coherent radio.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DfNode {
    #[serde(default)]
    pub settings: DfParams,
}

/// A passive radar: one lane watching the illuminator, one watching the sky.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PassiveRadarNode {
    #[serde(default)]
    pub settings: PassiveRadarParams,
}

pub const RADAR_REFERENCE_PORT: &str = "ref";
pub const RADAR_SURVEILLANCE_PORT: &str = "surv";
pub const DF_BEAM_PORT: &str = "beam";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum NodeBody {
    Device(DeviceNode),
    Gps(GpsNode),
    Channel(ChannelNode),
    Scope,
    Speaker,
    Map,
    SignalMap(SignalMapNode),
    Propagation(PropagationNode),
    Readout,
    DecoderLog,
    DmrTrunk(DmrTrunkNode),
    EventOutput(EventOutputNode),
    EventFilter(EventFilterNode),
    Video,
    Recorder,
    AudioRecorder,
    BasebandRecorder,
    TimeMachine(TimeMachineNode),
    NetworkExport(NetworkExportNode),
    Export,
    Scanner,
    Hunt(HuntNode),
    Df(DfNode),
    PassiveRadar(PassiveRadarNode),
}

impl NodeBody {
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Device(_) => "device",
            Self::Gps(_) => "gps",
            Self::Channel(_) => "channel",
            Self::Scope => "scope",
            Self::Speaker => "speaker",
            Self::Map => "map",
            Self::SignalMap(_) => "signal_map",
            Self::Propagation(_) => "propagation",
            Self::Readout => "readout",
            Self::DecoderLog => "decoder_log",
            Self::DmrTrunk(_) => "dmr_trunk",
            Self::EventOutput(_) => "event_output",
            Self::EventFilter(_) => "event_filter",
            Self::Video => "video",
            Self::Recorder => "recorder",
            Self::AudioRecorder => "audio_recorder",
            Self::BasebandRecorder => "baseband_recorder",
            Self::TimeMachine(_) => "time_machine",
            Self::NetworkExport(_) => "network_export",
            Self::Export => "export",
            Self::Scanner => "scanner",
            Self::Hunt(_) => "hunt",
            Self::Df(_) => "df",
            Self::PassiveRadar(_) => "passive_radar",
        }
    }

    #[must_use]
    pub const fn category(&self) -> NodeCategory {
        match self {
            Self::Device(_) | Self::Gps(_) => NodeCategory::Source,
            Self::Channel(_) | Self::Df(_) | Self::PassiveRadar(_) => NodeCategory::Channel,
            Self::Scope
            | Self::Map
            | Self::SignalMap(_)
            | Self::Propagation(_)
            | Self::Readout
            | Self::DecoderLog
            | Self::Video => NodeCategory::Display,
            Self::Scanner | Self::Hunt(_) | Self::DmrTrunk(_) | Self::EventFilter(_) => {
                NodeCategory::Feature
            }
            Self::Speaker
            | Self::Recorder
            | Self::AudioRecorder
            | Self::BasebandRecorder
            | Self::TimeMachine(_)
            | Self::NetworkExport(_)
            | Self::EventOutput(_)
            | Self::Export => NodeCategory::Sink,
        }
    }

    #[must_use]
    pub fn ports(&self) -> Vec<PortSpec> {
        let specs = ports_for(self.kind());
        match self {
            Self::Df(df) => spread_lanes(specs, df.settings.geometry.count()),
            _ => specs,
        }
    }

    #[must_use]
    pub fn ports_with(&self, backing: Option<PortBacking<'_>>) -> Vec<PortSpec> {
        let mut ports = Vec::new();
        for spec in self.ports() {
            if !spec.applies_to(backing) {
                continue;
            }
            if spec.repeat == PortRepeat::Once {
                ports.push(spec);
                continue;
            }
            for stream in 0..spec.repeat.count(backing) {
                let mut port = spec.clone();
                port.name = stream_port(&spec.name, stream);
                port.repeat = PortRepeat::Once;
                ports.push(port);
            }
        }
        ports
    }
}

/// Turns a per-lane port into one concrete port per element, because a direction finder's lane
/// order is the array's element order and a wire that could land on any of them would lose it.
fn spread_lanes(specs: Vec<PortSpec>, lanes: u32) -> Vec<PortSpec> {
    let lanes = lanes.clamp(1, MAX_STREAMS);
    let mut out = Vec::with_capacity(specs.len() + lanes as usize);
    for spec in specs {
        if spec.repeat != PortRepeat::PerRxStream {
            out.push(spec);
            continue;
        }
        for lane in 0..lanes {
            out.push(PortSpec {
                name: stream_port(&spec.name, lane),
                repeat: PortRepeat::Once,
                ..spec.clone()
            });
        }
    }
    out
}

fn ports_for(kind: &str) -> Vec<PortSpec> {
    use PortCondition::{
        Always, ChannelHasAudio, ChannelHasVideo, ChannelIsDecoder, ChannelNeedsPosition,
        DeviceIsTxCapable,
    };
    use PortDirection::{In, Out};
    use PortType::{Audio, Baseband, Control, Events, Iq, Position, Tx, Video};
    match kind {
        "device" => vec![
            PortSpec::new(Control, In, false, Always),
            PortSpec::new(Tx, In, false, DeviceIsTxCapable)
                .repeated(PortRepeat::PerTxStream)
                .noted(
                    "reserved: transmit is not built (), so nothing in this build emits \
                     a signal to key a radio with",
                ),
            PortSpec::new(Iq, Out, true, Always).repeated(PortRepeat::PerRxStream),
        ],
        "gps" => vec![PortSpec::new(Position, Out, true, Always)],
        "channel" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, ChannelNeedsPosition),
            PortSpec::new(Baseband, Out, true, Always),
            PortSpec::new(Audio, Out, true, ChannelHasAudio),
            PortSpec::new(Events, Out, true, ChannelIsDecoder),
            PortSpec::new(Video, Out, true, ChannelHasVideo),
        ],
        "scope" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Baseband, In, false, Always),
        ],
        "recorder" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "audio_recorder" => vec![PortSpec::new(Audio, In, true, Always)],
        "baseband_recorder" => vec![PortSpec::new(Baseband, In, true, Always)],
        "time_machine" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "network_export" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Baseband, In, false, Always),
        ],
        "scanner" | "hunt" => vec![PortSpec::new(Control, Out, false, Always)],
        "speaker" => vec![PortSpec::new(Audio, In, true, Always)],
        "video" => vec![PortSpec::new(Video, In, true, Always)],
        "map" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Position, In, true, Always),
        ],
        "signal_map" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "propagation" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Position, In, false, Always),
        ],
        "readout" | "decoder_log" | "export" => {
            vec![PortSpec::new(Events, In, true, Always)]
        }
        "dmr_trunk" => vec![
            PortSpec::new(Iq, In, false, Always)
                .noted("the radio the control channel sits on; the system runs its own decoders"),
            PortSpec::new(Events, Out, true, Always),
        ],
        "event_output" => vec![PortSpec::new(Events, In, true, Always)],
        "event_filter" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
        "df" => vec![
            PortSpec::new(Iq, In, false, Always).repeated(PortRepeat::PerRxStream),
            PortSpec::new(Position, In, false, Always),
            PortSpec::new(Events, Out, true, Always),
            PortSpec::named(DF_BEAM_PORT, Iq, Out, true, Always)
                .noted("the array summed towards the bearing it found, as one more radio lane"),
        ],
        "passive_radar" => vec![
            PortSpec::named(RADAR_REFERENCE_PORT, Iq, In, false, Always)
                .noted("the antenna pointed at the illuminator"),
            PortSpec::named(RADAR_SURVEILLANCE_PORT, Iq, In, false, Always)
                .noted("the antenna pointed at the sky the targets are in"),
            PortSpec::new(Position, In, false, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeTypeInfo {
    pub kind: String,
    pub name: String,
    pub category: NodeCategory,
    pub ports: Vec<PortSpec>,
    #[serde(default)]
    pub needs_channel_type: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchCatalog {
    pub nodes: Vec<NodeTypeInfo>,
}

impl PatchCatalog {
    #[must_use]
    pub fn build() -> Self {
        let entry = |body: &NodeBody, name: &str| NodeTypeInfo {
            kind: body.kind().to_owned(),
            name: name.to_owned(),
            category: body.category(),
            ports: body.ports(),
            needs_channel_type: matches!(body, NodeBody::Channel(_)),
        };
        Self {
            nodes: vec![
                entry(&NodeBody::Device(DeviceNode::default()), "Device"),
                entry(&NodeBody::Gps(GpsNode::default()), "GPS position"),
                entry(
                    &NodeBody::Channel(ChannelNode {
                        channel_type: String::new(),
                        record_calls: false,
                    }),
                    "Channel",
                ),
                entry(&NodeBody::Scope, "Scope"),
                entry(&NodeBody::Speaker, "Speaker"),
                entry(&NodeBody::Map, "Map"),
                entry(
                    &NodeBody::SignalMap(SignalMapNode::default()),
                    "Signal survey",
                ),
                entry(
                    &NodeBody::Propagation(PropagationNode::default()),
                    "Propagation map",
                ),
                entry(&NodeBody::Readout, "Readout"),
                entry(&NodeBody::DecoderLog, "Decoder log"),
                entry(
                    &NodeBody::DmrTrunk(DmrTrunkNode::default()),
                    "DMR trunk system",
                ),
                entry(
                    &NodeBody::EventFilter(EventFilterNode::default()),
                    "Event filter",
                ),
                entry(
                    &NodeBody::EventOutput(EventOutputNode::default()),
                    "Event output",
                ),
                entry(&NodeBody::Video, "Video"),
                entry(&NodeBody::Recorder, "Recorder"),
                entry(&NodeBody::AudioRecorder, "Audio recorder"),
                entry(&NodeBody::BasebandRecorder, "Baseband recorder"),
                entry(
                    &NodeBody::TimeMachine(TimeMachineNode::default()),
                    "Time machine",
                ),
                entry(
                    &NodeBody::NetworkExport(NetworkExportNode::default()),
                    "Network IQ",
                ),
                entry(&NodeBody::Export, "Export"),
                entry(&NodeBody::Scanner, "Scanner"),
                entry(&NodeBody::Hunt(HuntNode::default()), "Signal hunt"),
                entry(&NodeBody::Df(DfNode::default()), "Direction finder"),
                entry(
                    &NodeBody::PassiveRadar(PassiveRadarNode::default()),
                    "Passive radar",
                ),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PatchNode {
    pub id: String,
    #[serde(flatten)]
    pub body: NodeBody,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PortRef {
    pub node: String,
    pub port: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PatchEdge {
    pub from: PortRef,
    pub to: PortRef,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PatchGraph {
    pub nodes: Vec<PatchNode>,
    #[serde(default)]
    pub edges: Vec<PatchEdge>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RackCell {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RackSlot {
    pub node: String,
    #[serde(flatten)]
    pub cell: RackCell,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RackLayout {
    #[serde(default)]
    pub slots: Vec<RackSlot>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    TooManyNodes,
    TooManyEdges,
    NodeId(String),
    DuplicateNode(String),
    Label(String),
    Geometry(String),
    Backend(String),
    Gps(String),
    ChannelType(String),
    NodeSettings(String),
    UnknownNode(String),
    UnknownPort(PortRef),
    Direction(PortRef),
    TypeMismatch { from: PortType, to: PortType },
    DuplicateEdge(PortRef),
    PortOccupied(PortRef),
    MixedNetworkSource(String),
    SelfEdge(String),
    Cycle(String),
    RackCell(String),
    DuplicateRackSlot(String),
    RackOverlap(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyNodes => write!(f, "too many nodes (max {MAX_NODES})"),
            Self::TooManyEdges => write!(f, "too many edges (max {MAX_EDGES})"),
            Self::NodeId(id) => write!(f, "invalid node id {id:?}"),
            Self::DuplicateNode(id) => write!(f, "duplicate node id {id}"),
            Self::Label(id) => write!(f, "label of node {id} is longer than {MAX_NAME_LEN}"),
            Self::Geometry(id) => write!(f, "node {id} sits outside the canvas bounds"),
            Self::Backend(id) => write!(f, "node {id} names no backend"),
            Self::Gps(reason) => write!(f, "invalid GPS source: {reason}"),
            Self::ChannelType(ty) => write!(f, "unknown channel type {ty:?}"),
            Self::NodeSettings(id) => write!(f, "invalid settings for node {id}"),
            Self::UnknownNode(id) => write!(f, "edge names unknown node {id}"),
            Self::UnknownPort(port) => {
                write!(f, "node {} has no port {}", port.node, port.port)
            }
            Self::Direction(port) => write!(
                f,
                "port {} of node {} is on the wrong side of that wire",
                port.port, port.node
            ),
            Self::TypeMismatch { from, to } => write!(
                f,
                "a {} output cannot feed a {} input",
                from.as_str(),
                to.as_str()
            ),
            Self::DuplicateEdge(port) => {
                write!(f, "duplicate wire into {}.{}", port.node, port.port)
            }
            Self::PortOccupied(port) => write!(
                f,
                "{}.{} already has a wire and takes only one",
                port.node, port.port
            ),
            Self::MixedNetworkSource(id) => write!(
                f,
                "network sink {id} carries a radio's IQ or a channel's baseband, not both"
            ),
            Self::SelfEdge(id) => write!(f, "node {id} cannot wire to itself"),
            Self::Cycle(id) => write!(f, "node {id} sits on a loop of wires"),
            Self::RackCell(node) => write!(
                f,
                "rack slot for {node} is outside the {RACK_COLS}×{RACK_ROWS} grid"
            ),
            Self::DuplicateRackSlot(node) => write!(f, "node {node} is pinned twice"),
            Self::RackOverlap(node) => write!(f, "rack slot for {node} overlaps another"),
        }
    }
}

impl std::error::Error for PatchError {}

impl PatchGraph {
    #[must_use]
    pub fn node(&self, id: &str) -> Option<&PatchNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn device_nodes(&self) -> impl Iterator<Item = &PatchNode> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.body, NodeBody::Device(_)))
    }

    pub fn sources_of<'a>(&'a self, node: &'a str, port: &'a str) -> impl Iterator<Item = &'a str> {
        self.edges
            .iter()
            .filter(move |edge| edge.to.node == node && edge.to.port == port)
            .map(|edge| edge.from.node.as_str())
    }

    pub fn targets_of<'a>(&'a self, node: &'a str, port: &'a str) -> impl Iterator<Item = &'a str> {
        self.edges
            .iter()
            .filter(move |edge| edge.from.node == node && edge.from.port == port)
            .map(|edge| edge.to.node.as_str())
    }

    pub fn channels_of<'a>(
        &'a self,
        device_node: &'a str,
    ) -> impl Iterator<Item = (&'a PatchNode, u32)> {
        self.nodes.iter().filter_map(move |node| {
            if !matches!(node.body, NodeBody::Channel(_)) {
                return None;
            }
            let stream = self.edges.iter().find_map(|edge| {
                (edge.to.node == node.id && edge.to.port == "iq" && edge.from.node == device_node)
                    .then(|| port_stream("iq", &edge.from.port))
                    .flatten()
            })?;
            Some((node, stream))
        })
    }

    #[must_use]
    pub fn same_topology(&self, other: &Self) -> bool {
        self.edges == other.edges
            && self.nodes.len() == other.nodes.len()
            && self
                .nodes
                .iter()
                .zip(&other.nodes)
                .all(|(a, b)| a.id == b.id && a.body == b.body)
    }

    pub fn validate(&self) -> Result<(), PatchError> {
        self.check(None)
    }

    pub fn validate_against(&self, channels: &[ChannelDescriptor]) -> Result<(), PatchError> {
        self.check(Some(channels))
    }

    fn check(&self, channels: Option<&[ChannelDescriptor]>) -> Result<(), PatchError> {
        if self.nodes.len() > MAX_NODES {
            return Err(PatchError::TooManyNodes);
        }
        if self.edges.len() > MAX_EDGES {
            return Err(PatchError::TooManyEdges);
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            if node.id.is_empty() || node.id.len() > MAX_NODE_ID_LEN {
                return Err(PatchError::NodeId(node.id.clone()));
            }
            if seen.contains(&node.id.as_str()) {
                return Err(PatchError::DuplicateNode(node.id.clone()));
            }
            seen.push(&node.id);
            if node
                .label
                .as_ref()
                .is_some_and(|label| label.is_empty() || label.chars().count() > MAX_NAME_LEN)
            {
                return Err(PatchError::Label(node.id.clone()));
            }
            check_geometry(node)?;
            match &node.body {
                NodeBody::Device(device) => {
                    if device
                        .device
                        .as_ref()
                        .is_some_and(|r| r.backend.is_empty() || r.backend.len() > MAX_NAME_LEN)
                    {
                        return Err(PatchError::Backend(node.id.clone()));
                    }
                }
                NodeBody::Channel(channel) => {
                    if let Some(descriptors) = channels {
                        let descriptor = descriptors
                            .iter()
                            .find(|d| d.type_id == channel.channel_type)
                            .ok_or_else(|| PatchError::ChannelType(channel.channel_type.clone()))?;
                        if channel.record_calls
                            && descriptor.decoder_kind.as_deref() != Some(DV_DECODER_KIND)
                        {
                            return Err(PatchError::NodeSettings(node.id.clone()));
                        }
                    }
                }
                NodeBody::EventFilter(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::Gps(gps) => validate_gps_source(&gps.source)?,
                NodeBody::SignalMap(settings) => {
                    if settings.offset_hz.unsigned_abs() > MAX_SIGNAL_MAP_OFFSET_HZ as u64
                        || !(1..=MAX_SIGNAL_MAP_BANDWIDTH_HZ).contains(&settings.bandwidth_hz)
                    {
                        return Err(PatchError::NodeSettings(node.id.clone()));
                    }
                }
                NodeBody::Propagation(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::NetworkExport(export) => {
                    if export.settings.address.is_empty()
                        || export.settings.address.len() > MAX_NETWORK_ADDRESS_LEN
                        || !valid_host_port(&export.settings.address)
                    {
                        return Err(PatchError::NodeSettings(node.id.clone()));
                    }
                    if self.sources_of(&node.id, "iq").next().is_some()
                        && self.sources_of(&node.id, "baseband").next().is_some()
                    {
                        return Err(PatchError::MixedNetworkSource(node.id.clone()));
                    }
                }
                NodeBody::TimeMachine(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::DmrTrunk(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::EventOutput(settings) if !settings.target.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::Df(df) if !df.settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::PassiveRadar(radar) if !radar.settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                _ => {}
            }
        }
        self.check_edges(channels)?;
        self.check_acyclic()?;
        Ok(())
    }

    fn check_acyclic(&self) -> Result<(), PatchError> {
        let mut settled: Vec<&str> = Vec::with_capacity(self.nodes.len());
        let mut walking: Vec<&str> = Vec::new();
        for node in &self.nodes {
            self.walk(&node.id, &mut settled, &mut walking)?;
        }
        Ok(())
    }

    fn walk<'a>(
        &'a self,
        node: &'a str,
        settled: &mut Vec<&'a str>,
        walking: &mut Vec<&'a str>,
    ) -> Result<(), PatchError> {
        if settled.contains(&node) {
            return Ok(());
        }
        if walking.contains(&node) {
            return Err(PatchError::Cycle(node.to_owned()));
        }
        walking.push(node);
        for edge in self.edges.iter().filter(|edge| edge.from.node == node) {
            self.walk(&edge.to.node, settled, walking)?;
        }
        walking.pop();
        settled.push(node);
        Ok(())
    }

    fn check_edges(&self, channels: Option<&[ChannelDescriptor]>) -> Result<(), PatchError> {
        let mut landed: Vec<&PortRef> = Vec::with_capacity(self.edges.len());
        let mut left: Vec<&PortRef> = Vec::with_capacity(self.edges.len());
        for edge in &self.edges {
            if edge.from.node == edge.to.node {
                return Err(PatchError::SelfEdge(edge.from.node.clone()));
            }
            let out = self.port(&edge.from, PortDirection::Out, channels)?;
            let input = self.port(&edge.to, PortDirection::In, channels)?;
            if out.port_type != input.port_type {
                return Err(PatchError::TypeMismatch {
                    from: out.port_type,
                    to: input.port_type,
                });
            }
            if landed.contains(&&edge.to) && !input.multi {
                return Err(PatchError::PortOccupied(edge.to.clone()));
            }
            if left.contains(&&edge.from) && !out.multi {
                return Err(PatchError::PortOccupied(edge.from.clone()));
            }
            if self
                .edges
                .iter()
                .filter(|other| other.from == edge.from && other.to == edge.to)
                .count()
                > 1
            {
                return Err(PatchError::DuplicateEdge(edge.to.clone()));
            }
            landed.push(&edge.to);
            left.push(&edge.from);
        }
        Ok(())
    }

    fn port(
        &self,
        reference: &PortRef,
        direction: PortDirection,
        channels: Option<&[ChannelDescriptor]>,
    ) -> Result<PortSpec, PatchError> {
        let node = self
            .node(&reference.node)
            .ok_or_else(|| PatchError::UnknownNode(reference.node.clone()))?;
        let matches_name = |port: &PortSpec| {
            port.name == reference.port
                || (port.repeat != PortRepeat::Once
                    && port_stream(&port.name, &reference.port).is_some())
        };
        let ports = node.body.ports();
        let Some(spec) = ports
            .iter()
            .find(|port| port.direction == direction && matches_name(port))
            .cloned()
        else {
            return if ports.iter().any(matches_name) {
                Err(PatchError::Direction(reference.clone()))
            } else {
                Err(PatchError::UnknownPort(reference.clone()))
            };
        };
        if let (NodeBody::Channel(channel), Some(descriptors)) = (&node.body, channels) {
            let descriptor = descriptors
                .iter()
                .find(|d| d.type_id == channel.channel_type)
                .ok_or_else(|| PatchError::ChannelType(channel.channel_type.clone()))?;
            if !spec.applies_to(Some(PortBacking::Channel(descriptor))) {
                return Err(PatchError::UnknownPort(reference.clone()));
            }
        }
        Ok(spec)
    }
}

fn validate_gps_source(source: &PositionSource) -> Result<(), PatchError> {
    match source {
        PositionSource::Device => Ok(()),
        PositionSource::Gpsd { address } => {
            if address.is_empty() || address.len() > MAX_POSITION_ENDPOINT_LEN {
                Err(PatchError::Gps(
                    "gpsd address is empty or too long".to_owned(),
                ))
            } else if !valid_host_port(address) {
                Err(PatchError::Gps(
                    "gpsd address must be a host and non-zero port".to_owned(),
                ))
            } else {
                Ok(())
            }
        }
        PositionSource::Nmea {
            device,
            baud,
            update_interval_ms,
        } => {
            if device.is_empty() || device.len() > MAX_POSITION_ENDPOINT_LEN {
                return Err(PatchError::Gps(
                    "NMEA device is empty or too long".to_owned(),
                ));
            }
            if !(MIN_NMEA_BAUD..=MAX_NMEA_BAUD).contains(baud) {
                return Err(PatchError::Gps(format!(
                    "NMEA baud rate is outside {MIN_NMEA_BAUD}..={MAX_NMEA_BAUD}"
                )));
            }
            if !(MIN_NMEA_UPDATE_INTERVAL_MS..=MAX_NMEA_UPDATE_INTERVAL_MS)
                .contains(update_interval_ms)
            {
                return Err(PatchError::Gps(format!(
                    "NMEA update interval is outside {MIN_NMEA_UPDATE_INTERVAL_MS}..={MAX_NMEA_UPDATE_INTERVAL_MS} ms"
                )));
            }
            Ok(())
        }
    }
}

fn valid_host_port(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    if port.parse::<u16>().ok().is_none_or(|port| port == 0) {
        return false;
    }
    if let Some(ipv6) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        return ipv6.parse::<std::net::Ipv6Addr>().is_ok();
    }
    !host.is_empty()
        && !host.contains(':')
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn check_geometry(node: &PatchNode) -> Result<(), PatchError> {
    let bad_position = !node.position.x.is_finite()
        || !node.position.y.is_finite()
        || node.position.x.abs() > MAX_COORD
        || node.position.y.abs() > MAX_COORD;
    let bad_size = node.size.is_some_and(|size| {
        !size.w.is_finite()
            || !size.h.is_finite()
            || size.w <= 0.0
            || size.h <= 0.0
            || size.w > MAX_NODE_SIZE
            || size.h > MAX_NODE_SIZE
    });
    if bad_position || bad_size {
        return Err(PatchError::Geometry(node.id.clone()));
    }
    Ok(())
}

impl RackLayout {
    pub fn validate(&self, graph: &PatchGraph) -> Result<(), PatchError> {
        let mut seen: Vec<&str> = Vec::with_capacity(self.slots.len());
        for slot in &self.slots {
            if graph.node(&slot.node).is_none() {
                return Err(PatchError::UnknownNode(slot.node.clone()));
            }
            if seen.contains(&slot.node.as_str()) {
                return Err(PatchError::DuplicateRackSlot(slot.node.clone()));
            }
            seen.push(&slot.node);
            let cell = slot.cell;
            if cell.w == 0
                || cell.h == 0
                || u32::from(cell.x) + u32::from(cell.w) > u32::from(RACK_COLS)
                || u32::from(cell.y) + u32::from(cell.h) > u32::from(RACK_ROWS)
            {
                return Err(PatchError::RackCell(slot.node.clone()));
            }
        }
        for (i, slot) in self.slots.iter().enumerate() {
            if self.slots[..i]
                .iter()
                .any(|other| overlaps(other.cell, slot.cell))
            {
                return Err(PatchError::RackOverlap(slot.node.clone()));
            }
        }
        Ok(())
    }
}

fn overlaps(a: RackCell, b: RackCell) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

impl ChannelParams {
    #[must_use]
    pub fn default_for(type_id: &str) -> Option<Self> {
        serde_json::from_value(serde_json::json!({ "type": type_id, "settings": {} })).ok()
    }
}

#[cfg(test)]
mod tests;
