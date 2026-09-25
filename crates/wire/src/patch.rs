use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    EventOutputNode, GpsNode, MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS,
    MAX_POSITION_ENDPOINT_LEN, MIN_NMEA_BAUD, MIN_NMEA_UPDATE_INTERVAL_MS, PositionSource,
    channel::{ChannelDescriptor, ChannelParams},
    coherent::{CombinerParams, DfParams, PassiveRadarParams, StitchParams},
    device::{
        ArrayDefinition, Capabilities, Coherence, DeviceInfo, Direction, RECORDING_DRIVER_ID,
        SIGGEN_DRIVER_ID, recording_stem_valid,
    },
    filter::EventFilterNode,
    network::{MAX_NETWORK_ADDRESS_LEN, NetworkExportNode},
    propagation::PropagationNode,
    satellite::SatelliteNode,
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
    Tool,
    Output,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked_streams: Vec<u32>,
}

impl DeviceNode {
    #[must_use]
    pub fn tuning_locked(&self, stream: u32) -> bool {
        self.locked_streams.contains(&stream)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecordingNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<String>,
}

impl RecordingNode {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.recording.as_deref().is_none_or(recording_stem_valid)
    }

    #[must_use]
    pub fn device_ref(&self) -> Option<DeviceRef> {
        let stem = self
            .recording
            .as_deref()
            .filter(|stem| recording_stem_valid(stem))?;
        Some(DeviceRef {
            backend: RECORDING_DRIVER_ID.to_owned(),
            serial: None,
            key: Some(stem.to_owned()),
        })
    }
}

#[must_use]
pub fn siggen_key(node_id: &str) -> String {
    node_id.replace(':', "-")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct SignalGenNode {
    pub running: bool,
}

impl Default for SignalGenNode {
    fn default() -> Self {
        Self { running: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChannelNode {
    pub channel_type: String,
    #[serde(default)]
    pub record_calls: bool,
    #[serde(default)]
    pub tuning_locked: bool,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct RecorderNode {
    pub recording: bool,
}

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

/// What a hunt node remembers between sessions: whether the operator wanted a click track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HuntNode {
    #[serde(default = "default_clicks")]
    pub clicks: bool,
}

const fn default_clicks() -> bool {
    true
}

impl Default for HuntNode {
    fn default() -> Self {
        Self {
            clicks: default_clicks(),
        }
    }
}

/// A bank of separate radios the operator has wired to one clock, standing on the canvas as the
/// one radio they add up to.
///
/// Nothing here is discovered: which radios belong together, and whether their clock alone is
/// shared or their synthesizer too, is a fact about the bench that only the operator knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct ArrayNode {
    /// How many radios are wired in. The node always draws one more input than that, so there is
    /// somewhere to put the next one.
    pub members: u32,
    pub coherence: Coherence,
    pub shared_tuning: bool,
}

impl Default for ArrayNode {
    fn default() -> Self {
        Self {
            members: 0,
            coherence: Coherence::TimeSync,
            shared_tuning: true,
        }
    }
}

impl ArrayNode {
    /// The composite this node describes, given the radios wired into it. Its key comes from the
    /// node itself, so an array is set up by drawing it rather than by naming it somewhere else.
    #[must_use]
    pub fn definition(
        &self,
        node_id: &str,
        label: Option<&str>,
        members: Vec<String>,
    ) -> ArrayDefinition {
        ArrayDefinition {
            key: array_key(node_id),
            label: label.unwrap_or("Array").to_owned(),
            members,
            coherence: self.coherence,
            shared_tuning: self.shared_tuning,
        }
    }
}

/// A node name made safe to use as a device key, so the array a patch draws is the array the
/// driver opens without anything in between deciding what it is called.
#[must_use]
pub fn array_key(node_id: &str) -> String {
    node_id.replace(':', "-")
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

/// A bank of antennas added into one signal: either to hear better, or to stop hearing something.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CombinerNode {
    #[serde(default)]
    pub settings: CombinerParams,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StitchNode {
    #[serde(default)]
    pub settings: StitchParams,
}

pub const RADAR_REFERENCE_PORT: &str = "ref";
pub const RADAR_SURVEILLANCE_PORT: &str = "surv";
pub const DF_BEAM_PORT: &str = "beam";
pub const STITCH_WIDE_PORT: &str = "wide";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum NodeBody {
    Device(DeviceNode),
    Recording(RecordingNode),
    SignalGen(SignalGenNode),
    Array(ArrayNode),
    Gps(GpsNode),
    Channel(ChannelNode),
    Scope,
    BasebandScope,
    Speaker,
    Map,
    SignalMap(SignalMapNode),
    Propagation(PropagationNode),
    Readout,
    DecoderLog,
    DmrTrunk(DmrTrunkNode),
    SpectrumMonitor(crate::SpectrumMonitorNode),
    EventOutput(EventOutputNode),
    EventFilter(EventFilterNode),
    AudioFx(crate::AudioFxNode),
    Video,
    Recorder(RecorderNode),
    AudioRecorder(RecorderNode),
    BasebandRecorder(RecorderNode),
    TimeMachine(TimeMachineNode),
    NetworkExport(NetworkExportNode),
    Export,
    Scanner(crate::scan::ScannerNode),
    Hunt(HuntNode),
    Satellite(SatelliteNode),
    Df(DfNode),
    PassiveRadar(PassiveRadarNode),
    Combiner(CombinerNode),
    Stitch(StitchNode),
    Triangulation,
}

impl NodeBody {
    #[must_use]
    pub fn default_for(kind: &str) -> Option<Self> {
        let data = match kind {
            "df" => return Some(Self::Df(DfNode::default())),
            "passive_radar" => return Some(Self::PassiveRadar(PassiveRadarNode::default())),
            "combiner" => return Some(Self::Combiner(CombinerNode::default())),
            "stitch" => return Some(Self::Stitch(StitchNode::default())),
            "channel" => serde_json::json!({ "channel_type": "nfm", "record_calls": false }),
            "signal_gen" => serde_json::json!({ "running": true }),
            "array" => serde_json::json!({
                "members": 0,
                "coherence": "time_sync",
                "shared_tuning": true
            }),
            "signal_map" => serde_json::json!({ "offset_hz": 0, "bandwidth_hz": 12_500 }),
            "propagation" => serde_json::json!({
                "half_life_minutes": 30,
                "reflection_height_km": 300,
                "show_paths": false,
                "compare_forecast": true
            }),
            "spectrum_monitor" => {
                serde_json::json!({ "record_audio": true, "min_confidence": 0.7 })
            }
            "dmr_trunk" => serde_json::json!({ "protocol": "auto", "record_calls": true }),
            "event_filter" => serde_json::json!({
                "mode": "keep",
                "kinds": [],
                "stations": [],
                "talkgroups": [],
                "radios": [],
                "min_duration_ms": 0
            }),
            "audio_fx" => serde_json::json!({ "settings": {} }),
            "recorder" | "audio_recorder" | "baseband_recorder" => {
                serde_json::json!({ "recording": false })
            }
            "network_export" => serde_json::json!({
                "transport": "udp",
                "format": "cf32_le",
                "address": "127.0.0.1:7355"
            }),
            "hunt" => serde_json::json!({ "clicks": true }),
            "time_machine" => serde_json::json!({ "history_seconds": 10 }),
            "event_output" => serde_json::json!({
                "target": { "service": "webhook", "url": "", "format": "json" }
            }),
            _ => serde_json::json!({}),
        };
        serde_json::from_value(serde_json::json!({ "kind": kind, "data": data }))
            .or_else(|_| serde_json::from_value(serde_json::json!({ "kind": kind })))
            .ok()
    }

    #[must_use]
    pub const fn lane_output(&self) -> Option<&'static str> {
        match self {
            Self::Df(_) | Self::Combiner(_) => Some(DF_BEAM_PORT),
            Self::Stitch(_) => Some(STITCH_WIDE_PORT),
            _ => None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Device(_) => "device",
            Self::Recording(_) => "recording",
            Self::SignalGen(_) => "signal_gen",
            Self::Gps(_) => "gps",
            Self::Channel(_) => "channel",
            Self::Scope => "scope",
            Self::BasebandScope => "baseband_scope",
            Self::Speaker => "speaker",
            Self::Map => "map",
            Self::SignalMap(_) => "signal_map",
            Self::Propagation(_) => "propagation",
            Self::Readout => "readout",
            Self::DecoderLog => "decoder_log",
            Self::DmrTrunk(_) => "dmr_trunk",
            Self::SpectrumMonitor(_) => "spectrum_monitor",
            Self::EventOutput(_) => "event_output",
            Self::EventFilter(_) => "event_filter",
            Self::AudioFx(_) => "audio_fx",
            Self::Video => "video",
            Self::Recorder(_) => "recorder",
            Self::AudioRecorder(_) => "audio_recorder",
            Self::BasebandRecorder(_) => "baseband_recorder",
            Self::TimeMachine(_) => "time_machine",
            Self::NetworkExport(_) => "network_export",
            Self::Export => "export",
            Self::Scanner(_) => "scanner",
            Self::Hunt(_) => "hunt",
            Self::Satellite(_) => "satellite",
            Self::Df(_) => "df",
            Self::PassiveRadar(_) => "passive_radar",
            Self::Array(_) => "array",
            Self::Combiner(_) => "combiner",
            Self::Stitch(_) => "stitch",
            Self::Triangulation => "triangulation",
        }
    }

    #[must_use]
    pub const fn category(&self) -> NodeCategory {
        match self {
            Self::Device(_) | Self::Recording(_) | Self::SignalGen(_) | Self::Gps(_) => {
                NodeCategory::Source
            }
            Self::Channel(_) => NodeCategory::Channel,
            Self::Array(_)
            | Self::Df(_)
            | Self::PassiveRadar(_)
            | Self::Combiner(_)
            | Self::Stitch(_)
            | Self::Scanner(_)
            | Self::Hunt(_)
            | Self::Satellite(_)
            | Self::SpectrumMonitor(_)
            | Self::DmrTrunk(_)
            | Self::EventFilter(_)
            | Self::AudioFx(_)
            | Self::Triangulation => NodeCategory::Tool,
            Self::Scope
            | Self::BasebandScope
            | Self::Map
            | Self::SignalMap(_)
            | Self::Propagation(_)
            | Self::Readout
            | Self::DecoderLog
            | Self::Video
            | Self::Speaker
            | Self::Recorder(_)
            | Self::AudioRecorder(_)
            | Self::BasebandRecorder(_)
            | Self::TimeMachine(_)
            | Self::NetworkExport(_)
            | Self::EventOutput(_)
            | Self::Export => NodeCategory::Output,
        }
    }

    /// The radio a node opens, if it opens one. An array names itself, because the composite it
    /// describes exists only as long as the node drawing it does.
    #[must_use]
    pub const fn opens_device(&self) -> bool {
        matches!(
            self,
            Self::Device(_) | Self::Recording(_) | Self::SignalGen(_) | Self::Array(_)
        )
    }

    #[must_use]
    pub fn device_ref(&self, node_id: &str) -> Option<DeviceRef> {
        match self {
            Self::Device(device) => device.device.clone(),
            Self::Recording(recording) => recording.device_ref(),
            Self::SignalGen(generator) => generator.running.then(|| DeviceRef {
                backend: SIGGEN_DRIVER_ID.to_owned(),
                serial: None,
                key: Some(siggen_key(node_id)),
            }),
            Self::Array(array) => (array.members > 0).then(|| DeviceRef {
                backend: crate::device::ARRAY_DRIVER_ID.to_owned(),
                serial: None,
                key: Some(array_key(node_id)),
            }),
            _ => None,
        }
    }

    #[must_use]
    pub fn ports(&self) -> Vec<PortSpec> {
        let specs = ports_for(self.kind());
        match self {
            Self::Df(df) => spread_lanes(specs, df.settings.geometry.count()),
            Self::Combiner(combiner) => spread_lanes(specs, combiner.settings.lanes),
            Self::Stitch(stitch) => spread_lanes(specs, stitch.settings.lanes),
            Self::Array(array) => spread_array(specs, array.members),
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

/// An array draws one input more than it has radios, so the next one has somewhere to go, and one
/// output per radio it already has.
fn spread_array(specs: Vec<PortSpec>, members: u32) -> Vec<PortSpec> {
    let mut out = Vec::new();
    for spec in specs {
        if spec.repeat != PortRepeat::PerRxStream {
            out.push(spec);
            continue;
        }
        let lanes = match spec.direction {
            PortDirection::In => members.saturating_add(1),
            PortDirection::Out => members,
        }
        .min(MAX_STREAMS);
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
            PortSpec::new(Tx, In, false, DeviceIsTxCapable)
                .repeated(PortRepeat::PerTxStream)
                .noted(
                    "reserved: transmit is not built (), so nothing in this build emits \
                     a signal to key a radio with",
                ),
            PortSpec::new(Iq, Out, true, Always).repeated(PortRepeat::PerRxStream),
        ],
        "recording" => vec![PortSpec::new(Iq, Out, true, Always)],
        "signal_gen" => vec![PortSpec::new(Iq, Out, true, Always)],
        "array" => vec![
            PortSpec::new(Iq, In, false, Always)
                .repeated(PortRepeat::PerRxStream)
                .noted("one radio per input, in the order their antennas sit in the array"),
            PortSpec::new(Iq, Out, true, Always).repeated(PortRepeat::PerRxStream),
        ],
        "gps" => vec![PortSpec::new(Position, Out, true, Always)],
        "channel" => vec![
            PortSpec::new(Iq, In, true, Always)
                .noted("every radio that may carry this decoder; it runs on the one that hears it"),
            PortSpec::new(Control, In, false, Always).noted(
                "a scanner, signal hunt or satellite drives this decoder; its radio follows",
            ),
            PortSpec::new(Position, In, false, ChannelNeedsPosition),
            PortSpec::new(Baseband, Out, true, Always),
            PortSpec::new(Audio, Out, true, ChannelHasAudio),
            PortSpec::new(Events, Out, true, ChannelIsDecoder),
            PortSpec::new(Video, Out, true, ChannelHasVideo),
        ],
        "scope" => vec![PortSpec::new(Iq, In, false, Always)],
        "baseband_scope" => vec![PortSpec::new(Baseband, In, false, Always)],
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
        "satellite" => vec![
            PortSpec::new(Position, In, false, Always),
            PortSpec::new(Control, Out, true, Always).noted(
                "every decoder listening to this satellite; each is tuned and Doppler corrected",
            ),
        ],
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
        "spectrum_monitor" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
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
        "audio_fx" => vec![
            PortSpec::new(Audio, In, true, Always),
            PortSpec::new(Audio, Out, true, Always),
        ],
        "triangulation" => vec![
            PortSpec::new(Events, In, true, Always)
                .noted("every direction finder whose bearings should be crossed together"),
            PortSpec::new(Events, Out, true, Always),
        ],
        "df" => vec![
            PortSpec::new(Iq, In, false, Always).repeated(PortRepeat::PerRxStream),
            PortSpec::new(Position, In, false, Always),
            PortSpec::new(Events, Out, true, Always),
            PortSpec::named(DF_BEAM_PORT, Iq, Out, true, Always)
                .noted("the array summed towards the bearing it found, as one more radio lane"),
        ],
        "combiner" => vec![
            PortSpec::new(Iq, In, false, Always)
                .repeated(PortRepeat::PerRxStream)
                .noted("lane one is the antenna pointed at what you want"),
            PortSpec::named(DF_BEAM_PORT, Iq, Out, true, Always)
                .noted("the antennas added together, as one more radio lane"),
        ],
        "stitch" => vec![
            PortSpec::new(Iq, In, false, Always).repeated(PortRepeat::PerRxStream),
            PortSpec::named(STITCH_WIDE_PORT, Iq, Out, true, Always)
                .noted("every lane joined into one wide stream, as one more radio lane"),
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
    #[serde(default)]
    pub summary: String,
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
        // The catalog describes a kind, not one drawn node, so repeated ports stay repeated here:
        // how many a node actually carries depends on its own settings and is worked out where
        // that node is drawn.
        let entry = |body: &NodeBody, name: &str, summary: &str| NodeTypeInfo {
            kind: body.kind().to_owned(),
            name: name.to_owned(),
            summary: summary.to_owned(),
            category: body.category(),
            ports: ports_for(body.kind()),
            needs_channel_type: matches!(body, NodeBody::Channel(_)),
        };
        Self {
            nodes: vec![
                entry(
                    &NodeBody::Device(DeviceNode::default()),
                    "Device",
                    "A radio, where every patch starts",
                ),
                entry(
                    &NodeBody::Recording(RecordingNode::default()),
                    "Recording",
                    "Plays back a recorded IQ file",
                ),
                entry(
                    &NodeBody::SignalGen(SignalGenNode::default()),
                    "Signal generator",
                    "Test signals without a radio",
                ),
                entry(
                    &NodeBody::Array(ArrayNode::default()),
                    "Array",
                    "Several radios as one coherent array",
                ),
                entry(
                    &NodeBody::Gps(GpsNode::default()),
                    "GPS position",
                    "Your station location, live or fixed",
                ),
                entry(
                    &NodeBody::Channel(ChannelNode {
                        channel_type: String::new(),
                        record_calls: false,
                        tuning_locked: false,
                    }),
                    "Channel",
                    "Tunes and decodes one signal",
                ),
                entry(&NodeBody::Scope, "Scope", "Spectrum and waterfall"),
                entry(
                    &NodeBody::BasebandScope,
                    "Baseband scope",
                    "Constellation and eye of one channel",
                ),
                entry(&NodeBody::Speaker, "Speaker", "Plays channel audio"),
                entry(&NodeBody::Map, "Map", "Decoded positions on a map"),
                entry(
                    &NodeBody::SignalMap(SignalMapNode::default()),
                    "Signal survey",
                    "Maps signal strength while you move",
                ),
                entry(
                    &NodeBody::Propagation(PropagationNode::default()),
                    "Propagation map",
                    "Where FT8, FT4 and WSPR signals came from",
                ),
                entry(&NodeBody::Readout, "Readout", "Current decoder state"),
                entry(
                    &NodeBody::DecoderLog,
                    "Decoder log",
                    "Every decoded message in a table",
                ),
                entry(
                    &NodeBody::SpectrumMonitor(crate::SpectrumMonitorNode::default()),
                    "Spectrum monitor",
                    "Catches and decodes everything in view",
                ),
                entry(
                    &NodeBody::DmrTrunk(DmrTrunkNode::default()),
                    "DMR trunk system",
                    "Follows calls across a DMR trunk system",
                ),
                entry(
                    &NodeBody::EventFilter(EventFilterNode::default()),
                    "Event filter",
                    "Passes only matching events",
                ),
                entry(
                    &NodeBody::AudioFx(crate::AudioFxNode::default()),
                    "Audio FX",
                    "Filters, denoise and AGC",
                ),
                entry(
                    &NodeBody::EventOutput(EventOutputNode::default()),
                    "Event output",
                    "Sends events to other programs",
                ),
                entry(&NodeBody::Video, "Video", "ATV frames and SSTV pictures"),
                entry(
                    &NodeBody::Recorder(RecorderNode::default()),
                    "Recorder",
                    "Records a radio's full IQ",
                ),
                entry(
                    &NodeBody::AudioRecorder(RecorderNode::default()),
                    "Audio recorder",
                    "Records channel audio to WAV",
                ),
                entry(
                    &NodeBody::BasebandRecorder(RecorderNode::default()),
                    "Baseband recorder",
                    "Records one channel's IQ",
                ),
                entry(
                    &NodeBody::TimeMachine(TimeMachineNode::default()),
                    "Time machine",
                    "Saves IQ from before you pressed record",
                ),
                entry(
                    &NodeBody::NetworkExport(NetworkExportNode::default()),
                    "Network IQ",
                    "Streams IQ to other programs",
                ),
                entry(
                    &NodeBody::Export,
                    "Export",
                    "Saves logged rows as CSV or JSON",
                ),
                entry(
                    &NodeBody::Scanner(crate::scan::ScannerNode::default()),
                    "Scanner",
                    "Steps through frequencies, stops on activity",
                ),
                entry(
                    &NodeBody::Hunt(HuntNode::default()),
                    "Signal hunt",
                    "Walks you towards a transmitter",
                ),
                entry(
                    &NodeBody::Satellite(SatelliteNode::default()),
                    "Satellite",
                    "Predicts passes and follows Doppler",
                ),
                entry(
                    &NodeBody::Df(DfNode::default()),
                    "Direction finder",
                    "Bearing to a transmitter",
                ),
                entry(
                    &NodeBody::PassiveRadar(PassiveRadarNode::default()),
                    "Passive radar",
                    "Finds aircraft in broadcast reflections",
                ),
                entry(
                    &NodeBody::Combiner(CombinerNode::default()),
                    "Combiner",
                    "Adds antennas together",
                ),
                entry(
                    &NodeBody::Stitch(StitchNode::default()),
                    "Stitch",
                    "Joins lanes into one wide stream",
                ),
                entry(
                    &NodeBody::Triangulation,
                    "Triangulation",
                    "Crosses bearings into a position",
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
        self.nodes.iter().filter(|node| node.body.opens_device())
    }

    /// The radios wired into an array, in the order of the ports they arrive on, which is the
    /// order their lanes are numbered.
    #[must_use]
    pub fn array_members(&self, node: &str) -> Vec<&str> {
        let Some(PatchNode {
            body: NodeBody::Array(array),
            ..
        }) = self.node(node)
        else {
            return Vec::new();
        };
        (0..array.members.saturating_add(1))
            .filter_map(|element| {
                let port = stream_port("iq", element);
                self.edges
                    .iter()
                    .find(|edge| edge.to.node == node && edge.to.port == port)
                    .map(|edge| edge.from.node.as_str())
            })
            .collect()
    }

    /// The array a radio belongs to, if one has taken it. A radio in an array is tuned by the
    /// array and opened by it, so nothing else may claim it at the same time.
    #[must_use]
    pub fn array_holding(&self, device_node: &str) -> Option<&str> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.body, NodeBody::Array(_)))
            .find(|array| self.array_members(&array.id).contains(&device_node))
            .map(|array| array.id.as_str())
    }

    #[must_use]
    pub fn lanes_of(&self, channel_node: &str) -> Vec<(&str, u32)> {
        self.edges
            .iter()
            .filter(|edge| edge.to.node == channel_node && edge.to.port == "iq")
            .filter_map(|edge| {
                let source = self.node(&edge.from.node)?;
                if !source.body.opens_device() {
                    return None;
                }
                Some((edge.from.node.as_str(), port_stream("iq", &edge.from.port)?))
            })
            .collect()
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
        self.nodes
            .iter()
            .filter(|node| matches!(node.body, NodeBody::Channel(_)))
            .flat_map(move |node| {
                self.edges.iter().filter_map(move |edge| {
                    if edge.to.node != node.id
                        || edge.to.port != "iq"
                        || edge.from.node != device_node
                    {
                        return None;
                    }
                    port_stream("iq", &edge.from.port).map(|stream| (node, stream))
                })
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
                NodeBody::Recording(recording) if !recording.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::EventFilter(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::AudioFx(fx) if fx.settings.validate().is_err() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::SpectrumMonitor(settings) if !settings.valid() => {
                    return Err(PatchError::NodeSettings(node.id.clone()));
                }
                NodeBody::Gps(gps) => {
                    if let Some(source) = &gps.source {
                        validate_gps_source(source)?;
                    }
                }
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
                    if !export.settings.valid_format()
                        || export.settings.address.is_empty()
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
                NodeBody::Satellite(satellite) if !satellite.valid() => {
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
            if input.port_type == PortType::Iq
                && self
                    .node(&edge.to.node)
                    .is_some_and(|node| matches!(node.body, NodeBody::Channel(_)))
                && self
                    .edges
                    .iter()
                    .any(|other| other.to == edge.to && other.from != edge.from)
                && self
                    .edges
                    .iter()
                    .filter(|other| other.to == edge.to)
                    .any(|other| {
                        self.node(&other.from.node)
                            .is_some_and(|node| !node.body.opens_device())
                    })
            {
                return Err(PatchError::PortOccupied(edge.to.clone()));
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
        PositionSource::Fixed { lat, lon, .. } => {
            if (-90.0..=90.0).contains(lat) && (-180.0..=180.0).contains(lon) {
                Ok(())
            } else {
                Err(PatchError::Gps(
                    "a fixed position has to be on the globe".to_owned(),
                ))
            }
        }
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
