use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{
    channel::{ChannelDescriptor, ChannelSettings},
    decode::{DecoderEvent, DvMode},
    device::{DeviceInfo, DeviceSettings},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventAudio {
    pub url: String,
    pub media_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VoiceCall {
    pub id: u64,
    pub node: String,
    pub source_node: String,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub device_set: u32,
    pub channel: u32,
    pub freq_hz: f64,
    pub mode: DvMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_call: Option<bool>,
    pub encrypted: bool,
    pub emergency: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<EventAudio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VoiceCallsResponse {
    pub calls: Vec<VoiceCall>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EventImage {
    pub url: String,
    pub media_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CapturedImage {
    pub id: u64,
    pub device_set: u32,
    pub channel: u32,
    pub at: String,
    pub freq_hz: f64,
    pub source: String,
    pub mode: String,
    pub width: u16,
    pub height: u16,
    pub lines: u16,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<EventImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CapturedImagesResponse {
    pub images: Vec<CapturedImage>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DevicesResponse {
    pub devices: Vec<DeviceInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateDeviceSetRequest {
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateChannelRequest {
    #[serde(default)]
    pub stream: u32,
    pub settings: ChannelSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelTypesResponse {
    pub types: Vec<ChannelDescriptor>,
}

pub const PRESET_SNAPSHOT_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetSnapshot {
    pub version: u32,
    #[serde(default)]
    pub devices: Vec<PresetDevice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetDevice {
    pub node: String,
    pub device_id: String,
    pub settings: DeviceSettings,
    pub channels: Vec<ChannelSettings>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PresetInfo {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub devices: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatePresetRequest {
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Bookmark {
    pub id: i64,
    pub label: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreateBookmarkRequest {
    pub label: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordRequest {
    pub action: RecordAction,
    #[serde(default)]
    pub stream: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordAction {
    Start,
    Stop,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingInfo {
    pub id: i64,
    pub file: String,
    pub device_id: String,
    pub device_label: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub samples: u64,
    pub bytes: u64,
    pub duration_s: f64,
    pub created_at: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub const MAX_RECORDING_TAGS: usize = 32;
pub const MAX_RECORDING_TAG_LEN: usize = 48;
pub const MAX_RECORDING_NOTE_LEN: usize = 4_000;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecordingAnnotation {
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnnotationError {
    TagCount(usize),
    TagLen(usize),
    NoteLen(usize),
}

impl std::fmt::Display for AnnotationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TagCount(n) => {
                write!(
                    f,
                    "{n} tags is more than the {MAX_RECORDING_TAGS} a recording holds"
                )
            }
            Self::TagLen(n) => write!(
                f,
                "a {n}-character tag is longer than the {MAX_RECORDING_TAG_LEN} a tag holds"
            ),
            Self::NoteLen(n) => write!(
                f,
                "a {n}-character note is longer than the {MAX_RECORDING_NOTE_LEN} a note holds"
            ),
        }
    }
}

impl std::error::Error for AnnotationError {}

impl RecordingAnnotation {
    pub fn normalized(&self) -> Result<Self, AnnotationError> {
        let mut tags: Vec<String> = Vec::new();
        for tag in &self.tags {
            let tag = tag.trim();
            if tag.is_empty() {
                continue;
            }
            if tag.chars().count() > MAX_RECORDING_TAG_LEN {
                return Err(AnnotationError::TagLen(tag.chars().count()));
            }
            if !tags.iter().any(|kept| kept.eq_ignore_ascii_case(tag)) {
                tags.push(tag.to_owned());
            }
        }
        if tags.len() > MAX_RECORDING_TAGS {
            return Err(AnnotationError::TagCount(tags.len()));
        }
        let note = self
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        if let Some(note) = note
            && note.chars().count() > MAX_RECORDING_NOTE_LEN
        {
            return Err(AnnotationError::NoteLen(note.chars().count()));
        }
        Ok(Self {
            tags,
            note: note.map(str::to_owned),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RecordingsResponse {
    pub recordings: Vec<RecordingInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChannelRecordRequest {
    pub action: RecordAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioRecordingInfo {
    pub file: String,
    pub channels: u8,
    pub sample_rate: u32,
    pub frames: u64,
    pub bytes: u64,
    pub duration_s: f64,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AudioRecordingsResponse {
    pub recordings: Vec<AudioRecordingInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlaybackRequest {
    pub action: PlaybackAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_samples: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackAction {
    Play,
    Pause,
    Stop,
    Seek,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecordingFormat {
    #[default]
    Sigmf,
    Wav,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct RecordingDownloadQuery {
    #[serde(default)]
    pub format: RecordingFormat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecoderLogEntry {
    pub id: i64,
    pub at: String,
    pub device_set: u32,
    pub channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub kind: String,
    pub freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    pub summary: String,
    pub event: DecoderEvent,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecoderLogResponse {
    pub entries: Vec<DecoderLogEntry>,
    pub total: u64,
    pub dropped: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    #[default]
    Csv,
    Json,
}

#[derive(
    Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema, utoipa::IntoParams,
)]
#[into_params(parameter_in = Query)]
pub struct DecoderLogQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_set: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nodes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

pub const MAX_LOG_SOURCES: usize = crate::patch::MAX_EDGES;
pub const MAX_LOG_KIND_LEN: usize = 32;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogScope {
    pub nodes: Vec<String>,
    pub channels: Vec<(u32, u32)>,
}

impl LogScope {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.channels.is_empty()
    }
}

impl DecoderLogQuery {
    pub fn kind_list(&self) -> Result<Vec<String>, &str> {
        parse_list(self.kinds.as_deref(), |kind| {
            if kind.is_empty() || kind.len() > MAX_LOG_KIND_LEN {
                Err(kind)
            } else {
                Ok(kind.to_owned())
            }
        })
    }

    pub fn scope(&self) -> Result<Option<LogScope>, &str> {
        if self.nodes.is_none() && self.sources.is_none() {
            return Ok(None);
        }
        Ok(Some(LogScope {
            nodes: parse_list(self.nodes.as_deref(), |id| {
                if id.is_empty() || id.len() > crate::patch::MAX_NODE_ID_LEN {
                    Err(id)
                } else {
                    Ok(id.to_owned())
                }
            })?,
            channels: parse_list(self.sources.as_deref(), |source| {
                let (set, channel) = source.split_once(':').ok_or(source)?;
                Ok((
                    set.parse().map_err(|_| source)?,
                    channel.parse().map_err(|_| source)?,
                ))
            })?,
        }))
    }
}

fn parse_list<T>(
    list: Option<&str>,
    item: impl Fn(&str) -> Result<T, &str>,
) -> Result<Vec<T>, &str> {
    let Some(list) = list.filter(|list| !list.is_empty()) else {
        return Ok(Vec::new());
    };
    let fragments: Vec<&str> = list.split(',').collect();
    if fragments.len() > MAX_LOG_SOURCES {
        return Err(list);
    }
    fragments.into_iter().map(item).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeletedCount {
    pub deleted: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub explainer: String,
    pub center_hz: f64,
    pub sample_rate: f64,
    pub channels: Vec<ChannelSettings>,
    pub min_freq_hz: f64,
    pub max_freq_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<crate::patch::PatchGraph>,
    #[serde(default = "receive")]
    pub direction: crate::device::Direction,
    #[serde(default)]
    pub exact_rate: bool,
    #[serde(default)]
    pub supported_devices: Vec<String>,
}

const fn receive() -> crate::device::Direction {
    crate::device::Direction::Rx
}

const RATE_TOLERANCE: f64 = 0.25;

impl TemplateInfo {
    #[must_use]
    pub fn unmet_by(&self, profile: &crate::device::DeviceProfile) -> Option<String> {
        if !profile.duplex.supports(self.direction) {
            return Some(format!("this radio does not {}", self.direction));
        }
        if !profile.reaches(self.min_freq_hz) || !profile.reaches(self.max_freq_hz) {
            return Some(format!(
                "needs {:.3}–{:.3} MHz, outside this radio's tuning range",
                self.min_freq_hz / 1e6,
                self.max_freq_hz / 1e6
            ));
        }
        let tolerance = if self.exact_rate { 0.0 } else { RATE_TOLERANCE };
        if !profile.runs_at(self.sample_rate, tolerance) {
            return Some(if self.exact_rate {
                format!(
                    "needs exactly {:.3} Msps, which this radio does not offer",
                    self.sample_rate / 1e6
                )
            } else {
                format!(
                    "needs about {:.3} Msps, which this radio does not offer",
                    self.sample_rate / 1e6
                )
            });
        }
        None
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct TemplatesResponse {
    pub templates: Vec<TemplateInfo>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApplyTemplateRequest {
    pub device_set: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClientsResponse {
    pub clients: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AuthInfo {
    pub token_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatedId {
    pub id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CreatedRowId {
    pub id: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ApiError {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{Capabilities, DcArtifact, DeviceProfile, Duplex, Range, StreamScope};

    fn scoped(nodes: Option<&str>, sources: Option<&str>) -> DecoderLogQuery {
        DecoderLogQuery {
            nodes: nodes.map(str::to_owned),
            sources: sources.map(str::to_owned),
            ..DecoderLogQuery::default()
        }
    }

    #[test]
    fn a_wire_scope_reads_both_lists_and_refuses_anything_else() {
        assert_eq!(scoped(None, None).scope(), Ok(None));
        assert_eq!(
            scoped(Some(""), Some("")).scope(),
            Ok(Some(LogScope::default()))
        );
        assert!(
            scoped(Some(""), Some(""))
                .scope()
                .unwrap()
                .unwrap()
                .is_empty()
        );

        assert_eq!(
            scoped(Some("channel:a1,ch0"), Some("0:1,2:13")).scope(),
            Ok(Some(LogScope {
                nodes: vec!["channel:a1".to_owned(), "ch0".to_owned()],
                channels: vec![(0, 1), (2, 13)],
            }))
        );
        assert_eq!(
            scoped(Some("ch0"), None).scope(),
            Ok(Some(LogScope {
                nodes: vec!["ch0".to_owned()],
                channels: Vec::new(),
            }))
        );

        for bad in ["0", "0:", ":1", "0:1,", "a:1", "0:-1", "0:1:2"] {
            assert!(scoped(None, Some(bad)).scope().is_err(), "{bad}");
        }
        assert!(scoped(Some("a,,b"), None).scope().is_err());
        assert!(
            scoped(Some(&"n".repeat(crate::patch::MAX_NODE_ID_LEN + 1)), None)
                .scope()
                .is_err()
        );

        let outsize = |item: fn(usize) -> String| {
            (0..=MAX_LOG_SOURCES)
                .map(item)
                .collect::<Vec<_>>()
                .join(",")
        };
        assert!(
            scoped(None, Some(&outsize(|n| format!("0:{n}"))))
                .scope()
                .is_err()
        );
        assert!(
            scoped(Some(&outsize(|n| format!("ch{n}"))), None)
                .scope()
                .is_err()
        );
    }

    fn profile(freq: Vec<Range>, rates: Vec<f64>, duplex: Duplex) -> DeviceProfile {
        Capabilities {
            freq_ranges: freq,
            sample_rates: rates,
            sample_rate_ranges: Vec::new(),
            gains: Vec::new(),
            antennas: Vec::new(),
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Operator,
            hardware_sweep: false,
            coherence: crate::device::Coherence::None,
        }
        .profile()
    }

    fn range(min: f64, max: f64) -> Range {
        Range {
            min,
            max,
            step: None,
        }
    }

    fn template(min_freq_hz: f64, max_freq_hz: f64, sample_rate: f64) -> TemplateInfo {
        TemplateInfo {
            id: "t".to_string(),
            name: "T".to_string(),
            description: String::new(),
            explainer: String::new(),
            center_hz: min_freq_hz,
            sample_rate,
            channels: Vec::new(),
            min_freq_hz,
            max_freq_hz,
            patch: None,
            direction: crate::device::Direction::Rx,
            exact_rate: false,
            supported_devices: Vec::new(),
        }
    }

    #[test]
    fn a_template_out_of_a_radios_tuning_range_is_refused_with_the_span() {
        let adsb = template(1_090e6, 1_090e6, 2e6);
        let dongle = profile(vec![range(24e6, 1.766e9)], vec![2e6, 2.4e6], Duplex::RxOnly);
        assert_eq!(adsb.unmet_by(&dongle), None);

        let hf = profile(vec![range(0.0, 30e6)], vec![2e6], Duplex::RxOnly);
        let reason = adsb.unmet_by(&hf).expect("out of range");
        assert!(reason.contains("1090.000"), "{reason}");
    }

    #[test]
    fn an_exact_rate_template_refuses_a_neighbouring_rate() {
        let dongle = profile(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);

        let mut adsb = template(1_090e6, 1_090e6, 2e6);
        adsb.exact_rate = true;
        let reason = adsb.unmet_by(&dongle).expect("2.048 is not 2.000");
        assert!(reason.contains("exactly"), "{reason}");

        let fm = template(98e6, 98e6, 2.4e6);
        assert_eq!(fm.unmet_by(&dongle), None, "a nominal rate tolerates 2.048");
    }

    #[test]
    fn a_transmit_template_is_refused_on_a_receiver() {
        let mut beacon = template(144e6, 144e6, 1e6);
        beacon.direction = crate::device::Direction::Tx;

        let receiver = profile(vec![range(24e6, 1.766e9)], vec![1e6], Duplex::RxOnly);
        let reason = beacon.unmet_by(&receiver).expect("a receiver cannot send");
        assert!(reason.contains("transmitting"), "{reason}");

        let transceiver = profile(vec![range(1e6, 6e9)], vec![1e6], Duplex::Half);
        assert_eq!(beacon.unmet_by(&transceiver), None);
    }

    #[test]
    fn a_radio_that_advertises_nothing_is_not_refused() {
        let unknown = profile(Vec::new(), Vec::new(), Duplex::RxOnly);
        assert_eq!(template(1_090e6, 1_090e6, 2e6).unmet_by(&unknown), None);
    }

    #[test]
    fn a_v1_preset_is_not_a_workspace_preset() {
        let v1 = serde_json::json!({
            "version": 1,
            "device_id": "virtual:siggen",
            "settings": {},
            "channels": [],
        });
        let parsed: PresetSnapshot = serde_json::from_value(v1).expect("the shape still parses");
        assert_ne!(parsed.version, PRESET_SNAPSHOT_VERSION);
        assert!(parsed.devices.is_empty());
    }

    #[test]
    fn an_older_template_payload_reads_as_receive() {
        let parsed: TemplateInfo = serde_json::from_str(
            r#"{"id":"t","name":"T","description":"","explainer":"","center_hz":98e6,
                "sample_rate":2.4e6,"channels":[],"min_freq_hz":98e6,"max_freq_hz":98e6}"#,
        )
        .expect("a template from before the direction field");
        assert_eq!(parsed.direction, crate::device::Direction::Rx);
        assert!(!parsed.exact_rate);
        assert!(parsed.supported_devices.is_empty());
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OccupancyBucket {
    pub freq_hz: u64,
    pub duty: f32,
    pub samples: u64,
    pub by_hour: Vec<f32>,
    pub last_seen: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct OccupancyReport {
    pub bucket_hz: u64,
    pub since: String,
    pub buckets: Vec<OccupancyBucket>,
}
