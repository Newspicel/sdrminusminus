use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{decode::DecodedRecord, position::PositionFix};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum StateScope {
    All,
    Devices,
    DeviceSet(u32),
    Presets,
    Bookmarks,
    Recordings,
    Clients,
    DecoderLog,
    Calls,
    Images,
    Workspaces,
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", content = "data")]
pub enum ServerEvent {
    PipelineHealth {
        queues: Vec<crate::PipelineQueue>,
        websocket: crate::QueueHealth,
    },
    Hello {
        revision: u64,
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
    PositionChanged {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    SurfaceStreamStarted {
        stream_id: u16,
        device_set: u32,
        node: String,
    },
    DfUpdate {
        device_set: u32,
        node: String,
        reading: Box<crate::coherent::DfReading>,
        cal: Box<crate::coherent::CalState>,
    },
    DfFusionUpdate {
        node: String,
        state: Box<crate::coherent::DfFusionState>,
    },
    RadarDetections {
        device_set: u32,
        node: String,
        detections: Vec<crate::coherent::RadarDetection>,
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
    },
    UnsubscribeAudio {
        device_set: u32,
        channel: u32,
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
    PublishPosition {
        node: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fix: Option<PositionFix>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    SubscribeSurface {
        node: String,
    },
    UnsubscribeSurface {
        node: String,
    },
}
