use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{PskBaud, RadioClockStandard, channel::SstvMode};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RdsUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ps: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radiotext: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tp: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ta: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub music: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alt_freqs_hz: Vec<f64>,
    pub groups: u64,
    pub blocks: u64,
    pub block_errors: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PocsagPayload {
    Tone,
    Numeric,
    Alpha,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PocsagMessage {
    pub address: u32,
    pub function: u8,
    pub baud: u16,
    pub payload: PocsagPayload,
    pub text: String,
    pub errors_corrected: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PagerPayload {
    Tone,
    Numeric,
    Alpha,
    Binary,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct FlexMessage {
    pub address: u64,
    pub payload: PagerPayload,
    pub text: String,
    pub baud: u16,
    pub levels: u8,
    pub cycle: u8,
    pub frame: u8,
    pub phase: char,
    pub errors_corrected: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ErmesMessage {
    pub local_address: u32,
    pub message_number: u8,
    pub payload: PagerPayload,
    pub text: String,
    pub urgent: bool,
    pub alert: u8,
    pub errors_corrected: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AdsbMessage {
    pub icao: String,
    pub df: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_code: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callsign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground_speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vertical_rate_fpm: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squawk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_ground: Option<bool>,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AisMessage {
    pub mmsi: u32,
    pub msg_type: u8,
    pub ais_channel: char,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_sign: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sog_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cog_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_deg: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_status: Option<u8>,
    pub nmea: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AprsPacket {
    pub source: String,
    pub destination: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<String>,
    pub info: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub course_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_kt: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub altitude_ft: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_e_message: Option<String>,
    pub tnc2: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RttyText {
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MorseText {
    pub text: String,
    pub wpm: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct CwSkimmerSpot {
    pub offset_hz: f32,
    pub text: String,
    pub wpm: f32,
    pub snr_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WsjtMessage {
    pub text: String,
    pub snr_db: f32,
    pub audio_hz: f32,
    pub time_offset_s: f32,
    pub hard_errors: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct PskText {
    pub baud: PskBaud,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct WsprSpot {
    pub text: String,
    pub callsign: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<String>,
    pub power_dbm: i32,
    pub snr_db: f32,
    pub audio_hz: f32,
    pub time_offset_s: f32,
    pub drift_hz: f32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SelcallSequence {
    pub system: crate::channel::SelcallSystem,
    pub code: String,
    pub tone_ms: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NavtexMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<char>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<char>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<u8>,
    pub text: String,
    pub errors_corrected: u32,
    pub complete: bool,
}

impl NavtexMessage {
    #[must_use]
    pub fn header(&self) -> Option<String> {
        let (station, subject, serial) = (self.station?, self.subject?, self.serial?);
        Some(format!("{station}{subject}{serial:02}"))
    }

    #[must_use]
    pub fn subject_name(subject: char) -> Option<&'static str> {
        Some(match subject.to_ascii_uppercase() {
            'A' => "Navigational warning",
            'B' => "Meteorological warning",
            'C' => "Ice report",
            'D' => "Search and rescue / piracy",
            'E' => "Meteorological forecast",
            'F' => "Pilot service",
            'G' => "AIS",
            'H' => "LORAN",
            'J' => "SATNAV",
            'K' => "Other electronic navaid",
            'L' => "Navigational warning (additional)",
            'T' => "Test transmission",
            'V' => "Notice to fishermen",
            'W' => "Environmental",
            'X' | 'Y' => "Special service",
            'Z' => "No message on hand",
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AcarsMessage {
    pub mode: char,
    pub registration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack: Option<char>,
    pub label: String,
    pub block_id: char,
    pub downlink: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_no: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flight: Option<String>,
    pub text: String,
    pub more: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubghzEncoding {
    Pcm,
    Pwm,
    Ppm,
    Manchester,
    Dmc,
    #[default]
    Raw,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubghzReading {
    pub model: String,
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature_c: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub humidity_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moisture_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pressure_kpa: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_avg_kmh: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_max_kmh: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wind_dir_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rain_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_w: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy_kwh: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct SubghzFrame {
    pub modulation: crate::channel::SubghzModulation,
    pub encoding: SubghzEncoding,
    pub bits: u32,
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tri_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<SubghzReading>,
    pub short_us: u32,
    pub repeats: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub timings_us: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ToneSquelchStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctcss_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dcs_code: Option<u16>,
    pub open: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ScramblerStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inversion_hz: Option<f64>,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Modulation {
    #[default]
    None,
    Carrier,
    Ook,
    Am,
    Ssb,
    Fm,
    Fsk2,
    Fsk4,
    Psk2,
    Psk4,
    NoiseLike,
    Unknown,
}

impl Modulation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "no signal",
            Self::Carrier => "unmodulated carrier",
            Self::Ook => "OOK",
            Self::Am => "AM",
            Self::Ssb => "SSB",
            Self::Fm => "FM",
            Self::Fsk2 => "2-FSK",
            Self::Fsk4 => "4-FSK",
            Self::Psk2 => "BPSK",
            Self::Psk4 => "QPSK",
            Self::NoiseLike => "noise-like",
            Self::Unknown => "unknown",
        }
    }

    #[must_use]
    pub const fn is_signal(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IdentFeatures {
    pub envelope_variation: f32,
    pub duty: f32,
    pub keying_depth_db: f32,
    pub spectral_asymmetry: f32,
    pub carrier_db: f32,
    pub spectral_flatness: f32,
    pub frequency_levels: u8,
    pub frequency_spread_hz: f64,
    pub square_line_db: f32,
    pub quartic_line_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProtocolMatch {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_id: Option<String>,
    pub score: f32,
    #[serde(default)]
    pub confirmed: bool,
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IdentReport {
    pub modulation: Modulation,
    pub confidence: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideband: Option<crate::channel::Sideband>,
    pub bandwidth_hz: f64,
    pub center_offset_hz: f64,
    pub snr_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_rate_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deviation_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ProtocolMatch>,
    pub features: IdentFeatures,
}

impl IdentReport {
    #[must_use]
    pub fn best(&self) -> Option<&ProtocolMatch> {
        self.candidates.first()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvMode {
    #[default]
    Dmr,
    Dstar,
    Ysf,
    Nxdn,
    P25,
    Dpmr,
    M17,
    #[serde(rename = "freedv")]
    FreeDv,
}

impl DvMode {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Dmr => "DMR",
            Self::Dstar => "D-STAR",
            Self::Ysf => "YSF",
            Self::Nxdn => "NXDN",
            Self::P25 => "P25",
            Self::Dpmr => "dPMR",
            Self::M17 => "M17",
            Self::FreeDv => "FreeDV",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvFrameKind {
    #[default]
    Header,
    Voice,
    Terminator,
    Control,
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Standard,
    Etsi,
    Motorola,
    Hytera,
    Harris,
    Tait,
    JvcKenwood,
    Emc,
    RadioActivity,
    FlydeMicro,
    ProdEl,
    Unknown,
}

impl Vendor {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Etsi => "ETSI",
            Self::Motorola => "Motorola",
            Self::Hytera => "Hytera",
            Self::Harris => "Harris",
            Self::Tait => "Tait",
            Self::JvcKenwood => "JVCKENWOOD",
            Self::Emc => "EMC",
            Self::RadioActivity => "Radio Activity",
            Self::FlydeMicro => "Flyde Micro",
            Self::ProdEl => "PROD-EL",
            Self::Unknown => "unknown vendor",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DvSlotActivity {
    pub slot: u8,
    pub activity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_hash: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_channel: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DvChannelDefinition {
    pub channel: u16,
    pub tx_hz: u64,
    pub rx_hz: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DvTrunkProtocol {
    CapacityPlus,
    HyteraXpt,
    TierThree,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DvFrame {
    pub mode: DvMode,
    pub kind: DvFrameKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<Vendor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_call: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub algorithm_id: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_indicator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub talker_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_error_m: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_definition: Option<DvChannelDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trunk_protocol: Option<DvTrunkProtocol>,
    /// Whether the burst came from a trunked site's control channel rather than one of its
    /// traffic channels, where the air interface says so outright.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_channel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crc_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest_channel: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_id: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emergency: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_entry: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slot_activity: Vec<DvSlotActivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opcode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub errors_corrected: u32,
}

impl DvFrame {
    #[must_use]
    pub fn new(mode: DvMode, kind: DvFrameKind) -> Self {
        Self {
            mode,
            kind,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn parties(&self) -> Option<String> {
        let to = self.destination_call.clone().or_else(|| {
            self.destination.map(|d| match self.group_call {
                Some(false) => d.to_string(),
                _ => format!("TG {d}"),
            })
        });
        let from = self
            .source_call
            .clone()
            .or_else(|| self.source.map(|s| s.to_string()));
        match (to, from) {
            (Some(to), Some(from)) => Some(format!("{to} ← {from}")),
            (Some(to), None) => Some(to),
            (None, Some(from)) => Some(from),
            (None, None) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastSystem {
    #[default]
    Dab,
    DabPlus,
    DvbS,
    DvbS2,
    Drm30,
    DrmPlus,
}

impl BroadcastSystem {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dab => "DAB",
            Self::DabPlus => "DAB+",
            Self::DvbS => "DVB-S",
            Self::DvbS2 => "DVB-S2",
            Self::Drm30 => "DRM30",
            Self::DrmPlus => "DRM+",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BroadcastServiceKind {
    #[default]
    Audio,
    Data,
    Video,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BroadcastService {
    pub id: u32,
    pub label: String,
    pub kind: BroadcastServiceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct BroadcastStatus {
    pub system: BroadcastSystem,
    pub locked: bool,
    pub snr_db: f32,
    pub frequency_error_hz: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ensemble_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ensemble_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_rate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bit_error_rate: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default)]
    pub frames_ok: u32,
    #[serde(default)]
    pub frames_bad: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<BroadcastService>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct RadioClockFrame {
    pub standard: RadioClockStandard,
    pub datetime: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub utc_offset_minutes: Option<i16>,
    #[serde(default)]
    pub dst: bool,
    #[serde(default)]
    pub leap_warning: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dut1_seconds: Option<f32>,
    pub symbols: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GnssFrame {
    pub prn: u8,
    pub doppler_hz: f32,
    pub code_phase_chips: f32,
    pub cn0_db_hz: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subframe: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tow_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week: Option<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SstvPicture {
    pub seq: u32,
    pub mode: SstvMode,
    pub width: u16,
    pub height: u16,
    pub lines: u16,
    pub complete: bool,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct VorReading {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station_lon: Option<f64>,
    pub magnetic_declination_deg: f64,
    pub radial_deg: f64,
    pub variable_phase_deg: f64,
    pub reference_phase_deg: f64,
    pub signal_db: f32,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct IlsReading {
    pub component: crate::channel::IlsComponent,
    pub modulation_90: f32,
    pub modulation_150: f32,
    pub ddm: f32,
    pub deviation_dots: f32,
    pub signal_db: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DataLinkMessage {
    pub message_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub crc_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fec_corrected: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snr_db: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frequency_error_hz: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lat: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lon: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    pub details: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum DecoderEvent {
    Rds(RdsUpdate),
    Pocsag(PocsagMessage),
    Flex(FlexMessage),
    Ermes(ErmesMessage),
    Adsb(AdsbMessage),
    Ais(AisMessage),
    Aprs(AprsPacket),
    Rtty(RttyText),
    Morse(MorseText),
    CwSkimmer(CwSkimmerSpot),
    Selcall(SelcallSequence),
    Navtex(NavtexMessage),
    Acars(AcarsMessage),
    Subghz(SubghzFrame),
    Tone(ToneSquelchStatus),
    Scrambler(ScramblerStatus),
    Dv(DvFrame),
    Call(crate::rest::VoiceCall),
    Ft8(WsjtMessage),
    Ft4(WsjtMessage),
    Psk(PskText),
    Wspr(WsprSpot),
    Ident(IdentReport),
    Broadcast(BroadcastStatus),
    RadioClock(RadioClockFrame),
    Gnss(GnssFrame),
    Sstv(SstvPicture),
    Vor(VorReading),
    Ils(IlsReading),
    Dsc(DataLinkMessage),
    InmarsatStdc(DataLinkMessage),
    InmarsatAero(DataLinkMessage),
    Vdl2(DataLinkMessage),
    Hfdl(DataLinkMessage),
    Iridium(DataLinkMessage),
    Df(crate::coherent::DfBearing),
    DfFix(crate::coherent::DfEstimate),
    Radar(crate::coherent::RadarDetection),
}

fn rds_summary(r: &RdsUpdate) -> String {
    let mut parts = Vec::new();
    if let Some(pi) = &r.pi {
        parts.push(format!("PI {pi}"));
    }
    if let Some(ps) = &r.ps {
        parts.push(ps.clone());
    }
    if let Some(pty) = &r.pty_name {
        parts.push(pty.clone());
    }
    if let Some(rt) = &r.radiotext {
        parts.push(rt.clone());
    }
    parts.join(" · ")
}

fn tone_summary(t: &ToneSquelchStatus) -> String {
    let mut parts = Vec::new();
    if let Some(hz) = t.ctcss_hz {
        parts.push(format!("CTCSS {hz:.1} Hz"));
    }
    if let Some(code) = t.dcs_code {
        parts.push(format!("DCS {code:03}"));
    }
    if parts.is_empty() {
        parts.push("no tone".to_owned());
    }
    parts.push(if t.open { "open" } else { "muted" }.to_owned());
    parts.join(" · ")
}

fn subghz_summary(f: &SubghzFrame) -> String {
    let mut parts = vec![if f.bits == 0 {
        format!("raw, {} edges", f.timings_us.len())
    } else {
        format!("{} bit {}", f.bits, f.data)
    }];
    if let Some(address) = f.address {
        parts.push(format!("addr {address:05X}"));
    }
    if let Some(button) = f.button {
        parts.push(format!("btn {button:X}"));
    }
    if f.repeats > 1 {
        parts.push(format!("×{}", f.repeats));
    }
    parts.join(" · ")
}

fn call_summary(c: &crate::rest::VoiceCall) -> String {
    let mut parts = vec![c.mode.label().to_owned()];
    parts.push(c.destination.map_or_else(
        || "to unknown".to_owned(),
        |id| match c.group_call {
            Some(true) => format!("talkgroup {id}"),
            Some(false) => format!("radio {id}"),
            None => format!("to {id}"),
        },
    ));
    parts.push(
        c.source
            .map_or_else(|| "from unknown".to_owned(), |id| format!("from {id}")),
    );
    if let Some(slot) = c.slot {
        parts.push(format!("TS{slot}"));
    }
    if let Some(cc) = c.color_code {
        parts.push(format!("CC {cc}"));
    }
    parts.push(format!("{:.1} s", c.duration_ms as f64 / 1_000.0));
    if c.emergency {
        parts.push("emergency".to_owned());
    }
    if c.encrypted {
        parts.push("encrypted".to_owned());
    }
    parts.join(" · ")
}

fn dv_summary(f: &DvFrame) -> String {
    let mut parts = vec![f.mode.label().to_owned()];
    if let Some(slot) = f.slot {
        parts.push(format!("TS{slot}"));
    }
    if let Some(cc) = f.color_code {
        parts.push(match f.mode {
            DvMode::P25 => format!("NAC {cc:03X}"),
            DvMode::Nxdn | DvMode::Dpmr => format!("RAN {cc}"),
            _ => format!("CC {cc}"),
        });
    }
    if let Some(parties) = f.parties() {
        parts.push(parties);
    }
    if let Some(alias) = &f.talker_alias {
        parts.push(alias.clone());
    }
    if let Some(vendor) = f.vendor {
        parts.push(vendor.label().to_owned());
    }
    if let Some(via) = &f.via {
        parts.push(format!("via {via}"));
    }
    if let Some(opcode) = &f.opcode {
        parts.push(opcode.clone());
    }
    if f.encrypted == Some(true) {
        parts.push(match (f.algorithm_id, f.key_id) {
            (Some(algorithm), Some(key)) => {
                format!("encrypted ALG {algorithm:02X} KID {key:04X}")
            }
            _ => "encrypted".to_owned(),
        });
    }
    if let Some(text) = &f.text {
        parts.push(text.clone());
    }
    parts.join(" · ")
}

fn ident_summary(r: &IdentReport) -> String {
    let mut parts = vec![r.modulation.label().to_owned()];
    if r.modulation.is_signal() {
        parts.push(format!("{:.1} kHz", r.bandwidth_hz / 1_000.0));
        if let Some(baud) = r.symbol_rate_hz {
            parts.push(format!("{baud:.0} Bd"));
        }
        if let Some(deviation) = r.deviation_hz {
            parts.push(format!("±{deviation:.0} Hz"));
        }
        parts.push(format!("{:.0} dB SNR", r.snr_db));
    }
    if let Some(best) = r.best() {
        parts.push(if best.confirmed {
            format!("{} (confirmed)", best.name)
        } else {
            format!("{} ({:.0}%)", best.name, best.score * 100.0)
        });
    }
    parts.join(" · ")
}

fn gnss_summary(g: &GnssFrame) -> String {
    let mut parts = vec![
        format!("GPS PRN {}", g.prn),
        format!("{:+.0} Hz", g.doppler_hz),
        format!("{:.1} dB-Hz", g.cn0_db_hz),
    ];
    if let Some(id) = g.subframe {
        parts.push(format!("subframe {id}"));
    } else {
        parts.push("acquired".to_owned());
    }
    if let Some(tow) = g.tow_seconds {
        parts.push(format!("TOW {tow} s"));
    }
    parts.join(" · ")
}

fn data_link_summary(m: &DataLinkMessage) -> String {
    let mut parts = vec![m.message_type.clone()];
    if let Some(station) = &m.station {
        parts.push(station.clone());
    }
    if let Some(text) = &m.text {
        let text = text.replace('\n', " ");
        if !text.trim().is_empty() {
            parts.push(text.trim().to_owned());
        }
    }
    parts.join(" · ")
}

impl DecoderEvent {
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Rds(_) => "rds",
            Self::Pocsag(_) => "pocsag",
            Self::Flex(_) => "flex",
            Self::Ermes(_) => "ermes",
            Self::Adsb(_) => "adsb",
            Self::Ais(_) => "ais",
            Self::Aprs(_) => "aprs",
            Self::Rtty(_) => "rtty",
            Self::Morse(_) => "morse",
            Self::CwSkimmer(_) => "cw_skimmer",
            Self::Selcall(_) => "selcall",
            Self::Navtex(_) => "navtex",
            Self::Acars(_) => "acars",
            Self::Subghz(_) => "subghz",
            Self::Tone(_) => "tone",
            Self::Scrambler(_) => "scrambler",
            Self::Dv(_) => "dv",
            Self::Ft8(_) => "ft8",
            Self::Ft4(_) => "ft4",
            Self::Psk(_) => "psk",
            Self::Wspr(_) => "wspr",
            Self::Ident(_) => "ident",
            Self::Broadcast(_) => "broadcast",
            Self::RadioClock(_) => "radio_clock",
            Self::Gnss(_) => "gnss",
            Self::Sstv(_) => "sstv",
            Self::Vor(_) => "vor",
            Self::Ils(_) => "ils",
            Self::Dsc(_) => "dsc",
            Self::InmarsatStdc(_) => "inmarsat_stdc",
            Self::InmarsatAero(_) => "inmarsat_aero",
            Self::Vdl2(_) => "vdl2",
            Self::Hfdl(_) => "hfdl",
            Self::Iridium(_) => "iridium",
            Self::Call(_) => "call",
            Self::Df(_) => "df",
            Self::DfFix(_) => "df_fix",
            Self::Radar(_) => "radar",
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Rds(r) => rds_summary(r),
            Self::Pocsag(p) => {
                if p.text.is_empty() {
                    format!("{} ({})", p.address, p.function)
                } else {
                    format!("{}: {}", p.address, p.text)
                }
            }
            Self::Flex(p) => {
                if p.text.is_empty() {
                    format!("{} · {:?}", p.address, p.payload)
                } else {
                    format!("{}: {}", p.address, p.text)
                }
            }
            Self::Ermes(p) => {
                if p.text.is_empty() {
                    format!("{} · {:?}", p.local_address, p.payload)
                } else {
                    format!("{}: {}", p.local_address, p.text)
                }
            }
            Self::Adsb(a) => {
                let mut parts = vec![a.icao.clone()];
                if let Some(cs) = &a.callsign {
                    parts.push(cs.trim().to_owned());
                }
                if let Some(alt) = a.altitude_ft {
                    parts.push(format!("{alt} ft"));
                }
                if let (Some(lat), Some(lon)) = (a.lat, a.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            Self::Ais(m) => {
                let mut parts = vec![m.mmsi.to_string()];
                if let Some(name) = &m.name {
                    parts.push(name.trim().to_owned());
                }
                if let (Some(lat), Some(lon)) = (m.lat, m.lon) {
                    parts.push(format!("{lat:.4}, {lon:.4}"));
                }
                parts.join(" · ")
            }
            Self::Aprs(p) => match &p.mic_e_message {
                Some(message) => format!("{} · {message}", p.tnc2),
                None => p.tnc2.clone(),
            },
            Self::Rtty(t) => t.text.clone(),
            Self::Morse(m) => m.text.clone(),
            Self::CwSkimmer(s) => format!("{:+.0} Hz · {:.0} WPM · {}", s.offset_hz, s.wpm, s.text),
            Self::Selcall(s) => format!(
                "{} · {}",
                match s.system {
                    crate::channel::SelcallSystem::Ccir1 => "CCIR-1",
                    crate::channel::SelcallSystem::Zvei1 => "ZVEI-1",
                },
                s.code
            ),
            Self::Navtex(n) => {
                let mut parts = Vec::new();
                if let Some(id) = n.header() {
                    parts.push(id);
                }
                if let Some(subject) = &n.subject_name {
                    parts.push(subject.clone());
                }
                parts.push(n.text.replace('\n', " ").trim().to_owned());
                parts.retain(|p| !p.is_empty());
                parts.join(" · ")
            }
            Self::Acars(a) => {
                let mut parts = vec![a.registration.clone()];
                if let Some(flight) = &a.flight {
                    parts.push(flight.trim().to_owned());
                }
                parts.push(format!("[{}]", a.label));
                let text = a.text.replace('\n', " ");
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
                parts.join(" · ")
            }
            Self::Tone(t) => tone_summary(t),
            Self::Scrambler(s) => match s.inversion_hz {
                Some(hz) => format!(
                    "inversion {hz:.0} Hz · {:.0}% confidence",
                    s.confidence * 100.0
                ),
                None => "no inversion".to_owned(),
            },
            Self::Subghz(f) => subghz_summary(f),
            Self::Call(c) => call_summary(c),
            Self::Dv(f) => dv_summary(f),
            Self::Ft8(m) | Self::Ft4(m) => {
                format!("{} · {:+.0} dB · {:.0} Hz", m.text, m.snr_db, m.audio_hz)
            }
            Self::Psk(t) => t.text.clone(),
            Self::Wspr(s) => format!("{} · {:+.0} dB · {:.0} Hz", s.text, s.snr_db, s.audio_hz),
            Self::Ident(r) => ident_summary(r),
            Self::Broadcast(status) => {
                let mut parts = vec![status.system.label().to_owned()];
                parts.push(if status.locked { "locked" } else { "searching" }.to_owned());
                if status.locked {
                    parts.push(format!("{:.1} dB SNR", status.snr_db));
                    parts.push(format!("{:+.0} Hz", status.frequency_error_hz));
                }
                if let Some(label) = &status.ensemble_label {
                    parts.push(label.clone());
                }
                if let Some(label) = &status.label {
                    parts.push(label.clone());
                }
                if let Some(rate) = &status.code_rate {
                    parts.push(format!("FEC {rate}"));
                }
                if let Some(kbps) = status.bitrate_kbps {
                    parts.push(format!("{kbps} kbps"));
                }
                if let Some(ber) = status.bit_error_rate {
                    parts.push(format!("BER {ber:.1e}"));
                }
                if !status.services.is_empty() {
                    parts.push(format!("{} services", status.services.len()));
                }
                if let Some(text) = &status.text {
                    parts.push(text.clone());
                }
                parts.join(" · ")
            }
            Self::RadioClock(r) => {
                let mut parts = vec![
                    format!("{:?}", r.standard).to_uppercase(),
                    r.datetime.clone(),
                ];
                if r.leap_warning {
                    parts.push("leap warning".to_owned());
                }
                parts.join(" · ")
            }
            Self::Gnss(g) => gnss_summary(g),
            Self::Sstv(p) => {
                let mut parts = vec![
                    p.mode.label().to_owned(),
                    format!("{}×{}", p.width, p.height),
                ];
                parts.push(if p.complete {
                    format!("complete in {} s", p.duration_ms / 1_000)
                } else {
                    format!("{} of {} lines", p.lines, p.height)
                });
                parts.join(" · ")
            }
            Self::Vor(v) => {
                let station = v.station.as_deref().unwrap_or("VOR");
                format!(
                    "{station} · {:03.1}° radial · {:.0}%",
                    v.radial_deg,
                    v.confidence * 100.0
                )
            }
            Self::Df(b) => format!(
                "{:03.1}° bearing · {:.0}%",
                b.bearing_deg,
                b.confidence * 100.0
            ),
            Self::DfFix(e) => format!("{:.5}, {:.5} · ±{:.0} m", e.lat, e.lon, e.ellipse_major_m),
            Self::Radar(d) => format!(
                "range bin {} · {:.1} km · {:+.1} Hz · {:.1} dB",
                d.range_bin, d.range_km, d.doppler_hz, d.snr_db
            ),
            Self::Ils(i) => {
                let component = match i.component {
                    crate::channel::IlsComponent::Localizer => "localizer",
                    crate::channel::IlsComponent::Glideslope => "glideslope",
                };
                format!(
                    "{component} · {:+.3} DDM · {:+.2} dots",
                    i.ddm, i.deviation_dots
                )
            }
            Self::Dsc(m)
            | Self::InmarsatStdc(m)
            | Self::InmarsatAero(m)
            | Self::Vdl2(m)
            | Self::Hfdl(m)
            | Self::Iridium(m) => data_link_summary(m),
        }
    }

    #[must_use]
    pub fn position(&self) -> Option<(f64, f64)> {
        let (lat, lon) = match self {
            Self::Adsb(a) => (a.lat, a.lon),
            Self::Ais(m) => (m.lat, m.lon),
            Self::Aprs(p) => (p.lat, p.lon),
            Self::Dv(f) => (f.lat, f.lon),
            Self::Df(b) => (b.lat, b.lon),
            Self::DfFix(e) => (Some(e.lat), Some(e.lon)),
            Self::Dsc(m)
            | Self::InmarsatStdc(m)
            | Self::InmarsatAero(m)
            | Self::Vdl2(m)
            | Self::Hfdl(m)
            | Self::Iridium(m) => (m.lat, m.lon),
            _ => (None, None),
        };
        lat.zip(lon)
    }

    #[must_use]
    pub fn station(&self) -> Option<String> {
        match self {
            Self::Rds(r) => r.pi.clone(),
            Self::Pocsag(p) => Some(p.address.to_string()),
            Self::Flex(p) => Some(p.address.to_string()),
            Self::Ermes(p) => Some(p.local_address.to_string()),
            Self::Adsb(a) => Some(a.icao.clone()),
            Self::Ais(m) => Some(m.mmsi.to_string()),
            Self::Aprs(p) => Some(p.source.clone()),
            Self::Navtex(n) => n.station.map(String::from),
            Self::Acars(a) => Some(a.registration.clone()),
            Self::Subghz(f) => f
                .address
                .map(|a| format!("{a:05X}"))
                .or_else(|| (!f.data.is_empty()).then(|| f.data.clone())),
            Self::Dv(f) => f
                .source_call
                .clone()
                .or_else(|| f.source.map(|s| s.to_string())),
            Self::Call(c) => c.source.map(|s| s.to_string()),
            Self::Ft8(m) | Self::Ft4(m) => m.text.split_whitespace().nth(1).map(str::to_owned),
            Self::Wspr(s) => Some(s.callsign.clone()),
            Self::Rtty(_)
            | Self::Morse(_)
            | Self::CwSkimmer(_)
            | Self::Psk(_)
            | Self::Tone(_)
            | Self::Scrambler(_)
            | Self::Ident(_)
            | Self::Selcall(_) => None,
            Self::Broadcast(status) => status
                .service_id
                .or(status.ensemble_id)
                .map(|id| format!("{id:X}")),
            Self::Gnss(g) => Some(format!("GPS-{}", g.prn)),
            Self::RadioClock(r) => Some(format!("{:?}", r.standard).to_uppercase()),
            Self::Sstv(p) => Some(p.mode.label().to_owned()),
            Self::Vor(v) => v.station.clone(),
            Self::Df(b) => b.station_id.clone(),
            Self::DfFix(_) | Self::Radar(_) => None,
            Self::Dsc(m)
            | Self::InmarsatStdc(m)
            | Self::InmarsatAero(m)
            | Self::Vdl2(m)
            | Self::Hfdl(m)
            | Self::Iridium(m) => m.station.clone(),
            Self::Ils(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DecodedRecord {
    pub device_set: u32,
    pub channel: u32,
    pub at: String,
    pub freq_hz: f64,
    pub event: DecoderEvent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_sstv_picture_summarises_its_mode_size_and_outcome() {
        let complete = DecoderEvent::Sstv(SstvPicture {
            seq: 1,
            mode: SstvMode::MartinM1,
            width: 320,
            height: 256,
            lines: 256,
            complete: true,
            duration_ms: 114_300,
        });
        assert_eq!(complete.kind(), "sstv");
        assert_eq!(
            complete.summary(),
            "Martin M1 · 320×256 · complete in 114 s"
        );
        assert_eq!(complete.station().as_deref(), Some("Martin M1"));

        let cut_short = DecoderEvent::Sstv(SstvPicture {
            seq: 2,
            mode: SstvMode::Robot36,
            width: 320,
            height: 240,
            lines: 96,
            complete: false,
            duration_ms: 14_500,
        });
        assert_eq!(cut_short.summary(), "Robot 36 · 320×240 · 96 of 240 lines");
    }

    #[test]
    fn decoder_event_is_adjacently_tagged() {
        let ev = DecoderEvent::Pocsag(PocsagMessage {
            address: 1_234_567,
            function: 3,
            baud: 1_200,
            payload: PocsagPayload::Alpha,
            text: "TEST".to_owned(),
            errors_corrected: 1,
        });
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "pocsag");
        assert_eq!(json["data"]["address"], 1_234_567);
        assert_eq!(json["data"]["payload"], "alpha");
        assert_eq!(ev.kind(), "pocsag");

        let back: DecoderEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn kind_matches_the_serialized_tag() {
        let link = || DataLinkMessage {
            message_type: "test".to_owned(),
            station: None,
            text: None,
            crc_ok: true,
            fec_corrected: None,
            snr_db: None,
            frequency_error_hz: None,
            lat: None,
            lon: None,
            raw: None,
            details: serde_json::Value::Null,
        };
        for ev in [
            DecoderEvent::Rds(RdsUpdate::default()),
            DecoderEvent::Pocsag(PocsagMessage {
                address: 1,
                function: 0,
                baud: 512,
                payload: PocsagPayload::Tone,
                text: String::new(),
                errors_corrected: 0,
            }),
            DecoderEvent::Flex(FlexMessage {
                address: 1,
                payload: PagerPayload::Tone,
                text: String::new(),
                baud: 1_600,
                levels: 2,
                cycle: 0,
                frame: 0,
                phase: 'A',
                errors_corrected: 0,
            }),
            DecoderEvent::Ermes(ErmesMessage {
                local_address: 1,
                message_number: 0,
                payload: PagerPayload::Tone,
                text: String::new(),
                urgent: false,
                alert: 0,
                errors_corrected: 0,
            }),
            DecoderEvent::Adsb(AdsbMessage::default()),
            DecoderEvent::Ais(AisMessage::default()),
            DecoderEvent::Aprs(AprsPacket::default()),
            DecoderEvent::Rtty(RttyText {
                text: String::new(),
            }),
            DecoderEvent::Morse(MorseText {
                text: String::new(),
                wpm: 0.0,
            }),
            DecoderEvent::CwSkimmer(CwSkimmerSpot {
                offset_hz: 0.0,
                text: String::new(),
                wpm: 0.0,
                snr_db: 0.0,
            }),
            DecoderEvent::Selcall(SelcallSequence {
                system: crate::channel::SelcallSystem::Ccir1,
                code: "12345".to_owned(),
                tone_ms: 100,
            }),
            DecoderEvent::Navtex(NavtexMessage::default()),
            DecoderEvent::Acars(AcarsMessage::default()),
            DecoderEvent::Subghz(SubghzFrame::default()),
            DecoderEvent::Tone(ToneSquelchStatus::default()),
            DecoderEvent::Scrambler(ScramblerStatus::default()),
            DecoderEvent::Ft8(WsjtMessage {
                text: String::new(),
                snr_db: 0.0,
                audio_hz: 0.0,
                time_offset_s: 0.0,
                hard_errors: 0,
            }),
            DecoderEvent::Ft4(WsjtMessage {
                text: String::new(),
                snr_db: 0.0,
                audio_hz: 0.0,
                time_offset_s: 0.0,
                hard_errors: 0,
            }),
            DecoderEvent::Psk(PskText {
                baud: PskBaud::Psk31,
                text: String::new(),
            }),
            DecoderEvent::Wspr(WsprSpot {
                text: String::new(),
                callsign: String::new(),
                grid: None,
                power_dbm: 0,
                snr_db: 0.0,
                audio_hz: 0.0,
                time_offset_s: 0.0,
                drift_hz: 0.0,
            }),
            DecoderEvent::Broadcast(BroadcastStatus::default()),
            DecoderEvent::RadioClock(RadioClockFrame {
                standard: RadioClockStandard::Dcf77,
                datetime: "2026-08-15T12:34:00+02:00".to_owned(),
                utc_offset_minutes: Some(120),
                dst: true,
                leap_warning: false,
                dut1_seconds: None,
                symbols: String::new(),
            }),
            DecoderEvent::Gnss(GnssFrame {
                prn: 1,
                doppler_hz: 0.0,
                code_phase_chips: 0.0,
                cn0_db_hz: 40.0,
                subframe: None,
                tow_seconds: None,
                week: None,
                words: Vec::new(),
            }),
            DecoderEvent::Vor(VorReading {
                station: None,
                station_lat: None,
                station_lon: None,
                magnetic_declination_deg: 0.0,
                radial_deg: 0.0,
                variable_phase_deg: 0.0,
                reference_phase_deg: 0.0,
                signal_db: 0.0,
                confidence: 1.0,
            }),
            DecoderEvent::Ils(IlsReading {
                component: crate::channel::IlsComponent::Localizer,
                modulation_90: 0.0,
                modulation_150: 0.0,
                ddm: 0.0,
                deviation_dots: 0.0,
                signal_db: 0.0,
            }),
            DecoderEvent::Dsc(link()),
            DecoderEvent::InmarsatStdc(link()),
            DecoderEvent::InmarsatAero(link()),
            DecoderEvent::Vdl2(link()),
            DecoderEvent::Hfdl(link()),
            DecoderEvent::Iridium(link()),
        ] {
            let json = serde_json::to_value(&ev).unwrap();
            assert_eq!(json["kind"], ev.kind());
        }
    }

    #[test]
    fn navtex_subject_names_match_the_standard_table() {
        const NAMED: [(char, &str); 16] = [
            ('A', "Navigational warning"),
            ('B', "Meteorological warning"),
            ('C', "Ice report"),
            ('D', "Search and rescue / piracy"),
            ('E', "Meteorological forecast"),
            ('F', "Pilot service"),
            ('G', "AIS"),
            ('H', "LORAN"),
            ('J', "SATNAV"),
            ('K', "Other electronic navaid"),
            ('L', "Navigational warning (additional)"),
            ('T', "Test transmission"),
            ('V', "Notice to fishermen"),
            ('W', "Environmental"),
            ('X', "Special service"),
            ('Y', "Special service"),
        ];
        for (subject, name) in NAMED {
            assert_eq!(
                NavtexMessage::subject_name(subject),
                Some(name),
                "{subject}"
            );
        }
        assert_eq!(NavtexMessage::subject_name('Z'), Some("No message on hand"));
        for unassigned in ['I', 'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'U'] {
            assert_eq!(
                NavtexMessage::subject_name(unassigned),
                None,
                "{unassigned} is unassigned and must stay unnamed"
            );
        }
    }

    #[test]
    fn navtex_header_needs_the_whole_group() {
        let mut msg = NavtexMessage {
            station: Some('D'),
            subject: Some('A'),
            serial: Some(7),
            ..NavtexMessage::default()
        };
        assert_eq!(msg.header().as_deref(), Some("DA07"));
        msg.serial = None;
        assert_eq!(msg.header(), None);
    }

    #[test]
    fn subghz_summary_describes_a_raw_capture() {
        let raw = DecoderEvent::Subghz(SubghzFrame {
            timings_us: vec![320, 960, 960, 320],
            repeats: 1,
            ..SubghzFrame::default()
        });
        assert_eq!(raw.summary(), "raw, 4 edges");
        assert_eq!(raw.station(), None);

        let decoded = DecoderEvent::Subghz(SubghzFrame {
            encoding: SubghzEncoding::Pwm,
            bits: 24,
            data: "A1B2C3".to_owned(),
            address: Some(0x0A1B2),
            button: Some(3),
            repeats: 4,
            ..SubghzFrame::default()
        });
        assert_eq!(decoded.summary(), "24 bit A1B2C3 · addr 0A1B2 · btn 3 · ×4");
        assert_eq!(decoded.station().as_deref(), Some("0A1B2"));
    }

    #[test]
    fn clock_and_gnss_summaries_keep_the_measurements_used_by_the_live_log() {
        let clock = DecoderEvent::RadioClock(RadioClockFrame {
            standard: RadioClockStandard::Dcf77,
            datetime: "2026-08-15T12:34:00+02:00".to_owned(),
            utc_offset_minutes: Some(120),
            dst: true,
            leap_warning: false,
            dut1_seconds: None,
            symbols: String::new(),
        });
        assert_eq!(clock.summary(), "DCF77 · 2026-08-15T12:34:00+02:00");
        assert_eq!(clock.station().as_deref(), Some("DCF77"));

        let gnss = DecoderEvent::Gnss(GnssFrame {
            prn: 7,
            doppler_hz: 1_000.0,
            code_phase_chips: 158.34,
            cn0_db_hz: 44.5,
            subframe: None,
            tow_seconds: None,
            week: None,
            words: Vec::new(),
        });
        assert_eq!(
            gnss.summary(),
            "GPS PRN 7 · +1000 Hz · 44.5 dB-Hz · acquired"
        );
        assert_eq!(gnss.station().as_deref(), Some("GPS-7"));
    }

    #[test]
    fn a_mic_e_summary_names_the_message_beside_the_monitor_line() {
        let mut packet = AprsPacket {
            tnc2: "DL1ABC-9>S32U6T:`(_fn\"Oj/".to_owned(),
            ..AprsPacket::default()
        };
        assert_eq!(
            DecoderEvent::Aprs(packet.clone()).summary(),
            "DL1ABC-9>S32U6T:`(_fn\"Oj/"
        );
        packet.mic_e_message = Some("Returning".to_owned());
        assert_eq!(
            DecoderEvent::Aprs(packet).summary(),
            "DL1ABC-9>S32U6T:`(_fn\"Oj/ · Returning"
        );
    }

    #[test]
    fn position_is_reported_only_when_both_coordinates_are_known() {
        let mut a = AdsbMessage {
            icao: "3C6444".to_owned(),
            lat: Some(52.5),
            ..AdsbMessage::default()
        };
        assert_eq!(DecoderEvent::Adsb(a.clone()).position(), None);
        a.lon = Some(13.4);
        assert_eq!(DecoderEvent::Adsb(a).position(), Some((52.5, 13.4)));
        assert_eq!(
            DecoderEvent::Rtty(RttyText {
                text: "CQ".to_owned()
            })
            .position(),
            None
        );
    }

    #[test]
    fn decoded_record_roundtrips() {
        let rec = DecodedRecord {
            device_set: 1,
            channel: 2,
            at: "2026-08-09T12:00:00Z".to_owned(),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(AdsbMessage {
                icao: "3C6444".to_owned(),
                df: 17,
                type_code: Some(4),
                callsign: Some("DLH123".to_owned()),
                raw: "8D3C6444".to_owned(),
                ..AdsbMessage::default()
            }),
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["event"]["kind"], "adsb");
        assert_eq!(json["freq_hz"], 1_090_000_000.0);
        let back: DecodedRecord = serde_json::from_value(json).unwrap();
        assert_eq!(back, rec);
    }
}
