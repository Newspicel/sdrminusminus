use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    ChatOutputNode, GpsNode, MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS, MAX_POSITION_ENDPOINT_LEN,
    MIN_NMEA_BAUD, MIN_NMEA_UPDATE_INTERVAL_MS, PositionSource,
    channel::{ChannelDescriptor, ChannelParams},
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
    ChatOutput(ChatOutputNode),
    EventFilter(EventFilterNode),
    Video,
    Recorder,
    AudioRecorder,
    BasebandRecorder,
    TimeMachine(TimeMachineNode),
    NetworkExport(NetworkExportNode),
    Export,
    Scanner,
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
            Self::ChatOutput(_) => "chat_output",
            Self::EventFilter(_) => "event_filter",
            Self::Video => "video",
            Self::Recorder => "recorder",
            Self::AudioRecorder => "audio_recorder",
            Self::BasebandRecorder => "baseband_recorder",
            Self::TimeMachine(_) => "time_machine",
            Self::NetworkExport(_) => "network_export",
            Self::Export => "export",
            Self::Scanner => "scanner",
        }
    }

    #[must_use]
    pub const fn category(&self) -> NodeCategory {
        match self {
            Self::Device(_) | Self::Gps(_) => NodeCategory::Source,
            Self::Channel(_) => NodeCategory::Channel,
            Self::Scope
            | Self::Map
            | Self::SignalMap(_)
            | Self::Propagation(_)
            | Self::Readout
            | Self::DecoderLog
            | Self::Video => NodeCategory::Display,
            Self::Scanner | Self::DmrTrunk(_) | Self::EventFilter(_) => NodeCategory::Feature,
            Self::Speaker
            | Self::Recorder
            | Self::AudioRecorder
            | Self::BasebandRecorder
            | Self::TimeMachine(_)
            | Self::NetworkExport(_)
            | Self::ChatOutput(_)
            | Self::Export => NodeCategory::Sink,
        }
    }

    #[must_use]
    pub fn ports(&self) -> Vec<PortSpec> {
        ports_for(self.kind())
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
        "scanner" => vec![PortSpec::new(Control, Out, false, Always)],
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
        "chat_output" => vec![PortSpec::new(Events, In, true, Always)],
        "event_filter" => vec![
            PortSpec::new(Events, In, true, Always),
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
                    &NodeBody::ChatOutput(ChatOutputNode::default()),
                    "Discord / Matrix",
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
                NodeBody::ChatOutput(settings) if !settings.target.valid() => {
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
mod tests {
    use super::*;
    use crate::{
        channel::ChannelSettings,
        device::{DcArtifact, Duplex, StreamScope},
        filter::MAX_FILTER_IDS,
    };

    fn node(id: &str, body: NodeBody) -> PatchNode {
        PatchNode {
            id: id.to_owned(),
            body,
            position: Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }
    }

    fn channel(id: &str, ty: &str) -> PatchNode {
        node(
            id,
            NodeBody::Channel(ChannelNode {
                channel_type: ty.to_owned(),
                record_calls: false,
            }),
        )
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

    fn capabilities(duplex: Duplex, rx_streams: u32, tx_streams: u32) -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_ranges: Vec::new(),
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams,
            tx_streams,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Operator,
        }
    }

    fn descriptors() -> Vec<ChannelDescriptor> {
        vec![
            ChannelDescriptor {
                type_id: "nfm".to_owned(),
                name: "NFM".to_owned(),
                bandwidth_hz: 12_500.0,
                input_rate_hz: 48_000.0,
                ..ChannelDescriptor::default()
            },
            ChannelDescriptor {
                type_id: "adsb".to_owned(),
                name: "ADS-B".to_owned(),
                bandwidth_hz: 2_000_000.0,
                input_rate_hz: 2_000_000.0,
                has_audio: false,
                decoder_kind: Some("adsb".to_owned()),
                native_rate_max_hz: Some(4_000_000.0),
                ..ChannelDescriptor::default()
            },
            ChannelDescriptor {
                type_id: "dmr".to_owned(),
                name: "DMR".to_owned(),
                bandwidth_hz: 12_500.0,
                input_rate_hz: 48_000.0,
                decoder_kind: Some(DV_DECODER_KIND.to_owned()),
                ..ChannelDescriptor::default()
            },
        ]
    }

    fn recording_calls(id: &str, ty: &str, on: bool) -> PatchNode {
        node(
            id,
            NodeBody::Channel(ChannelNode {
                channel_type: ty.to_owned(),
                record_calls: on,
            }),
        )
    }

    #[test]
    fn a_voice_channel_may_record_its_calls() {
        let graph = PatchGraph {
            nodes: vec![recording_calls("dmr", "dmr", true)],
            edges: Vec::new(),
        };
        assert!(graph.validate_against(&descriptors()).is_ok());
    }

    #[test]
    fn a_channel_that_carries_no_voice_cannot_record_calls() {
        let graph = PatchGraph {
            nodes: vec![recording_calls("pager", "adsb", true)],
            edges: Vec::new(),
        };
        assert_eq!(
            graph.validate_against(&descriptors()),
            Err(PatchError::NodeSettings("pager".to_owned()))
        );
    }

    #[test]
    fn recording_nothing_is_always_allowed() {
        let graph = PatchGraph {
            nodes: vec![recording_calls("pager", "adsb", false)],
            edges: Vec::new(),
        };
        assert!(graph.validate_against(&descriptors()).is_ok());
    }

    #[test]
    fn an_event_filter_passes_events_through_and_bounds_its_lists() {
        let ports = ports_for("event_filter");
        assert_eq!(ports.len(), 2);
        assert!(
            ports
                .iter()
                .any(|p| p.direction == PortDirection::In && p.port_type == PortType::Events)
        );
        assert!(
            ports
                .iter()
                .any(|p| p.direction == PortDirection::Out && p.port_type == PortType::Events)
        );

        let graph = PatchGraph {
            nodes: vec![node(
                "filter",
                NodeBody::EventFilter(EventFilterNode {
                    kinds: vec!["call".to_owned()],
                    ..EventFilterNode::default()
                }),
            )],
            edges: Vec::new(),
        };
        assert!(graph.validate().is_ok());

        let mut invalid = graph;
        let NodeBody::EventFilter(settings) = &mut invalid.nodes[0].body else {
            panic!("event filter node");
        };
        settings.talkgroups = vec![0; MAX_FILTER_IDS + 1];
        assert_eq!(
            invalid.validate(),
            Err(PatchError::NodeSettings("filter".to_owned()))
        );
    }

    #[test]
    fn a_channel_can_reach_a_chat_output_through_a_filter() {
        let graph = PatchGraph {
            nodes: vec![
                recording_calls("dmr", "dmr", true),
                node("filter", NodeBody::EventFilter(EventFilterNode::default())),
                node(
                    "chat",
                    NodeBody::ChatOutput(ChatOutputNode {
                        target: crate::ChatOutputTarget::Discord {
                            webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
                        },
                    }),
                ),
            ],
            edges: vec![
                edge(("dmr", "events"), ("filter", "events")),
                edge(("filter", "events"), ("chat", "events")),
            ],
        };
        assert!(graph.validate_against(&descriptors()).is_ok());
    }

    fn workspace() -> PatchGraph {
        PatchGraph {
            nodes: vec![
                node("dev", NodeBody::Device(DeviceNode::default())),
                node("scope", NodeBody::Scope),
                channel("ch", "nfm"),
                node("spk", NodeBody::Speaker),
            ],
            edges: vec![
                edge(("dev", "iq"), ("scope", "iq")),
                edge(("dev", "iq"), ("ch", "iq")),
                edge(("ch", "audio"), ("spk", "audio")),
            ],
        }
    }

    #[test]
    fn topology_ignores_where_a_face_sits_and_what_it_is_called() {
        let graph = workspace();
        let mut moved = graph.clone();
        moved.nodes[0].position = Position { x: 900.0, y: -40.0 };
        moved.nodes[0].size = Some(Size { w: 500.0, h: 320.0 });
        moved.nodes[1].label = Some("Tower".to_owned());
        assert!(graph.same_topology(&moved));

        let mut rewired = graph.clone();
        rewired.edges.pop();
        assert!(!graph.same_topology(&rewired));

        let mut fewer = graph.clone();
        fewer.nodes.pop();
        assert!(!graph.same_topology(&fewer));

        let mut retyped = graph.clone();
        retyped.nodes[1].body = NodeBody::Channel(ChannelNode {
            channel_type: "am".to_owned(),
            record_calls: false,
        });
        assert!(!graph.same_topology(&retyped));

        let mut renamed = graph.clone();
        renamed.nodes[2].id = "spk2".to_owned();
        assert!(!graph.same_topology(&renamed));
    }

    #[test]
    fn node_body_is_adjacently_tagged_and_flattened_onto_the_node() {
        let json = serde_json::to_value(channel("ch", "nfm")).unwrap();
        assert_eq!(json["id"], "ch");
        assert_eq!(json["kind"], "channel");
        assert_eq!(json["data"]["channel_type"], "nfm");
        assert!(json.get("size").is_none());
        assert!(json.get("label").is_none());

        let scope = serde_json::to_value(node("s", NodeBody::Scope)).unwrap();
        assert_eq!(scope["kind"], "scope");
        assert!(scope.get("data").is_none());

        let back: PatchNode = serde_json::from_value(scope).unwrap();
        assert_eq!(back.body, NodeBody::Scope);
    }

    #[test]
    fn a_workspace_validates_structurally_and_against_the_registry() {
        let graph = workspace();
        graph.validate().expect("structurally valid");
        graph
            .validate_against(&descriptors())
            .expect("valid against the registry");
    }

    #[test]
    fn an_unknown_channel_type_is_refused_only_against_the_registry() {
        let mut graph = workspace();
        graph.nodes[2] = channel("ch", "wefax");
        graph.validate().expect("structure alone cannot know");
        assert_eq!(
            graph.validate_against(&descriptors()),
            Err(PatchError::ChannelType("wefax".to_owned()))
        );
    }

    #[test]
    fn a_dmr_trunk_system_takes_its_own_radio_before_a_control_frequency() {
        let wired = |control_hz| {
            PatchGraph {
                nodes: vec![
                    node("radio", NodeBody::Device(DeviceNode::default())),
                    node(
                        "system",
                        NodeBody::DmrTrunk(DmrTrunkNode {
                            control_hz,
                            ..DmrTrunkNode::default()
                        }),
                    ),
                ],
                edges: vec![edge(("radio", "iq"), ("system", "iq"))],
            }
            .validate()
        };

        wired(Some(451_000_000)).expect("a control channel anyone could tune");
        wired(None).expect("the wire comes first, the control channel is named after it");
        assert_eq!(
            wired(Some(0)),
            Err(PatchError::NodeSettings("system".to_owned())),
            "a control channel at nowhere was accepted"
        );
    }

    #[test]
    fn a_dmr_trunk_system_refuses_a_channel_search_it_cannot_run() {
        let searching = |ranges: Vec<DmrSearchRange>| {
            PatchGraph {
                nodes: vec![node(
                    "system",
                    NodeBody::DmrTrunk(DmrTrunkNode {
                        discovery: DmrDiscovery {
                            enabled: true,
                            ranges,
                            max_probes: 4,
                        },
                        ..DmrTrunkNode::default()
                    }),
                )],
                edges: Vec::new(),
            }
            .validate()
        };

        searching(vec![DmrSearchRange {
            start_hz: 451_000_000,
            end_hz: 451_500_000,
            step_hz: 12_500,
        }])
        .expect("a range the search can hold");
        assert_eq!(
            searching(vec![DmrSearchRange {
                start_hz: 450_000_000,
                end_hz: 460_000_000,
                step_hz: 12_500,
            }]),
            Err(PatchError::NodeSettings("system".to_owned())),
            "a range wider than the search can hold was accepted"
        );
        assert_eq!(
            searching(vec![DmrSearchRange {
                start_hz: 451_500_000,
                end_hz: 451_000_000,
                step_hz: 12_500,
            }]),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
        assert_eq!(
            searching(vec![DmrSearchRange {
                start_hz: 451_000_000,
                end_hz: 451_100_000,
                step_hz: 500,
            }]),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
    }

    #[test]
    fn a_dmr_trunk_system_refuses_a_channel_plan_it_cannot_use() {
        let planned = |channel_map: Vec<DmrChannelEntry>| {
            PatchGraph {
                nodes: vec![node(
                    "system",
                    NodeBody::DmrTrunk(DmrTrunkNode {
                        channel_map,
                        ..DmrTrunkNode::default()
                    }),
                )],
                edges: Vec::new(),
            }
            .validate()
        };

        planned(vec![DmrChannelEntry {
            lcn: 17,
            freq_hz: 451_012_500,
        }])
        .expect("a channel anyone could tune");
        assert_eq!(
            planned(vec![DmrChannelEntry {
                lcn: 17,
                freq_hz: 0,
            }]),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
        assert_eq!(
            planned(vec![DmrChannelEntry {
                lcn: MAX_DMR_LOGICAL_CHANNEL + 1,
                freq_hz: 451_012_500,
            }]),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
    }

    #[test]
    fn a_chat_output_accepts_decoder_and_completed_call_events() {
        let output = node(
            "chat",
            NodeBody::ChatOutput(ChatOutputNode {
                target: crate::ChatOutputTarget::Discord {
                    webhook_url: "https://discord.com/api/webhooks/1/token".to_owned(),
                },
            }),
        );
        let calls = PatchGraph {
            nodes: vec![
                node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
                output.clone(),
            ],
            edges: vec![edge(("system", "events"), ("chat", "events"))],
        };
        calls.validate().expect("completed calls");

        let decoded = PatchGraph {
            nodes: vec![channel("carrier", "dmr"), output],
            edges: vec![edge(("carrier", "events"), ("chat", "events"))],
        };
        decoded.validate().expect("decoded events");
    }

    #[test]
    fn a_dmr_trunk_records_calls_by_default_and_can_be_told_not_to() {
        assert!(DmrTrunkNode::default().record_calls);
        let graph = PatchGraph {
            nodes: vec![node(
                "system",
                NodeBody::DmrTrunk(DmrTrunkNode {
                    record_calls: false,
                    ..DmrTrunkNode::default()
                }),
            )],
            edges: Vec::new(),
        };
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn network_export_requires_a_bounded_host_and_nonzero_port() {
        let mut graph = PatchGraph {
            nodes: vec![node(
                "net",
                NodeBody::NetworkExport(NetworkExportNode::default()),
            )],
            edges: Vec::new(),
        };
        graph.validate().expect("default destination");

        let NodeBody::NetworkExport(export) = &mut graph.nodes[0].body else {
            panic!("network export node");
        };
        export.settings.address = "localhost:0".to_owned();
        assert_eq!(
            graph.validate(),
            Err(PatchError::NodeSettings("net".to_owned()))
        );

        let NodeBody::NetworkExport(export) = &mut graph.nodes[0].body else {
            panic!("network export node");
        };
        export.settings.address = "[::1]:7355".to_owned();
        graph.validate().expect("IPv6 destination");
    }

    #[test]
    fn a_network_sink_takes_a_radio_or_a_channel_but_not_both() {
        let graph = |edges: Vec<PatchEdge>| PatchGraph {
            nodes: vec![
                node("dev", NodeBody::Device(DeviceNode::default())),
                channel("ch", "nfm"),
                node("net", NodeBody::NetworkExport(NetworkExportNode::default())),
            ],
            edges,
        };

        graph(vec![edge(("dev", "iq"), ("net", "iq"))])
            .validate()
            .expect("a radio's IQ");
        graph(vec![edge(("ch", "baseband"), ("net", "baseband"))])
            .validate()
            .expect("a channel's baseband");
        assert_eq!(
            graph(vec![
                edge(("dev", "iq"), ("net", "iq")),
                edge(("ch", "baseband"), ("net", "baseband")),
            ])
            .validate(),
            Err(PatchError::MixedNetworkSource("net".to_owned()))
        );
    }

    #[test]
    fn a_baseband_recorder_takes_every_channel_wired_into_it() {
        let graph = PatchGraph {
            nodes: vec![
                channel("a", "nfm"),
                channel("b", "nfm"),
                node("files", NodeBody::BasebandRecorder),
            ],
            edges: vec![
                edge(("a", "baseband"), ("files", "baseband")),
                edge(("b", "baseband"), ("files", "baseband")),
            ],
        };
        graph
            .validate_against(&descriptors())
            .expect("a baseband recorder fans in");
    }

    #[test]
    fn a_time_machine_holds_a_window_the_engine_can_afford() {
        let graph = |seconds: u32| PatchGraph {
            nodes: vec![
                node("dev", NodeBody::Device(DeviceNode::default())),
                node(
                    "history",
                    NodeBody::TimeMachine(crate::TimeMachineNode {
                        history_seconds: seconds,
                    }),
                ),
            ],
            edges: vec![edge(("dev", "iq"), ("history", "iq"))],
        };
        graph(crate::DEFAULT_TIME_MACHINE_SECONDS)
            .validate()
            .expect("the default window");
        for refused in [0, crate::MAX_TIME_MACHINE_SECONDS + 1] {
            assert_eq!(
                graph(refused).validate(),
                Err(PatchError::NodeSettings("history".to_owned())),
                "{refused} s"
            );
        }
    }

    #[test]
    fn a_dmr_trunk_system_takes_a_radio_and_hands_out_events() {
        let graph = PatchGraph {
            nodes: vec![
                node("radio", NodeBody::Device(DeviceNode::default())),
                node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
                node("log", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge(("radio", "iq"), ("system", "iq")),
                edge(("system", "events"), ("log", "events")),
            ],
        };
        graph.validate().expect("matching wire names");

        let carrier = PatchGraph {
            nodes: vec![
                channel("carrier", "dmr"),
                node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
            ],
            edges: vec![edge(("carrier", "events"), ("system", "events"))],
        };
        assert_eq!(
            carrier.validate(),
            Err(PatchError::Direction(PortRef {
                node: "system".to_owned(),
                port: "events".to_owned()
            })),
            "a system that decodes for itself took a decoder's events"
        );
    }

    #[test]
    fn a_conditional_port_is_refused_on_a_type_that_lacks_it() {
        let mut graph = workspace();
        graph.nodes[2] = channel("ch", "adsb");
        let err = graph.validate_against(&descriptors()).unwrap_err();
        assert_eq!(
            err,
            PatchError::UnknownPort(PortRef {
                node: "ch".to_owned(),
                port: "audio".to_owned()
            })
        );
    }

    #[test]
    fn edges_must_name_real_ports_of_matching_type_and_direction() {
        let mut wrong_type = workspace();
        wrong_type.edges.push(edge(("dev", "iq"), ("spk", "audio")));
        assert_eq!(
            wrong_type.validate(),
            Err(PatchError::TypeMismatch {
                from: PortType::Iq,
                to: PortType::Audio
            })
        );

        let mut backwards = workspace();
        backwards.edges = vec![edge(("scope", "iq"), ("dev", "iq"))];
        assert_eq!(
            backwards.validate(),
            Err(PatchError::Direction(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut unknown = workspace();
        unknown.edges = vec![edge(("dev", "iq"), ("scope", "tap"))];
        assert_eq!(
            unknown.validate(),
            Err(PatchError::UnknownPort(PortRef {
                node: "scope".to_owned(),
                port: "tap".to_owned()
            }))
        );

        let mut missing = workspace();
        missing.edges = vec![edge(("ghost", "iq"), ("scope", "iq"))];
        assert_eq!(
            missing.validate(),
            Err(PatchError::UnknownNode("ghost".to_owned()))
        );
    }

    #[test]
    fn a_single_input_takes_one_wire_and_an_output_fans_out() {
        let mut two_devices = workspace();
        two_devices
            .nodes
            .push(node("dev2", NodeBody::Device(DeviceNode::default())));
        two_devices.edges.push(edge(("dev2", "iq"), ("ch", "iq")));
        assert_eq!(
            two_devices.validate(),
            Err(PatchError::PortOccupied(PortRef {
                node: "ch".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut fanned = workspace();
        fanned.nodes.push(node("rec", NodeBody::Recorder));
        fanned.edges.push(edge(("dev", "iq"), ("rec", "iq")));
        fanned.validate().expect("iq fans out");
    }

    #[test]
    fn duplicate_wires_and_self_wires_are_refused() {
        let mut duplicate = workspace();
        duplicate.edges.push(edge(("dev", "iq"), ("scope", "iq")));
        assert_eq!(
            duplicate.validate(),
            Err(PatchError::DuplicateEdge(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut loop_back = workspace();
        loop_back.edges = vec![edge(("ch", "audio"), ("ch", "iq"))];
        assert_eq!(
            loop_back.validate(),
            Err(PatchError::SelfEdge("ch".to_owned()))
        );
    }

    #[test]
    fn the_only_type_level_cycle_is_the_guarded_event_transform() {
        let catalog = PatchCatalog::build();
        let reaches = |from: &NodeTypeInfo, to: &NodeTypeInfo| {
            from.ports
                .iter()
                .filter(|port| port.direction == PortDirection::Out)
                .any(|out| {
                    to.ports.iter().any(|input| {
                        input.direction == PortDirection::In && input.port_type == out.port_type
                    })
                })
        };
        let count = catalog.nodes.len();
        let mut reachable = vec![vec![false; count]; count];
        for (a, row) in reachable.iter_mut().enumerate() {
            for (b, cell) in row.iter_mut().enumerate() {
                *cell = reaches(&catalog.nodes[a], &catalog.nodes[b]);
            }
        }
        for through in 0..count {
            for from in 0..count {
                for to in 0..count {
                    reachable[from][to] |= reachable[from][through] && reachable[through][to];
                }
            }
        }
        let cycle: Vec<&str> = (0..count)
            .filter(|&kind| reachable[kind][kind])
            .map(|kind| catalog.nodes[kind].kind.as_str())
            .collect();
        assert_eq!(cycle, vec!["event_filter"]);
    }

    #[test]
    fn every_node_the_palette_offers_round_trips_and_validates_on_its_own() {
        for entry in PatchCatalog::build().nodes {
            let body = default_body(&entry.kind);
            let json = serde_json::to_string(&body).expect("serialize the body");
            let back: NodeBody = serde_json::from_str(&json).unwrap_or_else(|error| {
                panic!("{} does not survive a round trip: {error}", entry.kind)
            });
            assert_eq!(back.kind(), entry.kind);

            let mut node = node("solo", back);
            if let NodeBody::Channel(channel) = &mut node.body {
                channel.channel_type = "nfm".to_owned();
            }
            let graph = PatchGraph {
                nodes: vec![node],
                edges: Vec::new(),
            };
            graph
                .validate()
                .unwrap_or_else(|error| panic!("a fresh {} is invalid: {error}", entry.kind));
        }
    }

    fn default_body(kind: &str) -> NodeBody {
        match kind {
            "device" => NodeBody::Device(DeviceNode::default()),
            "gps" => NodeBody::Gps(GpsNode::default()),
            "channel" => NodeBody::Channel(ChannelNode {
                channel_type: "nfm".to_owned(),
                record_calls: false,
            }),
            "scope" => NodeBody::Scope,
            "speaker" => NodeBody::Speaker,
            "map" => NodeBody::Map,
            "signal_map" => NodeBody::SignalMap(SignalMapNode::default()),
            "propagation" => NodeBody::Propagation(PropagationNode::default()),
            "readout" => NodeBody::Readout,
            "decoder_log" => NodeBody::DecoderLog,
            "dmr_trunk" => NodeBody::DmrTrunk(DmrTrunkNode::default()),
            "event_filter" => NodeBody::EventFilter(EventFilterNode::default()),
            "chat_output" => NodeBody::ChatOutput(ChatOutputNode::default()),
            "video" => NodeBody::Video,
            "recorder" => NodeBody::Recorder,
            "audio_recorder" => NodeBody::AudioRecorder,
            "baseband_recorder" => NodeBody::BasebandRecorder,
            "time_machine" => NodeBody::TimeMachine(TimeMachineNode::default()),
            "network_export" => NodeBody::NetworkExport(NetworkExportNode::default()),
            "export" => NodeBody::Export,
            "scanner" => NodeBody::Scanner,
            other => panic!("the palette offers {other}, which this test does not build"),
        }
    }

    #[test]
    fn the_json_the_editor_sends_for_a_fresh_filter_parses() {
        let sent = r#"{
            "id": "filter",
            "position": { "x": 0.0, "y": 0.0 },
            "kind": "event_filter",
            "data": {
                "kinds": [],
                "stations": [],
                "talkgroups": [],
                "radios": [],
                "min_duration_ms": 0
            }
        }"#;

        let parsed: PatchNode = serde_json::from_str(sent).expect("a fresh filter parses");

        assert!(
            matches!(parsed.body, NodeBody::EventFilter(ref f) if f == &EventFilterNode::default())
        );
    }

    #[test]
    fn a_loop_of_wires_is_refused_however_long_it_is() {
        let graph = PatchGraph {
            nodes: vec![
                node("a", NodeBody::EventFilter(EventFilterNode::default())),
                node("b", NodeBody::EventFilter(EventFilterNode::default())),
                node("c", NodeBody::EventFilter(EventFilterNode::default())),
            ],
            edges: vec![
                edge(("a", "events"), ("b", "events")),
                edge(("b", "events"), ("c", "events")),
                edge(("c", "events"), ("a", "events")),
            ],
        };
        assert!(matches!(graph.validate(), Err(PatchError::Cycle(_))));
    }

    #[test]
    fn a_chain_of_filters_is_not_a_loop() {
        let graph = PatchGraph {
            nodes: vec![
                node("a", NodeBody::EventFilter(EventFilterNode::default())),
                node("b", NodeBody::EventFilter(EventFilterNode::default())),
            ],
            edges: vec![edge(("a", "events"), ("b", "events"))],
        };
        assert!(graph.validate().is_ok());
    }

    #[test]
    fn the_reserved_transmit_input_can_take_no_wire() {
        let catalog = PatchCatalog::build();
        let ports = || catalog.nodes.iter().flat_map(|entry| &entry.ports);
        let reserved = ports()
            .find(|port| port.port_type == PortType::Tx)
            .expect("the device node reserves a transmit input");
        assert_eq!(reserved.direction, PortDirection::In);
        assert!(
            reserved.note.is_some(),
            "a port that refuses everything says why"
        );
        assert!(
            !ports()
                .any(|port| port.port_type == PortType::Tx && port.direction == PortDirection::Out),
            "nothing may emit transmit baseband before  gate exists"
        );

        let mut retransmit = workspace();
        retransmit
            .nodes
            .push(node("dev2", NodeBody::Device(DeviceNode::default())));
        retransmit.edges.push(edge(("dev", "iq"), ("dev2", "tx")));
        assert_eq!(
            retransmit.validate(),
            Err(PatchError::TypeMismatch {
                from: PortType::Iq,
                to: PortType::Tx
            })
        );
    }

    #[test]
    fn only_a_radio_that_can_transmit_shows_a_transmit_input() {
        let transmit = ports_for("device")
            .into_iter()
            .find(|port| port.port_type == PortType::Tx)
            .expect("the device kind can have a transmit input");

        let named =
            |port: &PortSpec, caps: &Capabilities| port.applies_to(Some(PortBacking::Device(caps)));
        assert!(
            !named(&transmit, &capabilities(Duplex::RxOnly, 1, 0)),
            "a receiver"
        );
        assert!(
            named(&transmit, &capabilities(Duplex::Half, 1, 1)),
            "a transceiver"
        );
        assert!(!transmit.applies_to(None), "no radio bound");

        for port in ports_for("device")
            .into_iter()
            .filter(|port| port.port_type != PortType::Tx)
        {
            assert!(port.applies_to(None), "{} is not conditional", port.name);
        }
    }

    #[test]
    fn a_scanner_owns_the_one_radio_its_wire_runs_into() {
        let mut driven = workspace();
        driven.nodes.push(node("scan", NodeBody::Scanner));
        driven
            .edges
            .push(edge(("scan", "control"), ("dev", "control")));
        driven.validate().expect("a scanner drives a radio");

        let mut backwards = driven.clone();
        backwards.edges = vec![edge(("dev", "control"), ("scan", "control"))];
        assert_eq!(
            backwards.validate(),
            Err(PatchError::Direction(PortRef {
                node: "dev".to_owned(),
                port: "control".to_owned()
            }))
        );

        let mut two_radios = driven.clone();
        two_radios
            .nodes
            .push(node("dev2", NodeBody::Device(DeviceNode::default())));
        two_radios
            .edges
            .push(edge(("scan", "control"), ("dev2", "control")));
        assert_eq!(
            two_radios.validate(),
            Err(PatchError::PortOccupied(PortRef {
                node: "scan".to_owned(),
                port: "control".to_owned()
            }))
        );

        let mut two_scanners = driven.clone();
        two_scanners.nodes.push(node("scan2", NodeBody::Scanner));
        two_scanners
            .edges
            .push(edge(("scan2", "control"), ("dev", "control")));
        assert_eq!(
            two_scanners.validate(),
            Err(PatchError::PortOccupied(PortRef {
                node: "dev".to_owned(),
                port: "control".to_owned()
            }))
        );
    }

    #[test]
    fn ids_geometry_and_labels_are_bounded() {
        let mut duplicate = workspace();
        duplicate.nodes.push(node("dev", NodeBody::Scope));
        assert_eq!(
            duplicate.validate(),
            Err(PatchError::DuplicateNode("dev".to_owned()))
        );

        let mut empty_id = workspace();
        empty_id.nodes[1].id = String::new();
        assert!(matches!(empty_id.validate(), Err(PatchError::NodeId(_))));

        let mut far_away = workspace();
        far_away.nodes[1].position.x = f32::INFINITY;
        assert_eq!(
            far_away.validate(),
            Err(PatchError::Geometry("scope".to_owned()))
        );

        let mut flat = workspace();
        flat.nodes[1].size = Some(Size { w: 0.0, h: 100.0 });
        assert_eq!(
            flat.validate(),
            Err(PatchError::Geometry("scope".to_owned()))
        );

        let mut long_label = workspace();
        long_label.nodes[1].label = Some("x".repeat(MAX_NAME_LEN + 1));
        assert_eq!(
            long_label.validate(),
            Err(PatchError::Label("scope".to_owned()))
        );
    }

    #[test]
    fn device_refs_match_by_serial_then_key_then_singleton() {
        let hardware = DeviceInfo {
            driver: "rtlsdr".to_owned(),
            key: "0".to_owned(),
            label: "RTL-SDR".to_owned(),
            serial: Some("00000001".to_owned()),
            profile: None,
        };
        let file = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "file:/rec/capture".to_owned(),
            label: "capture".to_owned(),
            serial: None,
            profile: None,
        };
        let siggen = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "siggen".to_owned(),
            label: "Signal Generator".to_owned(),
            serial: None,
            profile: None,
        };

        let by_serial = DeviceRef::from_info(&hardware);
        assert_eq!(by_serial.key, None, "a serial makes the key redundant");
        assert!(by_serial.matches(&hardware));
        assert!(!by_serial.matches(&DeviceInfo {
            key: "1".to_owned(),
            serial: Some("00000002".to_owned()),
            ..hardware.clone()
        }));
        assert!(by_serial.matches(&DeviceInfo {
            key: "3".to_owned(),
            ..hardware.clone()
        }));

        let duo = DeviceInfo {
            driver: "soapy".to_owned(),
            key: "123456@DT".to_owned(),
            label: "Dual Tuner".to_owned(),
            serial: Some("123456".to_owned()),
            profile: None,
        };
        let by_variant = DeviceRef::from_info(&duo);
        assert_eq!(by_variant.key.as_deref(), Some("123456@DT"));
        assert!(by_variant.matches(&duo));
        assert!(!by_variant.matches(&DeviceInfo {
            key: "123456@ST".to_owned(),
            ..duo
        }));

        let by_key = DeviceRef::from_info(&file);
        assert_eq!(by_key.key.as_deref(), Some("file:/rec/capture"));
        assert!(by_key.matches(&file));
        assert!(
            !by_key.matches(&siggen),
            "two serial-less devices of one backend stay distinct"
        );

        let singleton = DeviceRef {
            backend: "hackrf".to_owned(),
            serial: None,
            key: None,
        };
        assert!(singleton.matches(&DeviceInfo {
            driver: "hackrf".to_owned(),
            key: "0".to_owned(),
            label: "HackRF One".to_owned(),
            serial: None,
            profile: None,
        }));
        assert!(!singleton.matches(&hardware));
    }

    #[test]
    fn channels_of_walks_the_wires_in_stored_order() {
        let mut graph = workspace();
        graph.nodes.push(channel("ch2", "adsb"));
        graph.edges.push(edge(("dev", "iq"), ("ch2", "iq")));
        let bound: Vec<(&str, u32)> = graph
            .channels_of("dev")
            .map(|(n, stream)| (n.id.as_str(), stream))
            .collect();
        assert_eq!(bound, vec![("ch", 0), ("ch2", 0)]);
        assert_eq!(graph.channels_of("scope").count(), 0);
    }

    #[test]
    fn channels_of_reports_the_stream_each_channel_taps() {
        let mut graph = workspace();
        graph.nodes.push(channel("ch2", "adsb"));
        graph.edges.push(edge(("dev", "iq3"), ("ch2", "iq")));
        graph.validate().expect("a stream-3 wire is valid");
        let bound: Vec<(&str, u32)> = graph
            .channels_of("dev")
            .map(|(n, stream)| (n.id.as_str(), stream))
            .collect();
        assert_eq!(bound, vec![("ch", 0), ("ch2", 2)]);
    }

    #[test]
    fn stream_ports_number_from_two_and_round_trip() {
        for base in ["iq", "tx"] {
            assert_eq!(stream_port(base, 0), base, "stream 0 keeps the bare name");
            assert_eq!(stream_port(base, 1), format!("{base}2"));
            for index in 0..MAX_STREAMS {
                assert_eq!(port_stream(base, &stream_port(base, index)), Some(index));
            }
        }
        assert_eq!(port_stream("iq", "iq3"), Some(2));
        assert_eq!(
            port_stream("iq", "iq16"),
            Some(15),
            "the last storable name"
        );
    }

    #[test]
    fn a_name_outside_the_family_addresses_no_stream() {
        assert_eq!(port_stream("iq", "iq1"), None);
        assert_eq!(port_stream("iq", "iq0"), None);
        assert_eq!(port_stream("iq", "iqx"), None);
        assert_eq!(port_stream("iq", "iq17"), None, "over MAX_STREAMS");
        assert_eq!(port_stream("iq", "tx2"), None);
        assert_eq!(port_stream("iq", "iq02"), None);
        assert_eq!(port_stream("iq", "iq+2"), None);
        assert_eq!(port_stream("iq", "iq2 "), None);
        assert_eq!(port_stream("iq", ""), None);
    }

    #[test]
    fn ports_with_expands_a_repeating_port_per_stream() {
        let device = NodeBody::Device(DeviceNode::default());
        let names = |caps: &Capabilities| {
            device
                .ports_with(Some(PortBacking::Device(caps)))
                .into_iter()
                .map(|port| port.name)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            names(&capabilities(Duplex::RxOnly, 4, 0)),
            vec!["control", "iq", "iq2", "iq3", "iq4"]
        );
        assert_eq!(
            names(&capabilities(Duplex::Full, 2, 2)),
            vec!["control", "tx", "tx2", "iq", "iq2"]
        );
        assert_eq!(
            names(&capabilities(Duplex::RxOnly, 1, 0)),
            vec!["control", "iq"],
            "a single-stream radio keeps the table it always had"
        );

        let expanded =
            device.ports_with(Some(PortBacking::Device(&capabilities(Duplex::Full, 2, 2))));
        for port in &expanded {
            assert_eq!(port.repeat, PortRepeat::Once, "{}", port.name);
        }
        let iq2 = expanded.iter().find(|p| p.name == "iq2").unwrap();
        assert_eq!(iq2.port_type, PortType::Iq);
        assert_eq!(iq2.direction, PortDirection::Out);
        assert!(iq2.multi, "each stream fans out on its own");
        let tx2 = expanded.iter().find(|p| p.name == "tx2").unwrap();
        assert_eq!(tx2.condition, PortCondition::DeviceIsTxCapable);
        assert!(tx2.note.is_some(), "every reserved transmit port says why");
    }

    #[test]
    fn an_unbacked_node_expands_to_stream_zero_only() {
        let device = NodeBody::Device(DeviceNode::default());
        let names: Vec<String> = device
            .ports_with(None)
            .into_iter()
            .map(|port| port.name)
            .collect();
        assert_eq!(names, vec!["control", "iq"]);

        let body = NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
            record_calls: false,
        });
        let nfm = &descriptors()[0];
        let names: Vec<String> = body
            .ports_with(Some(PortBacking::Channel(nfm)))
            .into_iter()
            .map(|port| port.name)
            .collect();
        assert_eq!(names, vec!["iq", "baseband", "audio"]);
    }

    #[test]
    fn a_channels_outputs_follow_what_its_type_produces() {
        let names = |descriptor: &ChannelDescriptor| {
            NodeBody::Channel(ChannelNode {
                channel_type: descriptor.type_id.clone(),
                record_calls: false,
            })
            .ports_with(Some(PortBacking::Channel(descriptor)))
            .into_iter()
            .map(|port| port.name)
            .collect::<Vec<_>>()
        };
        let atv = ChannelDescriptor {
            type_id: "atv".to_owned(),
            name: "ATV".to_owned(),
            has_audio: false,
            has_video: true,
            ..ChannelDescriptor::default()
        };
        assert_eq!(names(&atv), vec!["iq", "baseband", "video"]);
        assert_eq!(names(&descriptors()[1]), vec!["iq", "baseband", "events"]);

        let mut graph = workspace();
        graph.nodes.push(node("vid", NodeBody::Video));
        graph.edges.push(edge(("ch", "audio"), ("vid", "video")));
        assert_eq!(
            graph.validate(),
            Err(PatchError::TypeMismatch {
                from: PortType::Audio,
                to: PortType::Video,
            })
        );
    }

    #[test]
    fn a_channel_tap_cannot_be_wired_where_a_wideband_stream_belongs() {
        let catalog = PatchCatalog::build();
        let ports_of = |kind: &str| {
            catalog
                .nodes
                .iter()
                .find(|entry| entry.kind == kind)
                .map(|entry| entry.ports.clone())
                .unwrap_or_default()
        };
        let takes = |kind: &str, port_type: PortType| {
            ports_of(kind)
                .iter()
                .any(|port| port.direction == PortDirection::In && port.port_type == port_type)
        };

        assert!(
            ports_of("channel")
                .iter()
                .any(|port| port.direction == PortDirection::Out
                    && port.port_type == PortType::Baseband),
            "a channel taps out its own passband"
        );
        assert!(!takes("channel", PortType::Baseband));
        assert!(!takes("recorder", PortType::Baseband));
        assert!(takes("recorder", PortType::Iq));
        assert!(!takes("recorder", PortType::Audio));
        assert!(takes("audio_recorder", PortType::Audio));
        assert!(!takes("audio_recorder", PortType::Iq));
        assert!(!takes("signal_map", PortType::Baseband));
        assert!(
            takes("scope", PortType::Baseband),
            "the scope is what reads it"
        );

        let mut graph = workspace();
        graph.edges.push(edge(("ch", "baseband"), ("ch", "iq")));
        assert!(graph.validate().is_err());
    }

    #[test]
    fn a_zero_or_outsize_stream_count_is_clamped() {
        let device = NodeBody::Device(DeviceNode::default());
        let names = |caps: &Capabilities| {
            device
                .ports_with(Some(PortBacking::Device(caps)))
                .into_iter()
                .map(|port| port.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&capabilities(Duplex::RxOnly, 0, 0)),
            vec!["control", "iq"]
        );
        let outsize = names(&capabilities(Duplex::RxOnly, 100, 0));
        assert_eq!(outsize.len() as u32, 1 + MAX_STREAMS);
        assert_eq!(outsize.last().map(String::as_str), Some("iq16"));
    }

    #[test]
    fn validation_resolves_the_bounded_stream_family() {
        let mut streamed = workspace();
        streamed.edges[0] = edge(("dev", "iq3"), ("scope", "iq"));
        streamed.validate().expect("iq3 is within the family");
        streamed
            .validate_against(&descriptors())
            .expect("the registry checks do not disturb the family");

        for bad in ["iq17", "iqx", "iq1", "iq0"] {
            let mut graph = workspace();
            graph.edges[0] = edge(("dev", bad), ("scope", "iq"));
            assert_eq!(
                graph.validate(),
                Err(PatchError::UnknownPort(PortRef {
                    node: "dev".to_owned(),
                    port: bad.to_owned()
                })),
                "{bad} is outside the family"
            );
        }

        let mut channel_family = workspace();
        channel_family.edges[1] = edge(("dev", "iq"), ("ch", "iq2"));
        assert_eq!(
            channel_family.validate(),
            Err(PatchError::UnknownPort(PortRef {
                node: "ch".to_owned(),
                port: "iq2".to_owned()
            }))
        );
    }

    #[test]
    fn arity_is_checked_per_resolved_stream_port() {
        let mut fanned = workspace();
        fanned.edges[0] = edge(("dev", "iq2"), ("scope", "iq"));
        fanned.edges[1] = edge(("dev", "iq2"), ("ch", "iq"));
        fanned.validate().expect("a stream fans out");

        let mut crossed = workspace();
        crossed.edges.push(edge(("dev", "iq3"), ("scope", "iq")));
        assert_eq!(
            crossed.validate(),
            Err(PatchError::PortOccupied(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut doubled = workspace();
        doubled.edges[0] = edge(("dev", "iq2"), ("scope", "iq"));
        doubled.edges.push(edge(("dev", "iq2"), ("scope", "iq")));
        assert_eq!(
            doubled.validate(),
            Err(PatchError::DuplicateEdge(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut backwards = workspace();
        backwards.edges = vec![edge(("ch", "audio"), ("dev", "iq2"))];
        assert_eq!(
            backwards.validate(),
            Err(PatchError::Direction(PortRef {
                node: "dev".to_owned(),
                port: "iq2".to_owned()
            }))
        );
    }

    #[test]
    fn the_rack_is_a_grid_with_no_two_faces_in_one_cell() {
        let graph = workspace();
        let slot = |node: &str, x, y, w, h| RackSlot {
            node: node.to_owned(),
            cell: RackCell { x, y, w, h },
        };

        RackLayout {
            slots: vec![slot("scope", 0, 0, 6, 4), slot("ch", 6, 0, 6, 4)],
        }
        .validate(&graph)
        .expect("side by side");

        assert_eq!(
            RackLayout {
                slots: vec![slot("scope", 0, 0, 6, 4), slot("ch", 3, 2, 6, 4)],
            }
            .validate(&graph),
            Err(PatchError::RackOverlap("ch".to_owned()))
        );

        assert_eq!(
            RackLayout {
                slots: vec![slot("scope", RACK_COLS - 1, 0, 2, 2)],
            }
            .validate(&graph),
            Err(PatchError::RackCell("scope".to_owned()))
        );

        assert_eq!(
            RackLayout {
                slots: vec![slot("ghost", 0, 0, 1, 1)],
            }
            .validate(&graph),
            Err(PatchError::UnknownNode("ghost".to_owned()))
        );

        assert_eq!(
            RackLayout {
                slots: vec![slot("scope", 0, 0, 2, 2), slot("scope", 4, 0, 2, 2)],
            }
            .validate(&graph),
            Err(PatchError::DuplicateRackSlot("scope".to_owned()))
        );
    }

    #[test]
    fn the_catalog_describes_every_node_kind_once() {
        let catalog = PatchCatalog::build();
        for node in &catalog.nodes {
            for port in &node.ports {
                assert_eq!(port.name, port.port_type.as_str());
            }
        }
        let mut kinds: Vec<&str> = catalog.nodes.iter().map(|n| n.kind.as_str()).collect();
        let total = kinds.len();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), total, "a kind is listed twice");

        let channel = catalog
            .nodes
            .iter()
            .find(|n| n.kind == "channel")
            .expect("channel in the palette");
        assert!(channel.needs_channel_type);
        assert_eq!(
            channel
                .ports
                .iter()
                .find(|p| p.name == "audio")
                .map(|p| p.condition),
            Some(PortCondition::ChannelHasAudio)
        );

        let json = serde_json::to_value(&catalog).unwrap();
        assert_eq!(json["nodes"][0]["kind"], "device");
        assert_eq!(json["nodes"][0]["name"], "Device");
        assert_eq!(json["nodes"][0]["category"], "source");
        let ports = &json["nodes"][0]["ports"];
        assert_eq!(ports[0]["port_type"], "control");
        assert_eq!(ports[0]["direction"], "in");
        assert!(
            ports[0].get("repeat").is_none(),
            "the common case stays off the wire"
        );
        assert_eq!(ports[1]["port_type"], "tx");
        assert_eq!(ports[1]["condition"], "device_is_tx_capable");
        assert_eq!(ports[1]["repeat"], "per_tx_stream");
        assert!(ports[1]["note"].is_string(), "the reserved port says why");
        assert_eq!(ports[2]["port_type"], "iq");
        assert_eq!(ports[2]["direction"], "out");
        assert_eq!(ports[2]["repeat"], "per_rx_stream");
        assert!(
            ports[2].get("condition").is_none() && ports[2].get("note").is_none(),
            "the common case stays off the wire"
        );

        let back: PatchCatalog = serde_json::from_value(json).unwrap();
        assert_eq!(back, catalog);
        let bare: PortSpec = serde_json::from_str(
            r#"{"name":"iq","port_type":"iq","direction":"in","multi":false}"#,
        )
        .unwrap();
        assert_eq!(bare.repeat, PortRepeat::Once);

        let gps = catalog
            .nodes
            .iter()
            .find(|node| node.kind == "gps")
            .expect("GPS source in the palette");
        assert_eq!(gps.ports[0].port_type, PortType::Position);
        let position = channel
            .ports
            .iter()
            .find(|port| port.name == "position")
            .expect("position input");
        assert_eq!(position.condition, PortCondition::ChannelNeedsPosition);

        let signal_map = catalog
            .nodes
            .iter()
            .find(|node| node.kind == "signal_map")
            .expect("signal survey in the palette");
        assert_eq!(signal_map.category, NodeCategory::Display);
        assert_eq!(
            signal_map
                .ports
                .iter()
                .map(|port| port.port_type)
                .collect::<Vec<_>>(),
            [PortType::Iq, PortType::Position]
        );
    }

    #[test]
    fn signal_map_settings_are_bounded() {
        let mut graph = PatchGraph {
            nodes: vec![node(
                "survey",
                NodeBody::SignalMap(SignalMapNode::default()),
            )],
            edges: Vec::new(),
        };
        assert_eq!(graph.validate(), Ok(()));

        let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
            panic!("signal map");
        };
        settings.offset_hz = MAX_SIGNAL_MAP_OFFSET_HZ + 1;
        assert_eq!(
            graph.validate(),
            Err(PatchError::NodeSettings("survey".to_owned()))
        );

        let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
            panic!("signal map");
        };
        settings.offset_hz = -(MAX_SIGNAL_MAP_OFFSET_HZ + 1);
        assert_eq!(
            graph.validate(),
            Err(PatchError::NodeSettings("survey".to_owned()))
        );

        let NodeBody::SignalMap(settings) = &mut graph.nodes[0].body else {
            panic!("signal map");
        };
        settings.offset_hz = DEFAULT_SIGNAL_MAP_OFFSET_HZ;
        settings.bandwidth_hz = 0;
        assert_eq!(
            graph.validate(),
            Err(PatchError::NodeSettings("survey".to_owned()))
        );
    }

    #[test]
    fn propagation_takes_decoder_events_and_a_station_position() {
        let catalog = PatchCatalog::build();
        let propagation = catalog
            .nodes
            .iter()
            .find(|node| node.kind == "propagation")
            .expect("propagation map in the palette");
        assert_eq!(propagation.category, NodeCategory::Display);
        assert_eq!(
            propagation
                .ports
                .iter()
                .map(|port| (port.port_type, port.direction))
                .collect::<Vec<_>>(),
            [
                (PortType::Events, PortDirection::In),
                (PortType::Position, PortDirection::In),
            ]
        );
    }

    #[test]
    fn propagation_settings_are_bounded() {
        let mut graph = PatchGraph {
            nodes: vec![node(
                "prop",
                NodeBody::Propagation(crate::propagation::PropagationNode::default()),
            )],
            edges: Vec::new(),
        };
        assert_eq!(graph.validate(), Ok(()));

        for broken in [
            crate::propagation::PropagationNode {
                half_life_minutes: 0,
                ..Default::default()
            },
            crate::propagation::PropagationNode {
                reflection_height_km: 1_000,
                ..Default::default()
            },
        ] {
            graph.nodes[0].body = NodeBody::Propagation(broken);
            assert_eq!(
                graph.validate(),
                Err(PatchError::NodeSettings("prop".to_owned()))
            );
        }
    }

    #[test]
    fn gps_source_settings_are_structurally_bounded() {
        let mut graph = PatchGraph {
            nodes: vec![node(
                "gps",
                NodeBody::Gps(GpsNode {
                    source: PositionSource::Nmea {
                        device: "/dev/ttyUSB0".to_owned(),
                        baud: 9_600,
                        update_interval_ms: 1_000,
                    },
                }),
            )],
            edges: Vec::new(),
        };
        assert_eq!(graph.validate(), Ok(()));
        if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
            gps.source = PositionSource::Nmea {
                device: "/dev/ttyUSB0".to_owned(),
                baud: 9_600,
                update_interval_ms: 49,
            };
        }
        assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
        if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
            gps.source = PositionSource::Gpsd {
                address: String::new(),
            };
        }
        assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
        if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
            gps.source = PositionSource::Gpsd {
                address: "not-an-endpoint".to_owned(),
            };
        }
        assert!(matches!(graph.validate(), Err(PatchError::Gps(_))));
        for address in [
            "localhost:0",
            "localhost:gps",
            "[::1:2947",
            "[::1]]:2947",
            "bad host:2947",
        ] {
            if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
                gps.source = PositionSource::Gpsd {
                    address: address.to_owned(),
                };
            }
            assert!(
                matches!(graph.validate(), Err(PatchError::Gps(_))),
                "accepted invalid GPSD endpoint {address}"
            );
        }
        if let NodeBody::Gps(gps) = &mut graph.nodes[0].body {
            gps.source = PositionSource::Gpsd {
                address: "[::1]:2947".to_owned(),
            };
        }
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn default_params_come_from_the_type_id() {
        let params = ChannelParams::default_for("ssb").expect("ssb is a channel type");
        assert_eq!(params.type_id(), "ssb");
        assert_eq!(
            ChannelSettings::default_for("ssb").expect("ssb is a channel type"),
            serde_json::from_str(r#"{"params":{"type":"ssb","settings":{}}}"#).unwrap()
        );
        assert_eq!(ChannelParams::default_for("wefax"), None);
        assert_eq!(ChannelSettings::default_for("wefax"), None);
    }
}
