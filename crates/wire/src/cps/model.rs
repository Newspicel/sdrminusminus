use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::codeplug::{ChannelKind, Power};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UsbMatch {
    pub vid: u16,
    pub pid: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FrequencyRange {
    pub lower_hz: u64,
    pub upper_hz: u64,
}

impl FrequencyRange {
    #[must_use]
    pub const fn new(lower_hz: u64, upper_hz: u64) -> Self {
        Self { lower_hz, upper_hz }
    }

    #[must_use]
    pub const fn contains(&self, hz: u64) -> bool {
        hz >= self.lower_hz && hz <= self.upper_hz
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioFeatures {
    pub dual_zone_lists: bool,
    pub per_channel_radio_id: bool,
    pub scan_lists: bool,
    pub group_lists: bool,
    pub dcs_tones: bool,
    pub talkaround: bool,
    pub named_radio_ids: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioLimits {
    pub channels: u32,
    pub contacts: u32,
    pub group_lists: u32,
    pub group_list_members: u32,
    pub zones: u32,
    pub zone_channels: u32,
    pub scan_lists: u32,
    pub scan_list_members: u32,
    pub radio_ids: u32,
    pub channel_name_len: u32,
    pub contact_name_len: u32,
    pub group_list_name_len: u32,
    pub zone_name_len: u32,
    pub scan_list_name_len: u32,
    pub radio_id_name_len: u32,
    pub rx_ranges: Vec<FrequencyRange>,
    pub tx_ranges: Vec<FrequencyRange>,
    pub powers: Vec<Power>,
    pub modes: Vec<ChannelKind>,
    pub frequency_step_hz: u64,
    pub features: RadioFeatures,
}

impl RadioLimits {
    #[must_use]
    pub fn can_receive(&self, hz: u64) -> bool {
        self.rx_ranges.iter().any(|range| range.contains(hz))
    }

    #[must_use]
    pub fn can_transmit(&self, hz: u64) -> bool {
        self.tx_ranges.iter().any(|range| range.contains(hz))
    }

    #[must_use]
    pub fn supports(&self, kind: ChannelKind) -> bool {
        self.modes.contains(&kind)
    }

    #[must_use]
    pub fn nearest_power(&self, wanted: Power) -> Power {
        self.powers
            .iter()
            .copied()
            .min_by_key(|power| power.rank().abs_diff(wanted.rank()))
            .unwrap_or(wanted)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioModelDescriptor {
    pub id: String,
    pub manufacturer: String,
    pub model: String,
    pub family: String,
    pub usb: Vec<UsbMatch>,
    pub needs_explicit_selection: bool,
    pub transfer_bytes: u64,
    pub limits: RadioLimits,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RadioModelsResponse {
    pub models: Vec<RadioModelDescriptor>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortMatch {
    Confirmed,
    Probable,
    Possible,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsPort {
    pub port: String,
    pub label: String,
    pub match_kind: PortMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial_number: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_vid: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usb_pid: Option<u16>,
    pub candidate_models: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CpsPortsResponse {
    pub ports: Vec<CpsPort>,
    pub ignored_ports: Vec<String>,
}
