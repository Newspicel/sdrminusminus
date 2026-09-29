use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{decode::DecodedRecord, position::PositionFix};

pub const WS_SUBPROTOCOL: &str = "sdrmm";
pub const WS_BEARER_PROTOCOL_PREFIX: &str = "sdrmm.bearer.";
pub const WS_CLOSE_REVOKED: u16 = 4003;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum StateScope {
    All,
    Devices,
    DeviceSet(u32),
    Presets,
    Bookmarks,
    SavedRadios,
    Recordings,
    Clients,
    DecoderLog,
    Calls,
    Images,
    Workspaces,
    Arrays,
    Phones,
    Missions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Spectrum,
    Audio,
    Video,
    Iq,
    Symbols,
    RangeDoppler,
    SpatialSpectrum,
    Visibility,
    FusionGrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SurfaceFit {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceRefusal {
    NoSurface,
    FitNotPositive,
    NoStreamIds,
}

impl SurfaceRefusal {
    pub const ALL: [Self; 3] = [Self::NoSurface, Self::FitNotPositive, Self::NoStreamIds];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoSurface => "No surface",
            Self::FitNotPositive => "Fit must be positive",
            Self::NoStreamIds => "Too many streams",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    BeastExportStatus(crate::event_output::BeastExportStatus),
    PipelineHealth {
        queues: Vec<crate::PipelineQueue>,
        websocket: crate::QueueHealth,
    },
    Hello {
        revision: u64,
        protocol: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        phone: Option<String>,
    },
    StateChanged {
        scope: StateScope,
    },
    StreamStarted {
        stream_id: u16,
        device_set: u32,
        #[serde(default)]
        stream: u32,
    },
    AudioStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
        #[serde(default)]
        fx: Vec<String>,
    },
    VideoStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    IqStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    SymbolStreamStarted {
        stream_id: u16,
        device_set: u32,
        channel: u32,
    },
    SurfaceStreamStarted {
        stream_id: u16,
        node: String,
        kind: StreamKind,
    },
    SurfaceRefused {
        node: String,
        reason: SurfaceRefusal,
    },
    StreamStopped {
        stream_id: u16,
        kind: StreamKind,
    },
    Decoded(Box<DecodedRecord>),
    DecodedBacklog {
        records: Vec<DecodedRecord>,
    },
    DecodedLost {
        count: u64,
    },
    ImageCaptured(Box<crate::rest::CapturedImage>),
    ChannelLevels {
        device_set: u32,
        levels: Vec<crate::state::ChannelLevel>,
    },
    ScannerUpdate {
        device_set: u32,
        status: Box<crate::scan::ScannerStatus>,
    },
    HuntUpdate {
        device_set: u32,
        status: Box<crate::hunt::HuntStatus>,
    },
    SatelliteUpdate {
        status: Box<crate::satellite::SatelliteStatus>,
    },
    PositionChanged {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    ArrayUpdate {
        status: Box<crate::array::ArrayStatus>,
    },
    ProcessorUpdate {
        node: String,
        reading: Box<crate::processor::ProcessorReading>,
    },
    DfFusionUpdate {
        node: String,
        state: Box<crate::fusion::DfFusionState>,
    },
    SurveyUpdate {
        node: String,
        update: Box<crate::survey::SurveyUpdate>,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ClientCommand {
    SubscribeDiagnostics {
        enabled: bool,
    },
    SubscribeSpectrum {
        device_set: u32,
        fps: u16,
        bins: u16,
        #[serde(default)]
        stream: u32,
    },
    UnsubscribeSpectrum {
        device_set: u32,
        #[serde(default)]
        stream: u32,
    },
    SubscribeAudio {
        device_set: u32,
        channel: u32,
        #[serde(default)]
        fx: Vec<String>,
    },
    UnsubscribeAudio {
        device_set: u32,
        channel: u32,
        #[serde(default)]
        fx: Vec<String>,
    },
    SubscribeVideo {
        device_set: u32,
        channel: u32,
    },
    UnsubscribeVideo {
        device_set: u32,
        channel: u32,
    },
    SubscribeIq {
        device_set: u32,
        channel: u32,
    },
    UnsubscribeIq {
        device_set: u32,
        channel: u32,
    },
    SubscribeSymbols {
        device_set: u32,
        channel: u32,
    },
    UnsubscribeSymbols {
        device_set: u32,
        channel: u32,
    },
    PublishPose {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    SubscribeSurface {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fit: Option<SurfaceFit>,
    },
    UnsubscribeSurface {
        node: String,
    },
}

#[cfg(test)]
mod tests;
