use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const CODEPLUG_VERSION: u32 = 1;

pub const ALL_CALL_NUMBER: u32 = 0x00ff_ffff;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CodeplugMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bands: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    Min,
    Low,
    #[default]
    Mid,
    High,
    Max,
}

impl Power {
    pub const ORDER: [Self; 5] = [Self::Min, Self::Low, Self::Mid, Self::High, Self::Max];

    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Min => 0,
            Self::Low => 1,
            Self::Mid => 2,
            Self::High => 3,
            Self::Max => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Bandwidth {
    #[default]
    Narrow,
    Wide,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeSlot {
    #[default]
    One,
    Two,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tone {
    Ctcss { decihertz: u16 },
    Dcs { code: u16, inverted: bool },
}

impl Tone {
    #[must_use]
    pub fn ctcss_hz(hz: f64) -> Self {
        Self::Ctcss {
            decihertz: (hz * 10.0).round().max(0.0).min(f64::from(u16::MAX)) as u16,
        }
    }

    #[must_use]
    pub fn hertz(self) -> Option<f64> {
        match self {
            Self::Ctcss { decihertz } => Some(f64::from(decihertz) / 10.0),
            Self::Dcs { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Admit {
    #[default]
    Always,
    ChannelFree,
    ColorCodeFree,
    DifferentColorCode,
    ToneFree,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    #[default]
    Fm,
    Dmr,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FmChannel {
    #[serde(default)]
    pub bandwidth: Bandwidth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx_tone: Option<Tone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_tone: Option<Tone>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch: Option<u8>,
    #[serde(default)]
    pub admit: Admit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DmrChannel {
    #[serde(default)]
    pub color_code: u8,
    #[serde(default)]
    pub time_slot: TimeSlot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_list: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_id: Option<String>,
    #[serde(default)]
    pub admit: Admit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ChannelMode {
    Fm(FmChannel),
    Dmr(DmrChannel),
}

impl Default for ChannelMode {
    fn default() -> Self {
        Self::Fm(FmChannel::default())
    }
}

impl ChannelMode {
    #[must_use]
    pub fn kind(&self) -> ChannelKind {
        match self {
            Self::Fm(_) => ChannelKind::Fm,
            Self::Dmr(_) => ChannelKind::Dmr,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Channel {
    pub name: String,
    pub rx_hz: u64,
    pub tx_hz: u64,
    #[serde(default)]
    pub power: Power,
    #[serde(default)]
    pub rx_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_list: Option<String>,
    #[serde(default, flatten)]
    pub mode: ChannelMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContactKind {
    Private,
    #[default]
    Group,
    All,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Contact {
    pub name: String,
    #[serde(default)]
    pub kind: ContactKind,
    pub number: u32,
    #[serde(default)]
    pub ring: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GroupList {
    pub name: String,
    #[serde(default)]
    pub contacts: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Zone {
    pub name: String,
    #[serde(default)]
    pub channels_a: Vec<String>,
    #[serde(default)]
    pub channels_b: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum ScanTarget {
    Selected,
    Channel { name: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScanRevert {
    #[default]
    Selected,
    LastCalled,
    LastUsed,
    Primary,
    Secondary,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ScanList {
    pub name: String,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<ScanTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<ScanTarget>,
    #[serde(default)]
    pub revert: ScanRevert,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dwell_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hang_ms: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioId {
    pub name: String,
    pub number: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GeneralSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_radio_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro_line1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro_line2: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squelch: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vox: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mic_gain: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Codeplug {
    pub version: u32,
    #[serde(default)]
    pub meta: CodeplugMeta,
    #[serde(default)]
    pub settings: GeneralSettings,
    #[serde(default)]
    pub radio_ids: Vec<RadioId>,
    #[serde(default)]
    pub contacts: Vec<Contact>,
    #[serde(default)]
    pub group_lists: Vec<GroupList>,
    #[serde(default)]
    pub channels: Vec<Channel>,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub scan_lists: Vec<ScanList>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl Codeplug {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: CODEPLUG_VERSION,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn counts(&self) -> CodeplugCounts {
        CodeplugCounts {
            channels: self.channels.len() as u32,
            contacts: self.contacts.len() as u32,
            group_lists: self.group_lists.len() as u32,
            zones: self.zones.len() as u32,
            scan_lists: self.scan_lists.len() as u32,
            radio_ids: self.radio_ids.len() as u32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CodeplugCounts {
    pub channels: u32,
    pub contacts: u32,
    pub group_lists: u32,
    pub zones: u32,
    pub scan_lists: u32,
    pub radio_ids: u32,
}
