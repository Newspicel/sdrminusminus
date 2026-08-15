use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceInfo {
    pub driver: String,
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<DeviceProfile>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceProfile {
    pub freq_ranges: Vec<Range>,
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_range: Option<Range>,
    pub duplex: Duplex,
    pub rx_streams: u32,
    pub tx_streams: u32,
    #[serde(default)]
    pub per_stream: StreamScope,
}

impl DeviceProfile {
    #[must_use]
    pub fn reaches(&self, hz: f64) -> bool {
        self.freq_ranges.is_empty()
            || self
                .freq_ranges
                .iter()
                .any(|range| hz >= range.min && hz <= range.max)
    }

    #[must_use]
    pub fn runs_at(&self, rate: f64, tolerance: f64) -> bool {
        if let Some(range) = &self.sample_rate_range
            && rate >= range.min
            && rate <= range.max
        {
            return true;
        }
        if self.sample_rates.is_empty() {
            return self.sample_rate_range.is_none();
        }
        if self
            .sample_rates
            .iter()
            .any(|have| (have - rate).abs() <= rate * tolerance)
        {
            return true;
        }
        if tolerance == 0.0 {
            return false;
        }
        let low = self
            .sample_rates
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let high = self
            .sample_rates
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        rate >= low && rate <= high
    }
}

impl DeviceInfo {
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.driver, self.key)
    }

    #[must_use]
    pub fn identity(&self) -> Self {
        Self {
            profile: None,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Rx,
    Tx,
}

impl Direction {
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Rx => Self::Tx,
            Self::Tx => Self::Rx,
        }
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Rx => "receiving",
            Self::Tx => "transmitting",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Duplex {
    #[default]
    RxOnly,
    TxOnly,
    Half,
    Full,
}

impl Duplex {
    #[must_use]
    pub const fn supports(self, direction: Direction) -> bool {
        match (self, direction) {
            (Self::RxOnly, Direction::Rx)
            | (Self::TxOnly, Direction::Tx)
            | (Self::Half | Self::Full, _) => true,
            (Self::RxOnly, Direction::Tx) | (Self::TxOnly, Direction::Rx) => false,
        }
    }

    #[must_use]
    pub const fn simultaneous(self) -> bool {
        matches!(self, Self::Full)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainStage {
    pub name: String,
    pub range: Range,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtraSetting {
    Bool {
        name: String,
        default: bool,
    },
    Range {
        name: String,
        range: Range,
        unit: String,
    },
    Enum {
        name: String,
        options: Vec<ArgumentOption>,
        default: String,
    },
    String {
        name: String,
        default: String,
    },
}

impl ExtraSetting {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Bool { name, .. }
            | Self::Range { name, .. }
            | Self::Enum { name, .. }
            | Self::String { name, .. } => name,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentType {
    Bool,
    Float,
    Int,
    String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArgumentOption {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ArgumentOption {
    #[must_use]
    pub fn plain(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArgumentInfo {
    pub key: String,
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    pub value_type: ArgumentType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<Range>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<ArgumentOption>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ChannelCapabilities {
    pub channel: u32,
    pub freq_ranges: Vec<Range>,
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rate_ranges: Vec<Range>,
    pub bandwidth_ranges: Vec<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    #[serde(default)]
    pub gain_mode: bool,
    #[serde(default)]
    pub dc_offset_mode: bool,
    #[serde(default)]
    pub iq_balance: bool,
    #[serde(default)]
    pub full_duplex: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_formats: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_stream_format: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_args: Vec<ArgumentInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frequency_args: Vec<ArgumentInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frequency_components: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<ArgumentInfo>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub info: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DirectionalCapabilities {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rx: Vec<ChannelCapabilities>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tx: Vec<ChannelCapabilities>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_settings: Vec<ArgumentInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clock_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub time_sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_source: Option<String>,
    #[serde(default)]
    pub hardware_time: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware_time_ns: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_clock_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hardware_info: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct StreamScope {
    #[serde(default)]
    pub tuning: bool,
    #[serde(default)]
    pub gain: bool,
    #[serde(default)]
    pub antenna: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Capabilities {
    pub freq_ranges: Vec<Range>,
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_range: Option<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub bandwidths: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraSetting>,
    #[serde(default)]
    pub ppm: bool,
    #[serde(default)]
    pub duplex: Duplex,
    #[serde(default = "one_stream")]
    pub rx_streams: u32,
    #[serde(default)]
    pub tx_streams: u32,
    #[serde(default)]
    pub per_stream: StreamScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directional: Option<DirectionalCapabilities>,
}

const fn one_stream() -> u32 {
    1
}

impl Capabilities {
    #[must_use]
    pub fn profile(&self) -> DeviceProfile {
        DeviceProfile {
            freq_ranges: self.freq_ranges.clone(),
            sample_rates: self.sample_rates.clone(),
            sample_rate_range: self.sample_rate_range,
            duplex: self.duplex,
            rx_streams: self.rx_streams,
            tx_streams: self.tx_streams,
            per_stream: self.per_stream,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainValue {
    pub stage: String,
    pub value_db: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExtraValue {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<StreamSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StreamSettings {
    pub stream: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
}

impl StreamSettings {
    fn merge_from(&mut self, delta: &StreamSettings) {
        if delta.center_hz.is_some() {
            self.center_hz = delta.center_hz;
        }
        if delta.antenna.is_some() {
            self.antenna.clone_from(&delta.antenna);
        }
        merge_gains(&mut self.gains, &delta.gains);
    }
}

fn merge_gains(gains: &mut Vec<GainValue>, delta: &[GainValue]) {
    for gain in delta {
        match gains.iter_mut().find(|g| g.stage == gain.stage) {
            Some(existing) => existing.value_db = gain.value_db,
            None => gains.push(gain.clone()),
        }
    }
}

impl DeviceSettings {
    pub fn merge_from(&mut self, delta: &DeviceSettings) {
        if delta.center_hz.is_some() {
            self.center_hz = delta.center_hz;
        }
        if delta.sample_rate.is_some() {
            self.sample_rate = delta.sample_rate;
        }
        if delta.ppm.is_some() {
            self.ppm = delta.ppm;
        }
        if delta.antenna.is_some() {
            self.antenna.clone_from(&delta.antenna);
        }
        if delta.bandwidth.is_some() {
            self.bandwidth = delta.bandwidth;
        }
        merge_gains(&mut self.gains, &delta.gains);
        for extra in &delta.extra {
            match self.extra.iter_mut().find(|e| e.name == extra.name) {
                Some(existing) => existing.value.clone_from(&extra.value),
                None => self.extra.push(extra.clone()),
            }
        }
        for stream in &delta.streams {
            match self.streams.iter_mut().find(|s| s.stream == stream.stream) {
                Some(existing) => existing.merge_from(stream),
                None => self.streams.push(stream.clone()),
            }
        }
    }

    #[must_use]
    pub fn for_stream(&self, index: u32, scope: &StreamScope) -> DeviceSettings {
        let mut resolved = DeviceSettings {
            streams: Vec::new(),
            ..self.clone()
        };
        let Some(overrides) = self.streams.iter().find(|s| s.stream == index) else {
            return resolved;
        };
        if scope.tuning && overrides.center_hz.is_some() {
            resolved.center_hz = overrides.center_hz;
        }
        if scope.gain {
            merge_gains(&mut resolved.gains, &overrides.gains);
        }
        if scope.antenna && overrides.antenna.is_some() {
            resolved.antenna.clone_from(&overrides.antenna);
        }
        resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(freq_ranges: Vec<Range>, sample_rates: Vec<f64>, duplex: Duplex) -> Capabilities {
        Capabilities {
            freq_ranges,
            sample_rates,
            sample_rate_range: None,
            gains: vec![GainStage {
                name: "TUNER".to_string(),
                range: Range {
                    min: 0.0,
                    max: 49.6,
                    step: None,
                },
            }],
            antennas: vec!["RX".to_string()],
            bandwidths: Vec::new(),
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
        }
    }

    fn range(min: f64, max: f64) -> Range {
        Range {
            min,
            max,
            step: None,
        }
    }

    #[test]
    fn a_profile_is_the_model_half_of_a_capability_set() {
        let mut full = caps(
            vec![range(24e6, 1.766e9)],
            vec![2.048e6, 2.4e6],
            Duplex::Half,
        );
        full.per_stream = StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
        };
        let profile = full.profile();
        assert_eq!(profile.freq_ranges, full.freq_ranges);
        assert_eq!(profile.sample_rates, full.sample_rates);
        assert_eq!(profile.duplex, Duplex::Half);
        assert_eq!(profile.rx_streams, 1);
        assert_eq!(profile.per_stream, full.per_stream);
    }

    #[test]
    fn a_profile_answers_reach_across_discontiguous_ranges() {
        let profile = caps(
            vec![range(0.5e6, 28.8e6), range(24e6, 1.766e9)],
            Vec::new(),
            Duplex::RxOnly,
        )
        .profile();
        assert!(profile.reaches(3.5e6), "the HF range");
        assert!(profile.reaches(1.09e9), "the tuner range");
        assert!(!profile.reaches(2.4e9));

        assert!(DeviceProfile::default().reaches(1.09e9));
    }

    #[test]
    fn a_rate_menu_is_exact_and_a_range_is_a_bound() {
        let menu = caps(Vec::new(), vec![1.024e6, 2.0e6, 2.4e6], Duplex::RxOnly).profile();
        assert!(menu.runs_at(2.0e6, 0.0), "ADS-B needs exactly 2 Msps");
        assert!(!menu.runs_at(2.2e6, 0.0));
        assert!(menu.runs_at(2.2e6, 0.1), "within tolerance");

        let mut continuous = caps(Vec::new(), Vec::new(), Duplex::RxOnly).profile();
        continuous.sample_rate_range = Some(range(2e6, 20e6));
        assert!(continuous.runs_at(8e6, 0.0));
        assert!(!continuous.runs_at(1e6, 0.0));

        assert!(DeviceProfile::default().runs_at(2.4e6, 0.0));
    }

    #[test]
    fn capabilities_default_to_one_receiver_and_no_transmitter() {
        let parsed: Capabilities = serde_json::from_str(
            r#"{"freq_ranges":[],"sample_rates":[],"gains":[],"antennas":[],"bandwidths":[]}"#,
        )
        .expect("a capability set from before this field existed");
        assert_eq!(parsed.rx_streams, 1);
        assert_eq!(parsed.tx_streams, 0);
        assert_eq!(parsed.duplex, Duplex::RxOnly);
        assert!(!parsed.duplex.supports(Direction::Tx));
    }

    #[test]
    fn identity_drops_the_probe_time_profile() {
        let probed = DeviceInfo {
            driver: "rtlsdr".to_string(),
            key: "00000001".to_string(),
            label: "RTL-SDR 00000001".to_string(),
            serial: Some("00000001".to_string()),
            profile: Some(caps(vec![range(24e6, 1.766e9)], Vec::new(), Duplex::RxOnly).profile()),
        };
        let stored = probed.identity();
        assert!(stored.profile.is_none());
        assert_eq!(stored.id(), probed.id());
        assert_eq!(stored.label, probed.label);
    }

    fn gain(stage: &str, value_db: f64) -> GainValue {
        GainValue {
            stage: stage.to_string(),
            value_db,
        }
    }

    fn extra(name: &str, value: serde_json::Value) -> ExtraValue {
        ExtraValue {
            name: name.to_string(),
            value,
        }
    }

    #[test]
    fn merge_patches_one_gain_stage_and_appends_new_ones() {
        let mut settings = DeviceSettings {
            gains: vec![gain("LNA", 16.0), gain("VGA", 20.0)],
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            gains: vec![gain("VGA", 30.0), gain("AMP", 14.0)],
            ..DeviceSettings::default()
        });
        assert_eq!(
            settings.gains,
            vec![gain("LNA", 16.0), gain("VGA", 30.0), gain("AMP", 14.0)]
        );
    }

    #[test]
    fn merge_patches_extra_by_name() {
        let mut settings = DeviceSettings {
            extra: vec![extra("bias_t", false.into()), extra("agc", true.into())],
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            extra: vec![
                extra("bias_t", true.into()),
                extra("offset_tuning", true.into()),
            ],
            ..DeviceSettings::default()
        });
        assert_eq!(
            settings.extra,
            vec![
                extra("bias_t", true.into()),
                extra("agc", true.into()),
                extra("offset_tuning", true.into()),
            ]
        );
    }

    #[test]
    fn merge_patches_streams_by_index_and_their_gains_by_stage() {
        let mut settings = DeviceSettings {
            streams: vec![StreamSettings {
                stream: 0,
                center_hz: Some(100_000_000.0),
                gains: vec![gain("LNA", 16.0), gain("VGA", 20.0)],
                antenna: None,
            }],
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            streams: vec![
                StreamSettings {
                    stream: 0,
                    gains: vec![gain("VGA", 30.0), gain("AMP", 14.0)],
                    antenna: Some("RX2".to_string()),
                    ..StreamSettings::default()
                },
                StreamSettings {
                    stream: 1,
                    center_hz: Some(433_920_000.0),
                    ..StreamSettings::default()
                },
            ],
            ..DeviceSettings::default()
        });
        assert_eq!(
            settings.streams,
            vec![
                StreamSettings {
                    stream: 0,
                    center_hz: Some(100_000_000.0),
                    gains: vec![gain("LNA", 16.0), gain("VGA", 30.0), gain("AMP", 14.0)],
                    antenna: Some("RX2".to_string()),
                },
                StreamSettings {
                    stream: 1,
                    center_hz: Some(433_920_000.0),
                    ..StreamSettings::default()
                },
            ]
        );
    }

    #[test]
    fn for_stream_resolves_each_setting_by_its_own_scope_flag() {
        let settings = DeviceSettings {
            center_hz: Some(100_000_000.0),
            antenna: Some("RX".to_string()),
            gains: vec![gain("LNA", 16.0), gain("VGA", 20.0)],
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(433_920_000.0),
                gains: vec![gain("VGA", 30.0)],
                antenna: Some("RX2".to_string()),
            }],
            ..DeviceSettings::default()
        };

        let tuning_only = StreamScope {
            tuning: true,
            gain: false,
            antenna: false,
        };
        let lane = settings.for_stream(1, &tuning_only);
        assert_eq!(lane.center_hz, Some(433_920_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0), gain("VGA", 20.0)]);
        assert_eq!(lane.antenna.as_deref(), Some("RX"));
        assert!(
            lane.streams.is_empty(),
            "a lane's view carries no overrides"
        );

        let gain_only = StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
        };
        let lane = settings.for_stream(1, &gain_only);
        assert_eq!(lane.center_hz, Some(100_000_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0), gain("VGA", 30.0)]);
        assert_eq!(lane.antenna.as_deref(), Some("RX"));

        let antenna_only = StreamScope {
            tuning: false,
            gain: false,
            antenna: true,
        };
        let lane = settings.for_stream(1, &antenna_only);
        assert_eq!(lane.center_hz, Some(100_000_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0), gain("VGA", 20.0)]);
        assert_eq!(lane.antenna.as_deref(), Some("RX2"));
    }

    #[test]
    fn for_stream_without_an_entry_is_the_radio_wide_settings() {
        let settings = DeviceSettings {
            center_hz: Some(100_000_000.0),
            gains: vec![gain("LNA", 16.0)],
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(433_920_000.0),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        let scope = StreamScope {
            tuning: true,
            gain: true,
            antenna: true,
        };
        let lane = settings.for_stream(0, &scope);
        assert_eq!(lane.center_hz, Some(100_000_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0)]);
        assert_eq!(lane.antenna, None);
    }

    #[test]
    fn a_payload_without_streams_or_per_stream_is_a_single_stream_radio() {
        let parsed: Capabilities = serde_json::from_str(
            r#"{"freq_ranges":[],"sample_rates":[],"gains":[],"antennas":[],"bandwidths":[]}"#,
        )
        .expect("a capability set from before per_stream existed");
        assert_eq!(parsed.per_stream, StreamScope::default());
        assert_eq!(parsed.profile().per_stream, StreamScope::default());

        let settings: DeviceSettings = serde_json::from_str(r#"{"center_hz":100000000.0}"#)
            .expect("a settings payload from before streams existed");
        assert!(settings.streams.is_empty());
        assert_eq!(
            settings.for_stream(0, &parsed.per_stream).center_hz,
            Some(100_000_000.0)
        );

        let json = serde_json::to_value(&settings).expect("serialize");
        assert!(json.get("streams").is_none());
    }

    #[test]
    fn merge_overlays_bandwidth_and_leaves_absent_fields() {
        let mut settings = DeviceSettings {
            center_hz: Some(100_000_000.0),
            bandwidth: Some(2_500_000.0),
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            bandwidth: Some(1_750_000.0),
            ..DeviceSettings::default()
        });
        assert_eq!(settings.center_hz, Some(100_000_000.0));
        assert_eq!(settings.bandwidth, Some(1_750_000.0));

        settings.merge_from(&DeviceSettings::default());
        assert_eq!(settings.bandwidth, Some(1_750_000.0));
    }
}
