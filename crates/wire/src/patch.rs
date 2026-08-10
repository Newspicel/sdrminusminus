//! The patch graph — the station drawn as nodes and wires (CANVAS §1, §4).
//!
//! A workspace stores a [`PatchGraph`] and a [`RackLayout`]: every radio, demodulator, decoder,
//! map and sink is a node, an edge names which existing stream (PLAN §5) a node consumes, and the
//! rack holds the faces being operated. This is *our* model, never the canvas library's
//! serialization — templates author stations in Rust and a React Flow major must not invalidate a
//! stored workspace (CANVAS §4, the same rule the M6 layout tree followed).
//!
//! The graph is control plane only (CANVAS §2). It is a description the server validates and
//! applies through the existing command queue; no wire is a data path in itself and the DSP plane
//! (PLAN §7) never sees it.
//!
//! Two things it deliberately does *not* store. Settings: a channel node names its *type*, and
//! the live settings stay where they already live (`ChannelSettings` on the engine's channel), so
//! turning a squelch knob is not a workspace write. And bindings: which engine device set or
//! channel a node is currently driving is recomputed per run from durable identity, because
//! engine ids are allocated per run and reused (PLAN §18, the M6 rule).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::{ChannelDescriptor, ChannelParams},
    device::{Capabilities, DeviceInfo},
    workspace::MAX_NAME_LEN,
};

/// Structural caps. A graph is stored whole in one row and rewritten on every arrangement
/// gesture, so the bounds are what keeps one row from growing without limit; the numbers are far
/// above any station a person would draw.
pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 256;
pub const MAX_NODE_ID_LEN: usize = 64;
/// Canvas coordinates are unbounded in React Flow; this is the box a stored position must sit in
/// so a corrupt write cannot park a node where no camera will find it.
pub const MAX_COORD: f32 = 100_000.0;
pub const MAX_NODE_SIZE: f32 = 10_000.0;
/// Rack grid. Cells are whole units of a fixed grid (CANVAS §5: alignment and muscle memory, no
/// camera), so a rack that does not fit is a smaller rack, never a zoomed one.
///
/// Twelve by eight, not the twenty-four squared it shipped as: the cell is the unit of every
/// gesture, and a cell too small to aim at is a drag that lands one short. CANVAS §5 already
/// named the remedy for a rack that feels cramped — bigger cells. A rack stored against the old
/// grid is re-laid out client-side (`pruneRack`) rather than migrated: the slots are an
/// arrangement, not data.
pub const RACK_COLS: u16 = 12;
pub const RACK_ROWS: u16 = 8;

/// What a wire carries. Hue encodes this and only this (`DESIGN.md` §2), so the set stays small
/// and every member is something the engine actually moves today — with one named exception,
/// [`PortType::Tx`], which is reserved and unwireable until transmit exists (PLAN §12a).
///
/// `iq-tap` (decimated channel IQ) and `position` (GPS) stay absent for the reason that exception
/// does *not* apply to them: the channel analyzer is PLAN §13 Phase 2 and the GPS source Phase 4,
/// so a port for either would be a wire that dangles with nothing reserving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// Wideband complex baseband at the device rate.
    Iq,
    /// 48 kHz demodulated audio (Opus on the wire, PLAN §9).
    Audio,
    /// Typed decoder frames ([`crate::DecodedRecord`]).
    Events,
    /// Tuning ownership, not a stream: a scanner sweeps the radio it is wired into, and client
    /// retunes on that radio are refused while it does (PLAN §18). The wire *is* the ownership,
    /// which is what makes "which radio has this sweep taken over" a thing you can see.
    Control,
    /// Complex baseband to be transmitted at the device rate.
    ///
    /// **Reserved, and inert by construction.** No node kind in this build emits it, so no edge
    /// into a transmit input can validate — the port is the shape transmit will arrive in, not a
    /// path to it. PLAN §12a owns what has to exist first: the authorized-use gate.
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
            Self::Audio => "audio",
            Self::Events => "events",
            Self::Control => "control",
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
    /// Only on a radio that can transmit ([`Capabilities::tx_capable`]). Unlike the channel
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
    /// Why this port refuses everything, for the ports that do. The client renders what the
    /// server describes (PLAN §2), and a port with no wire and no explanation reads as broken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn is_always(condition: &PortCondition) -> bool {
    *condition == PortCondition::Always
}

impl PortSpec {
    fn new(
        name: &str,
        port_type: PortType,
        direction: PortDirection,
        multi: bool,
        condition: PortCondition,
    ) -> Self {
        Self {
            name: name.to_owned(),
            port_type,
            direction,
            multi,
            condition,
            note: None,
        }
    }

    #[must_use]
    fn noted(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
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
            (PortCondition::DeviceIsTxCapable, Some(PortBacking::Device(device))) => {
                device.tx_capable
            }
            _ => false,
        }
    }
}

/// The band a node's header strip carries (CANVAS §6): what the operator is looking at before
/// they read the label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeCategory {
    Source,
    Channel,
    Display,
    Feature,
    Sink,
}

/// A device named by durable identity (CANVAS §3), never by an engine or probe id: those are
/// allocated per run and reused, so a stored engine id would silently bind a node to whichever
/// radio opened first — the kind of failure that looks like a working panel.
///
/// `key` is the tie-break CANVAS §3 does not name, added because a backend can have several
/// devices and no serials: the virtual backend's key is `siggen` or the stem of a recording, both
/// durable, and without it a patch could not say *which* capture it plays. It is consulted only
/// when there is no serial, which is what keeps it away from the case it would be wrong for — an
/// RTL-SDR clone whose key is a bus index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceRef {
    /// Driver id, matching [`DeviceInfo::driver`]: `"rtlsdr"`, `"hackrf"`, `"soapy"`, `"virtual"`.
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// Per-driver key, used only when the driver exposes no serial.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl DeviceRef {
    /// The reference that names this discovered device.
    #[must_use]
    pub fn from_info(info: &DeviceInfo) -> Self {
        Self {
            backend: info.driver.clone(),
            serial: info.serial.clone(),
            key: info.serial.is_none().then(|| info.key.clone()),
        }
    }

    /// Whether `info` is the device this reference names. Serial wins when the driver exposes
    /// one; otherwise the key does; a backend with a single serial-less device matches on the
    /// backend alone, which is what makes `{backend, serial: none}` unambiguous for a singleton.
    #[must_use]
    pub fn matches(&self, info: &DeviceInfo) -> bool {
        if self.backend != info.driver {
            return false;
        }
        match (&self.serial, &info.serial) {
            (Some(want), Some(have)) => want == have,
            (Some(_), None) => false,
            (None, _) => match &self.key {
                Some(key) => *key == info.key,
                None => true,
            },
        }
    }
}

/// A device node's payload: the radio it names, or nothing yet.
///
/// Unbound is a first-class state, not an error — it is the empty node a fresh station starts on,
/// and it renders the device picker. Bound-but-absent is the other one: controls disabled,
/// wires kept, never silently rebound (CANVAS §3).
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

/// What a node is. Adjacently tagged like [`crate::ChannelParams`], so the generated TypeScript
/// is a union the client can exhaustively switch on.
///
/// The catalog is deliberately shorter than CANVAS §1's table: the GPS source, the UDP sink and
/// the WAV audio-file sink need server features that do not exist (PLAN §13 Phase 2/4), and a
/// node whose backend is unbuilt is a face that can only apologise.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum NodeBody {
    Device(DeviceNode),
    Channel(ChannelNode),
    /// Spectrum + waterfall over a device's IQ.
    Scope,
    /// Client-side audio mix (PLAN §9: the server ships streams, not a mix).
    Speaker,
    /// MapLibre, one layer per connected decoder.
    Map,
    /// The stored decoder log, filtered to the decoders wired into it.
    DecoderLog,
    /// SigMF recording of a device's IQ.
    Recorder,
    /// CSV/JSON export of the stored decoder log.
    Export,
    /// Frequency scanner. Its edge runs *into* the device it drives, because it is ownership and
    /// not consumption: a running scan owns that set's centre frequency and client retunes are
    /// refused while it does (PLAN §18).
    Scanner,
}

impl NodeBody {
    /// Stable slug, matching the serde tag.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Device(_) => "device",
            Self::Channel(_) => "channel",
            Self::Scope => "scope",
            Self::Speaker => "speaker",
            Self::Map => "map",
            Self::DecoderLog => "decoder_log",
            Self::Recorder => "recorder",
            Self::Export => "export",
            Self::Scanner => "scanner",
        }
    }

    #[must_use]
    pub const fn category(&self) -> NodeCategory {
        match self {
            Self::Device(_) => NodeCategory::Source,
            Self::Channel(_) => NodeCategory::Channel,
            Self::Scope | Self::Map | Self::DecoderLog => NodeCategory::Display,
            Self::Scanner => NodeCategory::Feature,
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
}

/// The port table, keyed by node-kind slug so the catalog and a stored node answer from the same
/// place.
fn ports_for(kind: &str) -> Vec<PortSpec> {
    use PortCondition::{Always, ChannelHasAudio, ChannelIsDecoder, DeviceIsTxCapable};
    use PortDirection::{In, Out};
    use PortType::{Audio, Control, Events, Iq, Tx};
    match kind {
        // A radio's left side is what is done *to* it, and its right side is what comes off it.
        // Both inputs take one wire: one sweep owns the tuning, one baseband keys the transmitter.
        //
        // The transmit input is drawn only on a radio that has a send side: a receiver has no
        // socket to key, and a port that could never do anything on the commonest SDR there is
        // reads as a broken node rather than as a reservation.
        "device" => vec![
            PortSpec::new("control", Control, In, false, Always),
            PortSpec::new("tx", Tx, In, false, DeviceIsTxCapable).noted(
                "reserved: transmit is not built (PLAN §12a), so nothing in this build emits \
                 a signal to key a radio with",
            ),
            PortSpec::new("iq", Iq, Out, true, Always),
        ],
        "channel" => vec![
            PortSpec::new("iq", Iq, In, false, Always),
            PortSpec::new("audio", Audio, Out, true, ChannelHasAudio),
            PortSpec::new("events", Events, Out, true, ChannelIsDecoder),
        ],
        "scope" | "recorder" => vec![PortSpec::new("iq", Iq, In, false, Always)],
        "scanner" => vec![PortSpec::new("control", Control, Out, false, Always)],
        "speaker" => vec![PortSpec::new("audio", Audio, In, true, Always)],
        "map" | "decoder_log" | "export" => vec![PortSpec::new("events", Events, In, true, Always)],
        _ => Vec::new(),
    }
}

/// One entry of the node palette the client renders its "add node" menu from (PLAN §2: the client
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
                entry(
                    &NodeBody::Channel(ChannelNode {
                        channel_type: String::new(),
                    }),
                    "Channel",
                ),
                entry(&NodeBody::Scope, "Scope"),
                entry(&NodeBody::Speaker, "Speaker"),
                entry(&NodeBody::Map, "Map"),
                entry(&NodeBody::DecoderLog, "Decoder log"),
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

/// The station as a graph (CANVAS §1).
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

/// The operate view: faces on a snapping grid, no pan, no zoom, no wires (CANVAS §5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RackLayout {
    #[serde(default)]
    pub slots: Vec<RackSlot>,
}

/// Why a graph was refused. Structural and semantic checks both land here so the server has one
/// rejection point; `Display` is written out rather than derived because this crate carries no
/// error-derive dependency (PLAN §3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PatchError {
    TooManyNodes,
    TooManyEdges,
    NodeId(String),
    DuplicateNode(String),
    Label(String),
    Geometry(String),
    Backend(String),
    ChannelType(String),
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
            Self::ChannelType(ty) => write!(f, "unknown channel type {ty:?}"),
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

    /// Channel nodes taking IQ from `device_node`, in stored order. This order is the binding
    /// order (CANVAS §3): the n-th node of a type binds the n-th engine channel of that type.
    pub fn channels_of<'a>(&'a self, device_node: &'a str) -> impl Iterator<Item = &'a PatchNode> {
        self.nodes.iter().filter(move |node| {
            matches!(node.body, NodeBody::Channel(_))
                && self
                    .sources_of(&node.id, "iq")
                    .any(|src| src == device_node)
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
            // sweeping two radios at once is a station the engine cannot run.
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
        let spec = node
            .body
            .ports()
            .into_iter()
            .find(|port| port.name == reference.port)
            .ok_or_else(|| PatchError::UnknownPort(reference.clone()))?;
        if spec.direction != direction {
            return Err(PatchError::Direction(reference.clone()));
        }
        // A conditional port is only real on a type that produces it: wiring an ADS-B channel's
        // audio out would otherwise be a wire the engine has no stream for.
        //
        // A device node's one conditional port is not checked here and cannot be: whether the
        // radio it names can transmit is known only while that radio is attached, and validation
        // is pure over the stored graph. Nothing is let through by the gap — no port emits
        // [`PortType::Tx`], so an edge into a transmit input dies on the type mismatch first
        // (`the_reserved_transmit_input_can_take_no_wire`). The day something does emit it, the
        // check moves to where the capabilities are: `apply_station`.
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
    use crate::channel::ChannelSettings;

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

    /// Only the transmit flag matters to the port table; the rest of a radio's report does not.
    fn capabilities(tx_capable: bool) -> Capabilities {
        Capabilities {
            freq_ranges: Vec::new(),
            sample_rates: Vec::new(),
            sample_rate_range: None,
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            extra: Vec::new(),
            tx_capable,
        }
    }

    fn descriptors() -> Vec<ChannelDescriptor> {
        vec![
            ChannelDescriptor {
                type_id: "nfm".to_owned(),
                name: "NFM".to_owned(),
                bandwidth_hz: 12_500.0,
                input_rate_hz: 48_000.0,
                has_audio: true,
                decoder_kind: None,
                exact_rate_only: false,
                native_rate_max_hz: None,
            },
            ChannelDescriptor {
                type_id: "adsb".to_owned(),
                name: "ADS-B".to_owned(),
                bandwidth_hz: 2_000_000.0,
                input_rate_hz: 2_000_000.0,
                has_audio: false,
                decoder_kind: Some("adsb".to_owned()),
                exact_rate_only: false,
                native_rate_max_hz: Some(4_000_000.0),
            },
        ]
    }

    fn station() -> PatchGraph {
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
    fn a_station_validates_structurally_and_against_the_registry() {
        let graph = station();
        graph.validate().expect("structurally valid");
        graph
            .validate_against(&descriptors())
            .expect("valid against the registry");
    }

    #[test]
    fn an_unknown_channel_type_is_refused_only_against_the_registry() {
        let mut graph = station();
        graph.nodes[2] = channel("ch", "wefax");
        graph.validate().expect("structure alone cannot know");
        assert_eq!(
            graph.validate_against(&descriptors()),
            Err(PatchError::ChannelType("wefax".to_owned()))
        );
    }

    /// An ADS-B channel has no audio, so the port it would be wired by does not exist on it —
    /// the wire is refused where the operator drew it, not at stream time.
    #[test]
    fn a_conditional_port_is_refused_on_a_type_that_lacks_it() {
        let mut graph = station();
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
        let mut wrong_type = station();
        wrong_type.edges.push(edge(("dev", "iq"), ("spk", "audio")));
        assert_eq!(
            wrong_type.validate(),
            Err(PatchError::TypeMismatch {
                from: PortType::Iq,
                to: PortType::Audio
            })
        );

        let mut backwards = station();
        backwards.edges = vec![edge(("scope", "iq"), ("dev", "iq"))];
        assert_eq!(
            backwards.validate(),
            Err(PatchError::Direction(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut unknown = station();
        unknown.edges = vec![edge(("dev", "iq"), ("scope", "tap"))];
        assert_eq!(
            unknown.validate(),
            Err(PatchError::UnknownPort(PortRef {
                node: "scope".to_owned(),
                port: "tap".to_owned()
            }))
        );

        let mut missing = station();
        missing.edges = vec![edge(("ghost", "iq"), ("scope", "iq"))];
        assert_eq!(
            missing.validate(),
            Err(PatchError::UnknownNode("ghost".to_owned()))
        );
    }

    /// A channel has exactly one IQ input: two devices into one channel is refused until
    /// `CoherentArray` exists (CANVAS §1, PLAN §6).
    #[test]
    fn a_single_input_takes_one_wire_and_an_output_fans_out() {
        let mut two_devices = station();
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

        // The same device feeding a scope, a channel and a recorder is the point of the model.
        let mut fanned = station();
        fanned.nodes.push(node("rec", NodeBody::Recorder));
        fanned.edges.push(edge(("dev", "iq"), ("rec", "iq")));
        fanned.validate().expect("iq fans out");
    }

    #[test]
    fn duplicate_wires_and_self_wires_are_refused() {
        let mut duplicate = station();
        duplicate.edges.push(edge(("dev", "iq"), ("scope", "iq")));
        assert_eq!(
            duplicate.validate(),
            Err(PatchError::DuplicateEdge(PortRef {
                node: "scope".to_owned(),
                port: "iq".to_owned()
            }))
        );

        let mut loop_back = station();
        loop_back.edges = vec![edge(("ch", "audio"), ("ch", "iq"))];
        assert_eq!(
            loop_back.validate(),
            Err(PatchError::SelfEdge("ch".to_owned()))
        );
    }

    /// No station can be drawn that feeds itself. The proof is over the *kinds*: "some output of
    /// kind A can reach some input of kind B" is read off the port table, and that graph — self
    /// edges included, since one would mean two nodes of a kind could feed each other — has to be
    /// acyclic. Stronger than the per-kind assertions it replaces, and it does not have to be
    /// rewritten each time a kind grows a port.
    ///
    /// It stops being the whole proof the day something emits [`PortType::Tx`]: bench loopback is
    /// a named use of PLAN §12a and is device → channel → modulator → device by design, so cycle
    /// checking moves to the instance level then. That nothing emits it today is pinned below.
    #[test]
    fn the_port_table_admits_no_cycle() {
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
        let edges: Vec<(usize, usize)> = (0..catalog.nodes.len())
            .flat_map(|a| (0..catalog.nodes.len()).map(move |b| (a, b)))
            .filter(|&(a, b)| reaches(&catalog.nodes[a], &catalog.nodes[b]))
            .collect();
        // Kahn: drop any kind nothing still open can reach, until nothing more can be dropped.
        let mut open: Vec<usize> = (0..catalog.nodes.len()).collect();
        while let Some(at) = open
            .iter()
            .position(|&kind| !edges.iter().any(|&(a, b)| b == kind && open.contains(&a)))
        {
            open.remove(at);
        }
        let cycle: Vec<&str> = open
            .iter()
            .map(|&k| catalog.nodes[k].kind.as_str())
            .collect();
        assert!(
            cycle.is_empty(),
            "these kinds can feed each other: {cycle:?}"
        );
    }

    /// The transmit gate at this layer (PLAN §12a). The device node reserves the input transmit
    /// will arrive on, and *nothing in this build emits that type* — so no edge into it validates,
    /// and the reservation cannot quietly become a path to a keyed radio.
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
            "nothing may emit transmit baseband before PLAN §12a's gate exists"
        );

        // The nearest thing to a transmit source is another radio's IQ, and the types do not join.
        let mut retransmit = station();
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
    /// RTL-SDR reports `tx_capable: false`, so the node standing for one has two ports, and the
    /// operator is never shown a socket their hardware does not have.
    #[test]
    fn only_a_radio_that_can_transmit_shows_a_transmit_input() {
        let transmit = ports_for("device")
            .into_iter()
            .find(|port| port.port_type == PortType::Tx)
            .expect("the device kind can have a transmit input");

        let named =
            |port: &PortSpec, caps: &Capabilities| port.applies_to(Some(PortBacking::Device(caps)));
        assert!(!named(&transmit, &capabilities(false)), "a receiver");
        assert!(named(&transmit, &capabilities(true)), "a transceiver");
        // Which radio is behind the node is stored; what it can do is not — so an unattached one
        // is a receiver until it says otherwise.
        assert!(!transmit.applies_to(None), "no radio bound");

        // Its neighbours are unconditional, or a radio out of reach would lose its IQ with it.
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
        let mut driven = station();
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
        let mut duplicate = station();
        duplicate.nodes.push(node("dev", NodeBody::Scope));
        assert_eq!(
            duplicate.validate(),
            Err(PatchError::DuplicateNode("dev".to_owned()))
        );

        let mut empty_id = station();
        empty_id.nodes[1].id = String::new();
        assert!(matches!(empty_id.validate(), Err(PatchError::NodeId(_))));

        let mut far_away = station();
        far_away.nodes[1].position.x = f32::INFINITY;
        assert_eq!(
            far_away.validate(),
            Err(PatchError::Geometry("scope".to_owned()))
        );

        let mut flat = station();
        flat.nodes[1].size = Some(Size { w: 0.0, h: 100.0 });
        assert_eq!(
            flat.validate(),
            Err(PatchError::Geometry("scope".to_owned()))
        );

        let mut long_label = station();
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
        };
        let file = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "file:/rec/capture".to_owned(),
            label: "capture".to_owned(),
            serial: None,
        };
        let siggen = DeviceInfo {
            driver: "virtual".to_owned(),
            key: "siggen".to_owned(),
            label: "Signal Generator".to_owned(),
            serial: None,
        };

        let by_serial = DeviceRef::from_info(&hardware);
        assert_eq!(by_serial.key, None, "a serial makes the key redundant");
        assert!(by_serial.matches(&hardware));
        assert!(!by_serial.matches(&DeviceInfo {
            key: "1".to_owned(),
            serial: Some("00000002".to_owned()),
            ..hardware.clone()
        }));
        // Same radio on a different USB port: the key moved, the serial did not.
        assert!(by_serial.matches(&DeviceInfo {
            key: "3".to_owned(),
            ..hardware.clone()
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
        }));
        assert!(!singleton.matches(&hardware));
    }

    #[test]
    fn channels_of_walks_the_wires_in_stored_order() {
        let mut graph = station();
        graph.nodes.push(channel("ch2", "adsb"));
        graph.edges.push(edge(("dev", "iq"), ("ch2", "iq")));
        let bound: Vec<&str> = graph.channels_of("dev").map(|n| n.id.as_str()).collect();
        assert_eq!(bound, vec!["ch", "ch2"]);
        assert_eq!(graph.channels_of("scope").count(), 0);
    }

    #[test]
    fn the_rack_is_a_grid_with_no_two_faces_in_one_cell() {
        let graph = station();
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
        assert_eq!(ports[1]["port_type"], "tx");
        assert_eq!(ports[1]["condition"], "device_is_tx_capable");
        assert!(ports[1]["note"].is_string(), "the reserved port says why");
        assert_eq!(ports[2]["port_type"], "iq");
        assert_eq!(ports[2]["direction"], "out");
        assert!(
            ports[2].get("condition").is_none() && ports[2].get("note").is_none(),
            "the common case stays off the wire"
        );
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
