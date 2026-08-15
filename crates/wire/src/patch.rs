use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    GpsNode, MAX_NMEA_BAUD, MAX_NMEA_UPDATE_INTERVAL_MS, MAX_POSITION_ENDPOINT_LEN, MIN_NMEA_BAUD,
    MIN_NMEA_UPDATE_INTERVAL_MS, PositionSource,
    channel::{ChannelDescriptor, ChannelParams},
    device::{Capabilities, DeviceInfo, Direction},
    workspace::MAX_NAME_LEN,
};

/// Structural caps. A graph is stored whole in one row and rewritten on every arrangement
/// gesture, so the bounds are what keeps one row from growing without limit; the numbers are far
/// above any workspace a person would draw.
pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 256;
pub const MAX_NODE_ID_LEN: usize = 64;
/// Canvas coordinates are unbounded in React Flow; this is the box a stored position must sit in
/// so a corrupt write cannot park a node where no camera will find it.
pub const MAX_COORD: f32 = 100_000.0;
pub const MAX_NODE_SIZE: f32 = 10_000.0;
pub const RACK_COLS: u16 = 12;
pub const RACK_ROWS: u16 = 8;
/// Bounds a stored port string, not live hardware: validation is pure over the stored graph, so
/// the family a repeating port admits must end somewhere or any `iq…` digits would be a storable
/// name. A KrakenSDR is 5 coherent streams; sixteen leaves headroom.
pub const MAX_STREAMS: u32 = 16;

/// The port name for stream `index` of the family `base`.
///
/// Stream 0 keeps the bare name — every stored workspace, template and e2e selector names it —
/// so the visible numbering starts at 2: `iq`, `iq2`, `iq3`, …
#[must_use]
pub fn stream_port(base: &str, index: u32) -> String {
    if index == 0 {
        base.to_owned()
    } else {
        format!("{base}{}", index + 1)
    }
}

/// The stream `name` addresses within family `base`, if it is one of that family's.
///
/// One spelling per port: `iq1` would alias `iq` and `iq02` would alias `iq2`, so only the
/// canonical rendering of `2..=MAX_STREAMS` names a stream.
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
    /// Wideband complex baseband at the device rate.
    Iq,
    /// One channel's complex baseband at *its* rate: after the down-conversion and the channel
    /// filter, before the demodulator.
    ///
    /// Deliberately not [`PortType::Iq`]. A channel tap is not interchangeable with a radio's
    /// wideband stream — nothing can host a channel on one, and typing the two the same would
    /// make `channel → channel` a wireable cycle that the engine could never build.
    Baseband,
    /// 48 kHz demodulated audio (Opus on the wire).
    Audio,
    /// Typed decoder and completed-call events.
    Events,
    /// Scanned pictures, one raster per field (`VIDEO_GRAY` or `VIDEO_RGB` on the wire, ATV).
    Video,
    Control,
    /// Live station coordinates and motion.
    Position,
    /// Complex baseband to be transmitted at the device rate.
    ///
    /// **Reserved, and inert by construction.** No node kind in this build emits it, so no edge
    /// into a transmit input can validate — the port is the shape transmit will arrive in, not a
    /// path to it. The authorized-use gate has to exist first.
    ///
    /// The input it sits on is [`PortCondition::DeviceIsTxCapable`], so it is drawn on the radios
    /// that have a send side and nowhere else — an RTL-SDR node has no transmit input at all.
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

/// Which side of a node a port sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortDirection {
    In,
    Out,
}

/// When a port exists. A conditional port depends on what is *behind* the node — the channel type
/// it names, or the radio it is bound to — and those answers live once in [`ChannelDescriptor`]
/// and [`Capabilities`]. The catalog states the dependency instead of the client inventing port
/// names for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortCondition {
    #[default]
    Always,
    /// Only when the channel type produces listenable audio.
    ChannelHasAudio,
    /// Only when the channel type emits decoder events.
    ChannelIsDecoder,
    /// Only when the channel type scans out a picture.
    ChannelHasVideo,
    /// Only when the channel uses the station position while decoding.
    ChannelNeedsPosition,
    /// Only on a radio that has a transmit side ([`Capabilities::duplex`]). Unlike the channel
    /// conditions this one is answered by the *binding* rather than by the stored node: which
    /// radio a device node names is stored, but what that radio can do is only known while it is
    /// attached. A node naming no radio, or one that is not plugged in, has nothing to ask — and
    /// hides the port rather than guessing at it.
    DeviceIsTxCapable,
}

/// What a node's conditional ports are resolved against: whatever is behind the node. Never
/// stored and never on the wire — it is the argument to [`PortSpec::applies_to`], assembled by
/// each caller from the tables it already holds.
#[derive(Clone, Copy, Debug)]
pub enum PortBacking<'a> {
    /// The descriptor of the type a channel node names.
    Channel(&'a ChannelDescriptor),
    /// The capabilities of the radio a device node is bound to.
    Device(&'a Capabilities),
}

/// How many of a port a node really has. A repeating port is a *family*: the catalog is
/// per-build static and cannot see how many streams a radio delivers, so it ships the base spec
/// with this flag and whoever can see the backing expands it — one port per stream, named by
/// [`stream_port`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortRepeat {
    #[default]
    Once,
    PerRxStream,
    PerTxStream,
}

impl PortRepeat {
    /// How many ports this spec expands to on a node with `backing` behind it. Clamped low
    /// because a radio reporting 0 rx streams still has the one IQ port every stored wire names,
    /// and high because a port past [`MAX_STREAMS`] could never take a valid wire.
    fn count(self, backing: Option<PortBacking<'_>>) -> u32 {
        match (self, backing) {
            (Self::Once, _) => 1,
            (Self::PerRxStream, Some(PortBacking::Device(caps))) => {
                caps.rx_streams.clamp(1, MAX_STREAMS)
            }
            (Self::PerTxStream, Some(PortBacking::Device(caps))) => {
                caps.tx_streams.clamp(1, MAX_STREAMS)
            }
            // No backing: stream 0 only. A port drawn on a guess is one the operator can be
            // told to use and then refused.
            (Self::PerRxStream | Self::PerTxStream, _) => 1,
        }
    }
}

/// One port of a node type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PortSpec {
    /// Stable slug, unique within its node and direction; this is what an edge names.
    pub name: String,
    pub port_type: PortType,
    pub direction: PortDirection,
    /// Whether more than one edge may touch this port. A *stream* output fans out — one device
    /// feeds N channels, scopes and a recorder, which is today's device set drawn — but an
    /// ownership output does not: a scanner drives one radio, because one sweep is what the
    /// engine runs. So arity is stated on both sides and checked on both sides.
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

    /// Whether this port exists on a node with `backing` behind it. `None` is a node with nothing
    /// behind it yet — a device naming no radio, or naming one that is not attached — which keeps
    /// every conditional port off it: a port drawn on a guess is one the operator can be told to
    /// use and then refused.
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
    /// Driver id, matching [`DeviceInfo::driver`]: `"rtlsdr"`, `"hackrf"`, `"soapy"`, `"virtual"`.
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Per-driver key, used without a serial or to distinguish variants sharing one serial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl DeviceRef {
    /// The reference that names this discovered device.
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

    /// Whether `info` is the device this reference names. Serial identifies the physical radio;
    /// an accompanying key narrows that to a variant. Without a serial the key identifies the
    /// device; a backend with a single serial-less device can match on the backend alone.
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

/// A channel node's payload. The *type* is topology — it decides the node's ports — while the
/// settings behind it stay on the engine's channel (module docs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChannelNode {
    /// [`ChannelDescriptor::type_id`]: `"nfm"`, `"adsb"`, `"subghz"`, …
    pub channel_type: String,
}

const fn default_call_retention_seconds() -> u32 {
    300
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DmrTrunkProtocol {
    #[default]
    Auto,
    CapacityPlus,
    HyteraXpt,
    TierThree,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrTrunkNode {
    #[serde(default)]
    pub protocol: DmrTrunkProtocol,
    #[serde(default = "default_call_retention_seconds")]
    pub retention_seconds: u32,
}

pub const DEFAULT_SIGNAL_MAP_OFFSET_HZ: i64 = 0;
pub const DEFAULT_SIGNAL_MAP_BANDWIDTH_HZ: u64 = 12_500;
pub const MAX_SIGNAL_MAP_OFFSET_HZ: i64 = 1_000_000_000_000;
pub const MAX_SIGNAL_MAP_BANDWIDTH_HZ: u64 = 100_000_000;

/// The IQ-relative slice a signal survey measures while pairing spectrum frames with positions.
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
            retention_seconds: default_call_retention_seconds(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum NodeBody {
    Device(DeviceNode),
    Gps(GpsNode),
    Channel(ChannelNode),
    /// Spectrum + waterfall over a device's IQ.
    Scope,
    /// Client-side audio mix (: the server ships streams, not a mix).
    Speaker,
    /// MapLibre, one layer per connected decoder.
    Map,
    /// A drive survey of one RF slice, pairing spectrum power with GPS fixes.
    SignalMap(SignalMapNode),
    /// The live picture a decoder holds — an RDS station, a table of aircraft, a teleprinter
    /// roll — one readout per connected decoder. This is the *state* a decoder accumulates, which
    /// is the half of its output that a log row cannot carry; the frames themselves are read in
    /// [`NodeBody::DecoderLog`], so nothing here repeats it.
    Readout,
    /// The stored decoder log, filtered to the decoders wired into it.
    DecoderLog,
    DmrTrunk(DmrTrunkNode),
    /// The raster a video channel scans out.
    Video,
    /// SigMF recording of a device's IQ.
    Recorder,
    /// CSV/JSON export of the stored decoder log.
    Export,
    Scanner,
}

impl NodeBody {
    /// Stable slug, matching the serde tag.
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
            Self::Readout => "readout",
            Self::DecoderLog => "decoder_log",
            Self::DmrTrunk(_) => "dmr_trunk",
            Self::Video => "video",
            Self::Recorder => "recorder",
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
            | Self::Readout
            | Self::DecoderLog
            | Self::Video => NodeCategory::Display,
            Self::Scanner | Self::DmrTrunk(_) => NodeCategory::Feature,
            Self::Speaker | Self::Recorder | Self::Export => NodeCategory::Sink,
        }
    }

    /// Every port this kind can have, conditions included. A node's real port list is this
    /// filtered by [`PortSpec::applies_to`] against what backs it — a channel's descriptor, a
    /// device's capabilities.
    #[must_use]
    pub fn ports(&self) -> Vec<PortSpec> {
        ports_for(self.kind())
    }

    /// The real port list of a node with `backing` behind it: conditions resolved and every
    /// repeating spec expanded to one port per stream, named by [`stream_port`]. Expanded ports
    /// are concrete sockets, so they carry [`PortRepeat::Once`] — leaving the flag on would
    /// invite a second expansion.
    ///
    /// With no backing a repeating port keeps stream 0 only: how many streams a radio delivers
    /// is known only while it is attached, and a port drawn on a guess is one the operator can
    /// be told to use and then refused.
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

/// The port table, keyed by node-kind slug so the catalog and a stored node answer from the same
/// place.
fn ports_for(kind: &str) -> Vec<PortSpec> {
    use PortCondition::{
        Always, ChannelHasAudio, ChannelHasVideo, ChannelIsDecoder, ChannelNeedsPosition,
        DeviceIsTxCapable,
    };
    use PortDirection::{In, Out};
    use PortType::{Audio, Baseband, Control, Events, Iq, Position, Tx, Video};
    match kind {
        // A radio's left side is what is done *to* it, and its right side is what comes off it.
        // Both inputs take one wire: one sweep owns the tuning, one baseband keys the transmitter.
        //
        // The transmit input is drawn only on a radio that has a send side: a receiver has no
        // socket to key, and a port that could never do anything on the commonest SDR there is
        // reads as a broken node rather than as a reservation.
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
        // The baseband output is the channel's own passband — what the demodulator is looking
        // at, not what the radio handed the channel. A scope on it sees what a decoder sees.
        "channel" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, ChannelNeedsPosition),
            PortSpec::new(Baseband, Out, true, Always),
            PortSpec::new(Audio, Out, true, ChannelHasAudio),
            PortSpec::new(Events, Out, true, ChannelIsDecoder),
            PortSpec::new(Video, Out, true, ChannelHasVideo),
        ],
        // Two inputs, one instrument: a radio's wideband stream or one channel's passband. Both
        // may be wired at once and the face reads the baseband, which is the narrower answer.
        "scope" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Baseband, In, false, Always),
        ],
        "recorder" => vec![
            PortSpec::new(Iq, In, false, Always),
            PortSpec::new(Position, In, false, Always),
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
        "readout" | "decoder_log" | "export" => {
            vec![PortSpec::new(Events, In, true, Always)]
        }
        "dmr_trunk" => vec![
            PortSpec::new(Events, In, true, Always),
            PortSpec::new(Events, Out, true, Always),
        ],
        _ => Vec::new(),
    }
}

/// One entry of the node palette the client renders its "add node" menu from (: the client
/// renders what the server describes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct NodeTypeInfo {
    /// Slug matching [`NodeBody::kind`].
    pub kind: String,
    pub name: String,
    pub category: NodeCategory,
    pub ports: Vec<PortSpec>,
    /// Channel nodes need a type from `GET /api/channeltypes`; the menu offers one entry per
    /// descriptor rather than one entry for "channel".
    #[serde(default)]
    pub needs_channel_type: bool,
}

/// `GET /api/patch/catalog` — the node palette and its ports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PatchCatalog {
    pub nodes: Vec<NodeTypeInfo>,
}

impl PatchCatalog {
    /// The catalog this build offers, in the order the palette lists it.
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
                entry(&NodeBody::Readout, "Readout"),
                entry(&NodeBody::DecoderLog, "Decoder log"),
                entry(
                    &NodeBody::DmrTrunk(DmrTrunkNode::default()),
                    "DMR trunk system",
                ),
                entry(&NodeBody::Video, "Video"),
                entry(&NodeBody::Recorder, "Recorder"),
                entry(&NodeBody::Export, "Export"),
                entry(&NodeBody::Scanner, "Scanner"),
            ],
        }
    }
}

/// Canvas position of a node, in React Flow's coordinate space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Position {
    pub x: f32,
    pub y: f32,
}

/// Face size in canvas units. Absent means the node's natural size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// One node: what it is, where it sits, and what the operator called it.
///
/// There is no `pinned` flag: rack membership is the single truth for "this face is being
/// operated", and two representations of one fact drift.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PatchNode {
    /// Client-generated, unique within the graph, stable for the node's life.
    pub id: String,
    #[serde(flatten)]
    pub body: NodeBody,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<Size>,
    /// User-renamed caption; `None` renders the kind's default name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// One end of an edge.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct PortRef {
    pub node: String,
    /// [`PortSpec::name`] on that node.
    pub port: String,
}

/// A wire: which stream a node consumes, and from whom.
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

/// One pinned face on the rack grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RackCell {
    /// Whole grid cells from the left / top.
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// One pinned node and the cells it occupies.
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
    SelfEdge(String),
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
            Self::SelfEdge(id) => write!(f, "node {id} cannot wire to itself"),
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

    /// Device nodes in stored order — the order every binding pass walks, so which node claims a
    /// duplicate-serial clone is stable across runs.
    pub fn device_nodes(&self) -> impl Iterator<Item = &PatchNode> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.body, NodeBody::Device(_)))
    }

    /// Nodes wired into `node`'s `port` input, in stored order.
    pub fn sources_of<'a>(&'a self, node: &'a str, port: &'a str) -> impl Iterator<Item = &'a str> {
        self.edges
            .iter()
            .filter(move |edge| edge.to.node == node && edge.to.port == port)
            .map(|edge| edge.from.node.as_str())
    }

    /// Nodes fed by `node`'s `port` output, in stored order.
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
            // The stream is what the wire says, not the string "iq": the device end of the edge
            // may name any port of its rx family.
            let stream = self.edges.iter().find_map(|edge| {
                (edge.to.node == node.id && edge.to.port == "iq" && edge.from.node == device_node)
                    .then(|| port_stream("iq", &edge.from.port))
                    .flatten()
            })?;
            Some((node, stream))
        })
    }

    /// Structural validity: ids, geometry, and wires that name real ports of compatible type.
    /// Semantics that need the running build's channel registry are
    /// [`Self::validate_against`].
    pub fn validate(&self) -> Result<(), PatchError> {
        self.check(None)
    }

    /// Structural validity plus the checks that need the channel registry: a channel node names
    /// a type this build has, and a conditional port exists only on a type that produces it.
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
                    if let Some(descriptors) = channels
                        && !descriptors
                            .iter()
                            .any(|d| d.type_id == channel.channel_type)
                    {
                        return Err(PatchError::ChannelType(channel.channel_type.clone()));
                    }
                }
                NodeBody::Gps(gps) => validate_gps_source(&gps.source)?,
                NodeBody::SignalMap(settings) => {
                    if settings.offset_hz.unsigned_abs() > MAX_SIGNAL_MAP_OFFSET_HZ as u64
                        || !(1..=MAX_SIGNAL_MAP_BANDWIDTH_HZ).contains(&settings.bandwidth_hz)
                    {
                        return Err(PatchError::NodeSettings(node.id.clone()));
                    }
                }
                NodeBody::DmrTrunk(settings) => {
                    if settings.retention_seconds != 0
                        && !(10..=86_400).contains(&settings.retention_seconds)
                    {
                        return Err(PatchError::NodeSettings(node.id.clone()));
                    }
                    let only_dmr = self.sources_of(&node.id, "events").all(|source| {
                        self.node(source).is_some_and(|source| {
                            matches!(
                                &source.body,
                                NodeBody::Channel(channel) if channel.channel_type == "dmr"
                            )
                        })
                    });
                    if !only_dmr {
                        return Err(PatchError::NodeSettings(node.id.clone()));
                    }
                }
                _ => {}
            }
        }
        self.check_edges(channels)?;
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
            // An output says its arity too: a stream fans out, ownership does not, and a scanner
            // sweeping two radios at once is a workspace the engine cannot run.
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
        // A repeating port admits its whole bounded family, backing or not: validation is pure
        // over the stored graph and runs on every write, so refusing a stored `iq3` here would
        // refuse every later write — a node drag included. [`MAX_STREAMS`] is what keeps an
        // arbitrary name `UnknownPort`.
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
        // A conditional port is only real on a type that produces it: wiring an ADS-B channel's
        // audio out would otherwise be a wire the engine has no stream for.
        //
        // A device node's one conditional port is not checked here and cannot be: whether the
        // radio it names can transmit is known only while that radio is attached, and validation
        // is pure over the stored graph. Nothing is let through by the gap — no port emits
        // [`PortType::Tx`], so an edge into a transmit input dies on the type mismatch first
        // (`the_reserved_transmit_input_can_take_no_wire`). The day something does emit it, the
        // check moves to where the capabilities are: the server's `bring_up`.
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
    /// Every slot names a node of `graph`, is inside the grid, and overlaps nothing.
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
    /// The documented defaults for a channel type id, as `{"type": id, "settings": {}}`
    /// deserializes them. One source for the mapping: the enum's own serde tag, so a type this
    /// build does not have answers `None` instead of a guess.
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
        device::{Duplex, StreamScope},
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

    /// Only the transmit flag and the stream counts matter to the port table; the rest of a
    /// radio's report does not.
    fn capabilities(duplex: Duplex, rx_streams: u32, tx_streams: u32) -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams,
            tx_streams,
            per_stream: StreamScope::default(),
            directional: None,
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
        ]
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

    /// The tags the generated TS union switches on.
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
    fn a_dmr_trunk_system_accepts_only_dmr_carrier_events() {
        let system = node("system", NodeBody::DmrTrunk(DmrTrunkNode::default()));
        let dmr = PatchGraph {
            nodes: vec![channel("carrier", "dmr"), system.clone()],
            edges: vec![edge(("carrier", "events"), ("system", "events"))],
        };
        dmr.validate().expect("DMR carrier");

        let other = PatchGraph {
            nodes: vec![channel("carrier", "nfm"), system],
            edges: vec![edge(("carrier", "events"), ("system", "events"))],
        };
        assert_eq!(
            other.validate(),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
    }

    #[test]
    fn dmr_trunk_call_retention_can_be_off() {
        let graph = PatchGraph {
            nodes: vec![node(
                "system",
                NodeBody::DmrTrunk(DmrTrunkNode {
                    retention_seconds: 0,
                    ..DmrTrunkNode::default()
                }),
            )],
            edges: Vec::new(),
        };
        graph.validate().expect("retention off");

        let mut invalid = graph;
        let NodeBody::DmrTrunk(settings) = &mut invalid.nodes[0].body else {
            panic!("DMR trunk node");
        };
        settings.retention_seconds = 1;
        assert_eq!(
            invalid.validate(),
            Err(PatchError::NodeSettings("system".to_owned()))
        );
    }

    #[test]
    fn dmr_trunk_wires_use_the_same_name_at_both_ends() {
        let graph = PatchGraph {
            nodes: vec![
                channel("carrier", "dmr"),
                node("system", NodeBody::DmrTrunk(DmrTrunkNode::default())),
                node("log", NodeBody::DecoderLog),
            ],
            edges: vec![
                edge(("carrier", "events"), ("system", "events")),
                edge(("system", "events"), ("log", "events")),
            ],
        };
        graph.validate().expect("matching wire names");
    }

    /// An ADS-B channel has no audio, so the port it would be wired by does not exist on it —
    /// the wire is refused where the operator drew it, not at stream time.
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
    fn the_only_type_level_cycle_is_the_guarded_dmr_transform() {
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
        assert_eq!(cycle, vec!["dmr_trunk"]);
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

        // The nearest thing to a transmit source is another radio's IQ, and the types do not join.
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

    /// A receiver has no send side to draw. The reservation is per *radio*, not per node kind: an
    /// An RTL-SDR is receive-only, so the node standing for one has two ports, and the
    /// operator is never shown a socket their hardware does not have.
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
        // Which radio is behind the node is stored; what it can do is not — so an unattached one
        // is a receiver until it says otherwise.
        assert!(!transmit.applies_to(None), "no radio bound");

        for port in ports_for("device")
            .into_iter()
            .filter(|port| port.port_type != PortType::Tx)
        {
            assert!(port.applies_to(None), "{} is not conditional", port.name);
        }
    }

    /// The scanner wire runs *into* the radio it drives, and ownership is exclusive at both ends:
    /// the engine runs one sweep per device set, and a set answers to one sweep.
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
            label: "RSPduo Dual Tuner".to_owned(),
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

    /// Binding follows the wire, not the string: a channel on `dev.iq3` belongs to that device
    /// and that stream, or its engine channel would leak on delete and swap settings on bind.
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
        // Stream 0 is spelled "iq": one spelling per port, or arity would split across aliases.
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

    /// The catalog stays static; the expansion happens wherever the stream counts are known.
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

        // A channel's ports never repeat; its backing only resolves conditions.
        let body = NodeBody::Channel(ChannelNode {
            channel_type: "nfm".to_owned(),
        });
        let nfm = &descriptors()[0];
        let names: Vec<String> = body
            .ports_with(Some(PortBacking::Channel(nfm)))
            .into_iter()
            .map(|port| port.name)
            .collect();
        // Baseband is unconditional: every channel has a passband, whatever it does with it.
        assert_eq!(names, vec!["iq", "baseband", "audio"]);
    }

    /// The three *demodulated* things a channel's right side can carry are each conditional, and
    /// a face draws only the ones its type actually produces: an NFM channel has no picture to
    /// send anywhere, and a picture port on it is a socket the operator can be told to use and
    /// then refused. The baseband tap is not one of them — it is the channel's input, and it
    /// exists whether or not anything is demodulated from it.
    #[test]
    fn a_channels_outputs_follow_what_its_type_produces() {
        let names = |descriptor: &ChannelDescriptor| {
            NodeBody::Channel(ChannelNode {
                channel_type: descriptor.type_id.clone(),
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

        // And the type is what joins them: a picture cannot be poured into a readout.
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

    /// The whole reason the channel tap is not typed `Iq`: a wideband stream and one channel's
    /// passband are not interchangeable, and typing them the same would let an operator wire a
    /// channel into a channel — a pipeline the engine has no way to build.
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
        assert!(!takes("signal_map", PortType::Baseband));
        assert!(
            takes("scope", PortType::Baseband),
            "the scope is what reads it"
        );

        // And a wire that tries it anyway is refused, rather than left to convention.
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

        // The family exists only where the table repeats: a channel consumes one stream, so
        // its input has no siblings.
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

    /// The catalog is what the client builds its palette and its drag-time rules from, so its
    /// shape is a contract.
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

        // A catalog from a peer that predates `repeat` reads every port as `Once`; the roundtrip
        // pins that the field is the only thing the elision drops.
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

    /// A channel node names a type; the engine is asked for a channel at that type's documented
    /// defaults, which is the same body the client sends when it adds one by hand.
    #[test]
    fn default_params_come_from_the_type_id() {
        let params = ChannelParams::default_for("ssb").expect("ssb is a channel type");
        assert_eq!(params.type_id(), "ssb");
        assert_eq!(
            ChannelSettings {
                offset_hz: 0.0,
                squelch_db: None,
                params,
            },
            serde_json::from_str(r#"{"params":{"type":"ssb","settings":{}}}"#).unwrap()
        );
        assert_eq!(ChannelParams::default_for("wefax"), None);
    }
}
