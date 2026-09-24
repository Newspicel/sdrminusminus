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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rate_ranges: Vec<Range>,
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

    /// The rate this radio would run to cover a `want` Hz window.
    ///
    /// The lowest rate at or above `want` wins, because a window wider than asked for costs work
    /// but decimates back down while a narrower one loses signal. A radio that cannot reach
    /// `want` at all runs the widest window it has.
    #[must_use]
    pub fn rate_for(&self, want: f64) -> Option<f64> {
        self.lowest_rate_in(want, f64::INFINITY)
            .or_else(|| self.highest_rate_in(0.0, want))
    }

    fn lowest_rate_in(&self, min: f64, max: f64) -> Option<f64> {
        self.rates_in(min, max).min_by(f64::total_cmp)
    }

    fn highest_rate_in(&self, min: f64, max: f64) -> Option<f64> {
        self.rates_in(min, max).max_by(f64::total_cmp)
    }

    /// The edges of what the radio offers inside `[min, max]`. A declared window contributes the
    /// ends of its overlap and overrules the menu; a radio that declares neither is taken at its
    /// word for whatever it is asked.
    fn rates_in(&self, min: f64, max: f64) -> impl Iterator<Item = f64> {
        let unbounded = self.sample_rate_ranges.is_empty() && self.sample_rates.is_empty();
        let windows = self
            .sample_rate_ranges
            .iter()
            .filter(move |range| range.min <= max && range.max >= min)
            .flat_map(move |range| [range.min.max(min), range.max.min(max)]);
        let menu = self
            .sample_rate_ranges
            .is_empty()
            .then(|| self.sample_rates.iter().copied())
            .into_iter()
            .flatten();
        windows
            .chain(menu)
            .chain(unbounded.then_some(min))
            .filter(move |rate| (min..=max).contains(rate) && rate.is_finite())
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

/// Whether a source lands its own DC term at the tuned centre, and whether the blocker that
/// removes it starts on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DcArtifact {
    /// Hardware the engine does not recognise. The blocker is offered and starts off, because
    /// only the operator knows what the front end already does for itself.
    #[default]
    Operator,
    /// A zero-IF front end known to land an impulse at the centre. The blocker starts on.
    Managed,
    /// A source with no analog front end, such as a recording. There is no term to remove.
    None,
}

impl DcArtifact {
    #[must_use]
    pub const fn is_managed(self) -> bool {
        matches!(self, Self::Managed)
    }

    #[must_use]
    pub const fn is_operator(self) -> bool {
        matches!(self, Self::Operator)
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

impl Range {
    #[must_use]
    pub fn holds(&self, value: f64) -> bool {
        self.min <= value && value <= self.max
    }
}

fn reaches(ranges: &[Range], hz: f64) -> bool {
    ranges.is_empty() || any_range_holds(ranges, hz)
}

fn supported_gains(gains: &[GainValue], capabilities: &Capabilities) -> Vec<GainValue> {
    gains
        .iter()
        .filter(|value| {
            capabilities
                .gains
                .iter()
                .any(|stage| stage.name == value.stage)
        })
        .cloned()
        .collect()
}

/// Whether a value lies in any of `ranges`. An empty list holds nothing, so callers that mean
/// "unconstrained" have to say so themselves.
#[must_use]
pub fn any_range_holds(ranges: &[Range], value: f64) -> bool {
    ranges.iter().any(|range| range.holds(value))
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GainKind {
    Lna,
    Mixer,
    Vga,
    If,
    Rf,
    Tuner,
    Amp,
    Attenuator,
    Tx,
    Other,
}

impl GainKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Lna => "LNA",
            Self::Mixer => "MIX",
            Self::Vga => "VGA",
            Self::If => "IF",
            Self::Rf => "RF",
            Self::Tuner => "TUNER",
            Self::Amp => "AMP",
            Self::Attenuator => "ATT",
            Self::Tx => "TX",
            Self::Other => "GAIN",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let upper = name.trim().to_ascii_uppercase();
        match upper.as_str() {
            "LNA" => Self::Lna,
            "MIX" | "MIXER" | "TIA" => Self::Mixer,
            "VGA" | "PGA" | "BB" | "IFGR" => Self::Vga,
            "IF" => Self::If,
            "RF" | "RFGR" => Self::Rf,
            "TUNER" | "GAIN" | "FULL" | "RX" => Self::Tuner,
            "AMP" | "PREAMP" => Self::Amp,
            "ATT" | "ATTENUATOR" | "ATTEN" => Self::Attenuator,
            "TX" | "PAD" => Self::Tx,
            _ => Self::Other,
        }
    }

    #[must_use]
    pub const fn is_switch(self) -> bool {
        matches!(self, Self::Amp)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GainUnit {
    #[default]
    Db,
    Index,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainStage {
    pub name: String,
    pub kind: GainKind,
    #[serde(default)]
    pub unit: GainUnit,
    pub range: Range,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<f64>,
}

impl GainStage {
    #[must_use]
    pub fn new(kind: GainKind, range: Range) -> Self {
        Self::named(kind.name(), kind, range)
    }

    #[must_use]
    pub fn named(name: impl Into<String>, kind: GainKind, range: Range) -> Self {
        Self {
            name: name.into(),
            kind,
            unit: GainUnit::Db,
            range,
            values: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_values(mut self, values: Vec<f64>) -> Self {
        self.values = values;
        self
    }

    #[must_use]
    pub const fn with_unit(mut self, unit: GainUnit) -> Self {
        self.unit = unit;
        self
    }

    #[must_use]
    pub fn setting_count(&self) -> usize {
        if !self.values.is_empty() {
            return self.values.len();
        }
        match self.range.step.filter(|step| *step > 0.0) {
            Some(step) => ((self.range.max - self.range.min) / step).round() as usize + 1,
            None => usize::MAX,
        }
    }

    #[must_use]
    pub fn is_switch(&self) -> bool {
        self.kind.is_switch()
    }

    #[must_use]
    pub fn off(&self) -> f64 {
        self.range.min
    }

    #[must_use]
    pub fn on(&self) -> f64 {
        self.range.max
    }

    /// The nearest setting the hardware can hold. Ties take the lower one, so snapping never
    /// raises gain past what was asked for.
    #[must_use]
    pub fn snap(&self, value_db: f64) -> f64 {
        let clamped = value_db.clamp(self.range.min, self.range.max);
        if !self.values.is_empty() {
            return self
                .values
                .iter()
                .copied()
                .min_by(|a, b| {
                    (a - clamped)
                        .abs()
                        .total_cmp(&(b - clamped).abs())
                        .then(a.total_cmp(b))
                })
                .unwrap_or(clamped);
        }
        match self.range.step.filter(|step| *step > 0.0) {
            Some(step) => (self.range.min + ((clamped - self.range.min) / step).round() * step)
                .clamp(self.range.min, self.range.max),
            None => clamped,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExtraSetting {
    Bool {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        default: bool,
    },
    Range {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        range: Range,
        unit: String,
    },
    Enum {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        options: Vec<ArgumentOption>,
        default: String,
    },
    String {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
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

    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Bool { label, .. }
            | Self::Range { label, .. }
            | Self::Enum { label, .. }
            | Self::String { label, .. } => label.as_deref(),
        }
    }

    #[must_use]
    pub fn bool(name: impl Into<String>, label: impl Into<String>, default: bool) -> Self {
        Self::Bool {
            name: name.into(),
            label: Some(label.into()),
            default,
        }
    }

    #[must_use]
    pub fn range(
        name: impl Into<String>,
        label: impl Into<String>,
        range: Range,
        unit: impl Into<String>,
    ) -> Self {
        Self::Range {
            name: name.into(),
            label: Some(label.into()),
            range,
            unit: unit.into(),
        }
    }

    #[must_use]
    pub fn choice(
        name: impl Into<String>,
        label: impl Into<String>,
        options: Vec<ArgumentOption>,
        default: impl Into<String>,
    ) -> Self {
        Self::Enum {
            name: name.into(),
            label: Some(label.into()),
            options,
            default: default.into(),
        }
    }

    #[must_use]
    pub fn text(
        name: impl Into<String>,
        label: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        Self::String {
            name: name.into(),
            label: Some(label.into()),
            default: default.into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Agc {
    #[default]
    None,
    Switch,
    Modes {
        options: Vec<ArgumentOption>,
    },
}

impl Agc {
    #[must_use]
    pub fn offered(&self) -> bool {
        !matches!(self, Self::None)
    }

    #[must_use]
    pub fn admits(&self, setting: &AgcSetting) -> bool {
        match self {
            Self::None => false,
            Self::Switch => setting.mode.is_none(),
            Self::Modes { options } => setting
                .mode
                .as_ref()
                .is_none_or(|mode| options.iter().any(|option| option.value == *mode)),
        }
    }

    #[must_use]
    pub fn first_mode(&self) -> Option<&str> {
        match self {
            Self::Modes { options } => options.first().map(|option| option.value.as_str()),
            Self::None | Self::Switch => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AgcGain {
    pub stream: u32,
    pub value_db: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AgcSetting {
    pub on: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl AgcSetting {
    #[must_use]
    pub const fn off() -> Self {
        Self {
            on: false,
            mode: None,
        }
    }

    #[must_use]
    pub const fn switched(on: bool) -> Self {
        Self { on, mode: None }
    }

    #[must_use]
    pub fn in_mode(on: bool, mode: impl Into<String>) -> Self {
        Self {
            on,
            mode: Some(mode.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BandwidthSetting {
    Auto,
    Manual { hz: f64 },
}

impl BandwidthSetting {
    #[must_use]
    pub const fn hz(self) -> Option<f64> {
        match self {
            Self::Auto => None,
            Self::Manual { hz } => Some(hz),
        }
    }

    #[must_use]
    pub const fn is_auto(self) -> bool {
        matches!(self, Self::Auto)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BandwidthWire {
    Tagged(BandwidthTagged),
    Hz(f64),
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BandwidthTagged {
    Auto,
    Manual { hz: f64 },
}

impl<'de> Deserialize<'de> for BandwidthSetting {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match BandwidthWire::deserialize(deserializer)? {
            BandwidthWire::Tagged(BandwidthTagged::Auto) => Self::Auto,
            BandwidthWire::Tagged(BandwidthTagged::Manual { hz }) => Self::Manual { hz },
            BandwidthWire::Hz(hz) if hz > 0.0 => Self::Manual { hz },
            BandwidthWire::Hz(_) => Self::Auto,
        })
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
    #[serde(default)]
    pub agc: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Capabilities {
    pub freq_ranges: Vec<Range>,
    pub sample_rates: Vec<f64>,
    /// Continuous windows the radio resamples across. A radio with holes in its rate coverage,
    /// the RTL2832U aliases between 300 kHz and 900 kHz, needs more than one, which is why this
    /// is a list and not the single range it replaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rate_ranges: Vec<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub bandwidths: Vec<f64>,
    /// Continuous analog filter widths, for hardware whose IF filter is not a discrete menu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bandwidth_ranges: Vec<Range>,
    #[serde(default)]
    pub bandwidth_auto: bool,
    #[serde(default)]
    pub bias_tee: bool,
    #[serde(default)]
    pub agc: Agc,
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
    #[serde(default)]
    pub dc_artifact: DcArtifact,
    /// Whether the radio sweeps in its own firmware, delivering blocks stamped with the frequency
    /// each was taken at instead of a stream at one tuning.
    #[serde(default)]
    pub hardware_sweep: bool,
    #[serde(default)]
    pub coherence: Coherence,
    /// Whether the radio can switch a calibration reference into every lane at once. An array
    /// that carries its own is calibrated without an operator reaching for a splitter, and
    /// without one it has to be told what to solve against.
    #[serde(default)]
    pub noise_source: bool,
}

/// How much of the relationship between two of a radio's receive lanes survives calibration.
///
/// A shared clock alone fixes the sample rate, so a measured delay between lanes stays true; the
/// separate synthesizers still come up at an arbitrary phase after every retune. Only a shared
/// local oscillator makes inter-lane phase, and therefore a bearing, mean anything.
#[derive(
    Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Coherence {
    #[default]
    None,
    TimeSync,
    PhaseCoherent,
}

impl Coherence {
    #[must_use]
    pub const fn has_time(self) -> bool {
        matches!(self, Self::TimeSync | Self::PhaseCoherent)
    }

    #[must_use]
    pub const fn has_phase(self) -> bool {
        matches!(self, Self::PhaseCoherent)
    }
}

const fn one_stream() -> u32 {
    1
}

impl Capabilities {
    #[must_use]
    pub fn has_filter(&self) -> bool {
        self.bandwidth_auto || !self.bandwidths.is_empty() || !self.bandwidth_ranges.is_empty()
    }

    #[must_use]
    pub fn admits_bandwidth(&self, bandwidth: BandwidthSetting) -> bool {
        match bandwidth {
            BandwidthSetting::Auto => self.bandwidth_auto,
            BandwidthSetting::Manual { hz } => {
                self.bandwidths.contains(&hz) || any_range_holds(&self.bandwidth_ranges, hz)
            }
        }
    }

    #[must_use]
    pub fn stage(&self, name: &str) -> Option<&GainStage> {
        self.gains.iter().find(|stage| stage.name == name)
    }

    /// Whether the radio can be moved at all; a recording plays where it was taken, so a
    /// converter offset means nothing to it.
    #[must_use]
    pub fn tunes(&self) -> bool {
        self.freq_ranges.is_empty() || self.freq_ranges.iter().any(|r| r.min < r.max)
    }

    /// The same radio seen through a converter: every frequency it reaches, moved by the
    /// converter's local oscillator.
    #[must_use]
    pub fn shifted_by(&self, offset_hz: f64) -> Capabilities {
        if offset_hz == 0.0 {
            return self.clone();
        }
        let mut shifted = self.clone();
        for range in &mut shifted.freq_ranges {
            range.min += offset_hz;
            range.max += offset_hz;
        }
        shifted
    }

    #[must_use]
    pub fn profile(&self) -> DeviceProfile {
        DeviceProfile {
            freq_ranges: self.freq_ranges.clone(),
            sample_rates: self.sample_rates.clone(),
            sample_rate_ranges: self.sample_rate_ranges.clone(),
            duplex: self.duplex,
            rx_streams: self.rx_streams,
            tx_streams: self.tx_streams,
            per_stream: self.per_stream,
        }
    }
}

pub const ARRAY_DRIVER_ID: &str = "array";
pub const RECORDING_DRIVER_ID: &str = "recording";
pub const SIGGEN_DRIVER_ID: &str = "siggen";
pub const MAX_ARRAY_MEMBERS: usize = 16;
pub const MAX_ARRAY_KEY_LEN: usize = 64;
pub const MAX_RECORDING_STEM_LEN: usize = 200;

#[must_use]
pub fn recording_stem_valid(stem: &str) -> bool {
    !stem.is_empty()
        && stem.len() <= MAX_RECORDING_STEM_LEN
        && stem != "."
        && stem != ".."
        && !stem.contains('/')
        && !stem.contains('\\')
        && !stem.contains('\0')
}

/// A bank of separate radios the operator has wired to one clock and wants treated as one.
///
/// Nothing here is discovered: which radios belong together, and whether their clock alone is
/// shared or their synthesizer too, is a fact about the bench that only the operator knows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ArrayDefinition {
    pub key: String,
    pub label: String,
    /// Member device ids, in the order their lanes are numbered.
    pub members: Vec<String>,
    pub coherence: Coherence,
    /// Whether every member is tuned together. A bank whose members can be tuned apart is a bank
    /// of receivers; only one tuned together is an array.
    #[serde(default = "yes")]
    pub shared_tuning: bool,
}

const fn yes() -> bool {
    true
}

impl ArrayDefinition {
    #[must_use]
    pub fn id(&self) -> String {
        format!("{ARRAY_DRIVER_ID}:{}", self.key)
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        !self.key.is_empty()
            && self.key.len() <= MAX_ARRAY_KEY_LEN
            && self
                .key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            && (2..=MAX_ARRAY_MEMBERS).contains(&self.members.len())
            && self.members.iter().all(|member| member.contains(':'))
            && {
                let mut seen = self.members.clone();
                seen.sort_unstable();
                seen.dedup();
                seen.len() == self.members.len()
            }
            && self.coherence != Coherence::None
    }

    #[must_use]
    pub fn per_stream(&self) -> StreamScope {
        StreamScope {
            tuning: !self.shared_tuning,
            gain: true,
            antenna: true,
            agc: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainValue {
    pub stage: String,
    pub value_db: f64,
}

impl GainValue {
    #[must_use]
    pub fn new(kind: GainKind, value_db: f64) -> Self {
        Self {
            stage: kind.name().to_string(),
            value_db,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExtraValue {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tuning {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ppm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<BandwidthSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc_block: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bias_tee: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agc: Option<AgcSetting>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<Tuning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub antenna: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agc: Option<AgcSetting>,
}

impl StreamSettings {
    fn merge_from(&mut self, delta: &StreamSettings) {
        if delta.agc.is_some() {
            self.agc.clone_from(&delta.agc);
        }
        if delta.center_hz.is_some() {
            self.center_hz = delta.center_hz;
        }
        if delta.tuning.is_some() {
            self.tuning = delta.tuning;
        }
        if delta.antenna.is_some() {
            self.antenna.clone_from(&delta.antenna);
        }
        merge_gains(&mut self.gains, &delta.gains);
    }
}

fn shift_centers(settings: &mut DeviceSettings, by_hz: f64) {
    if by_hz == 0.0 {
        return;
    }
    settings.center_hz = settings.center_hz.map(|hz| hz + by_hz);
    for stream in &mut settings.streams {
        stream.center_hz = stream.center_hz.map(|hz| hz + by_hz);
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
        if delta.tuning.is_some() {
            self.tuning = delta.tuning;
        }
        if delta.sample_rate.is_some() {
            self.sample_rate = delta.sample_rate;
        }
        if delta.ppm.is_some() {
            self.ppm = delta.ppm;
        }
        if delta.offset_hz.is_some() {
            self.offset_hz = delta.offset_hz;
        }
        if delta.antenna.is_some() {
            self.antenna.clone_from(&delta.antenna);
        }
        if delta.bandwidth.is_some() {
            self.bandwidth = delta.bandwidth;
        }
        if delta.dc_block.is_some() {
            self.dc_block = delta.dc_block;
        }
        if delta.bias_tee.is_some() {
            self.bias_tee = delta.bias_tee;
        }
        if delta.agc.is_some() {
            self.agc.clone_from(&delta.agc);
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

    /// The part of these settings a radio can actually take.
    ///
    /// Settings are remembered per patch node, not per radio, so the same stored values are
    /// replayed onto whatever is bound there next. A recording has no bias tee and a receiver has
    /// no other receiver's gain stages; dropping what the radio never offered lets the rest of the
    /// settings land instead of the whole patch being refused.
    #[must_use]
    pub fn supported_by(&self, capabilities: &Capabilities) -> DeviceSettings {
        let scope = capabilities.per_stream;
        let offset_hz = self.offset_hz.filter(|_| capabilities.tunes());
        let offset = offset_hz.unwrap_or(0.0);
        DeviceSettings {
            center_hz: self
                .center_hz
                .filter(|hz| reaches(&capabilities.freq_ranges, *hz - offset)),
            tuning: self.tuning,
            sample_rate: self.sample_rate.filter(|rate| {
                capabilities.sample_rates.contains(rate)
                    || any_range_holds(&capabilities.sample_rate_ranges, *rate)
            }),
            ppm: self.ppm.filter(|_| capabilities.ppm),
            offset_hz,
            antenna: self
                .antenna
                .clone()
                .filter(|antenna| capabilities.antennas.contains(antenna)),
            bandwidth: self
                .bandwidth
                .filter(|bandwidth| capabilities.admits_bandwidth(*bandwidth)),
            dc_block: self
                .dc_block
                .filter(|_| capabilities.dc_artifact != DcArtifact::None),
            bias_tee: self.bias_tee.filter(|_| capabilities.bias_tee),
            agc: self.agc.clone().filter(|agc| capabilities.agc.admits(agc)),
            gains: supported_gains(&self.gains, capabilities),
            extra: self
                .extra
                .iter()
                .filter(|value| capabilities.extra.iter().any(|e| e.name() == value.name))
                .cloned()
                .collect(),
            streams: self
                .streams
                .iter()
                .filter(|stream| stream.stream < capabilities.rx_streams)
                .map(|stream| StreamSettings {
                    stream: stream.stream,
                    center_hz: stream.center_hz.filter(|hz| {
                        scope.tuning && reaches(&capabilities.freq_ranges, *hz - offset)
                    }),
                    tuning: stream.tuning.filter(|_| scope.tuning),
                    gains: if scope.gain {
                        supported_gains(&stream.gains, capabilities)
                    } else {
                        Vec::new()
                    },
                    antenna: stream
                        .antenna
                        .clone()
                        .filter(|a| scope.antenna && capabilities.antennas.contains(a)),
                    agc: None,
                })
                .filter(|stream| {
                    stream.center_hz.is_some()
                        || stream.tuning.is_some()
                        || !stream.gains.is_empty()
                        || stream.antenna.is_some()
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn tunes_itself(&self) -> bool {
        self.tuning.unwrap_or_default() == Tuning::Auto
    }

    #[must_use]
    pub fn gain(&self, stage: &str) -> Option<f64> {
        self.gains
            .iter()
            .find(|gain| gain.stage == stage)
            .map(|gain| gain.value_db)
    }

    #[must_use]
    pub fn agc_on(&self) -> bool {
        self.agc.as_ref().is_some_and(|agc| agc.on)
    }

    #[must_use]
    pub fn bandwidth_hz(&self) -> Option<f64> {
        self.bandwidth.and_then(BandwidthSetting::hz)
    }

    /// The local oscillator of a converter in front of the radio: what is shown is what the
    /// radio is tuned to plus this.
    #[must_use]
    pub fn offset(&self) -> f64 {
        self.offset_hz.unwrap_or(0.0)
    }

    /// The settings the driver is handed: the front end's own controls stay with the engine and
    /// every frequency is the one the radio itself has to reach.
    #[must_use]
    pub fn to_hardware(&self) -> DeviceSettings {
        let mut hardware = self.clone();
        hardware.dc_block = None;
        hardware.tuning = None;
        hardware.offset_hz = None;
        shift_centers(&mut hardware, -self.offset());
        for stream in &mut hardware.streams {
            stream.tuning = None;
        }
        hardware
    }

    /// What a radio reports back, seen through the converter in front of it.
    #[must_use]
    pub fn from_hardware(mut hardware: DeviceSettings, offset_hz: Option<f64>) -> DeviceSettings {
        shift_centers(&mut hardware, offset_hz.unwrap_or(0.0));
        hardware.offset_hz = offset_hz;
        hardware
    }

    /// Gives a delta the converter offset it is applied under. A delta that moves the offset
    /// without naming a frequency leaves the radio where it is, so what is shown moves instead.
    pub fn carry_offset(&self, delta: &mut DeviceSettings) {
        let Some(offset_hz) = delta.offset_hz else {
            delta.offset_hz = self.offset_hz;
            return;
        };
        let moved = offset_hz - self.offset();
        if delta.center_hz.is_none() {
            delta.center_hz = self.center_hz.map(|hz| hz + moved);
        }
        for stream in &self.streams {
            let Some(hz) = stream.center_hz else {
                continue;
            };
            match delta.streams.iter_mut().find(|s| s.stream == stream.stream) {
                Some(own) if own.center_hz.is_some() => {}
                Some(own) => own.center_hz = Some(hz + moved),
                None => delta.streams.push(StreamSettings {
                    stream: stream.stream,
                    center_hz: Some(hz + moved),
                    ..StreamSettings::default()
                }),
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
        if scope.tuning && overrides.tuning.is_some() {
            resolved.tuning = overrides.tuning;
        }
        if scope.gain {
            merge_gains(&mut resolved.gains, &overrides.gains);
        }
        if scope.antenna && overrides.antenna.is_some() {
            resolved.antenna.clone_from(&overrides.antenna);
        }
        if scope.agc && overrides.agc.is_some() {
            resolved.agc.clone_from(&overrides.agc);
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
            sample_rate_ranges: Vec::new(),
            gains: vec![GainStage::new(
                GainKind::Tuner,
                Range {
                    min: 0.0,
                    max: 49.6,
                    step: None,
                },
            )],
            antennas: vec!["RX".to_string()],
            bandwidths: Vec::new(),
            bandwidth_ranges: Vec::new(),
            bandwidth_auto: false,
            bias_tee: false,
            agc: Agc::None,
            extra: Vec::new(),
            ppm: false,
            duplex,
            rx_streams: 1,
            tx_streams: 0,
            per_stream: StreamScope::default(),
            directional: None,
            dc_artifact: DcArtifact::Operator,
            hardware_sweep: false,
            coherence: Coherence::None,
            noise_source: false,
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
            agc: false,
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

    fn stage(range: Range, values: Vec<f64>) -> GainStage {
        GainStage::new(GainKind::Lna, range).with_values(values)
    }

    #[test]
    fn a_stage_without_a_step_or_a_table_only_clamps() {
        let plain = stage(range(0.0, 10.0), Vec::new());
        assert_eq!(plain.snap(3.7), 3.7);
        assert_eq!(plain.snap(11.0), 10.0);
        assert_eq!(plain.snap(-1.0), 0.0);
        assert!(!plain.is_switch());
    }

    #[test]
    fn a_stepped_stage_lands_on_the_grid() {
        let mut stepped = range(0.0, 40.0);
        stepped.step = Some(8.0);
        let stage = stage(stepped, Vec::new());
        assert_eq!(stage.snap(13.0), 16.0);
        assert_eq!(stage.snap(12.0), 16.0);
        assert_eq!(stage.snap(-5.0), 0.0);
        assert_eq!(stage.snap(1_000.0), 40.0);
        assert!(!stage.is_switch(), "five settings is not a switch");
    }

    #[test]
    fn a_tabled_stage_lands_only_on_a_real_setting() {
        let table = stage(range(0.0, 49.6), vec![0.0, 9.0, 14.0, 19.7, 20.7, 49.6]);
        assert_eq!(table.snap(20.0), 19.7, "nearest, and ties take the lower");
        assert_eq!(table.snap(19.7), 19.7);
        assert_eq!(table.snap(-3.0), 0.0);
        assert_eq!(table.snap(1_000.0), 49.6);
        assert_eq!(table.snap(11.5), 9.0, "exactly between 9.0 and 14.0");
        assert!(!table.is_switch());
    }

    #[test]
    fn only_an_amp_is_a_switch() {
        let mut stepped = range(0.0, 14.0);
        stepped.step = Some(14.0);
        assert!(GainStage::new(GainKind::Amp, stepped).is_switch());
        assert!(!stage(stepped, Vec::new()).is_switch());
        assert_eq!(GainStage::new(GainKind::Amp, stepped).setting_count(), 2);
        assert_eq!(GainStage::new(GainKind::Amp, stepped).on(), 14.0);
    }

    #[test]
    fn a_stage_name_is_its_kind() {
        let stage = GainStage::new(GainKind::Mixer, range(0.0, 15.0));
        assert_eq!(stage.name, "MIX");
        assert_eq!(GainKind::from_name("Mixer"), GainKind::Mixer);
        assert_eq!(GainKind::from_name("tuner"), GainKind::Tuner);
        assert_eq!(GainKind::from_name("PGA"), GainKind::Vga);
        assert_eq!(GainKind::from_name("weird"), GainKind::Other);
        assert_eq!(GainValue::new(GainKind::Attenuator, -6.0).stage, "ATT");
    }

    #[test]
    fn a_bandwidth_reads_a_bare_number_from_before_it_was_tagged() {
        let manual: BandwidthSetting = serde_json::from_str("1750000.0").expect("bare hz");
        assert_eq!(manual, BandwidthSetting::Manual { hz: 1_750_000.0 });
        let auto: BandwidthSetting = serde_json::from_str("0").expect("bare zero");
        assert_eq!(auto, BandwidthSetting::Auto);
        let tagged: BandwidthSetting = serde_json::from_str(r#"{"kind":"auto"}"#).expect("tagged");
        assert_eq!(tagged, BandwidthSetting::Auto);
        let json = serde_json::to_string(&BandwidthSetting::Manual { hz: 5e6 }).expect("serialize");
        assert_eq!(json, r#"{"kind":"manual","hz":5000000.0}"#);
    }

    #[test]
    fn agc_admits_only_the_modes_it_offers() {
        assert!(!Agc::None.admits(&AgcSetting::switched(true)));
        assert!(Agc::Switch.admits(&AgcSetting::switched(true)));
        assert!(!Agc::Switch.admits(&AgcSetting::in_mode(true, "fast")));
        let modes = Agc::Modes {
            options: vec![ArgumentOption::plain("fast"), ArgumentOption::plain("slow")],
        };
        assert!(modes.admits(&AgcSetting::in_mode(true, "slow")));
        assert!(modes.admits(&AgcSetting::off()));
        assert!(!modes.admits(&AgcSetting::in_mode(true, "hybrid")));
        assert_eq!(modes.first_mode(), Some("fast"));
    }

    #[test]
    fn a_radio_that_cannot_go_narrow_enough_runs_wide_instead() {
        let menu = caps(Vec::new(), vec![1.024e6, 2.0e6, 2.4e6], Duplex::RxOnly).profile();
        assert_eq!(menu.rate_for(2.0e6), Some(2.0e6), "offered as asked");
        assert_eq!(menu.rate_for(2.2e6), Some(2.4e6), "the next one up");
        assert_eq!(
            menu.rate_for(250e3),
            Some(1.024e6),
            "the slowest the radio has, decimated back down"
        );
        assert_eq!(menu.rate_for(3e6), Some(2.4e6), "the widest it has");

        let mut continuous = caps(Vec::new(), Vec::new(), Duplex::RxOnly).profile();
        continuous.sample_rate_ranges = vec![range(2e6, 20e6)];
        assert_eq!(continuous.rate_for(8e6), Some(8e6));
        assert_eq!(
            continuous.rate_for(250e3),
            Some(2e6),
            "a transceiver with a 2 MS/s floor still runs a narrow template"
        );
        assert_eq!(continuous.rate_for(24e6), Some(20e6));

        assert_eq!(
            DeviceProfile::default().rate_for(2.4e6),
            Some(2.4e6),
            "a radio that advertises nothing is taken at its word"
        );
    }

    #[test]
    fn a_gap_between_two_windows_is_not_a_rate_the_radio_runs_at() {
        let mut windows = caps(
            Vec::new(),
            vec![250_000.0, 1_024_000.0, 2_048_000.0, 3_200_000.0],
            Duplex::RxOnly,
        )
        .profile();
        windows.sample_rate_ranges = vec![range(225_001.0, 300_000.0), range(900_001.0, 3.2e6)];
        assert_eq!(windows.rate_for(250_000.0), Some(250_000.0));
        assert_eq!(windows.rate_for(1.8e6), Some(1.8e6));
        assert_eq!(
            windows.rate_for(500_000.0),
            Some(900_001.0),
            "the RTL2832U aliases between the two windows, so the next window up runs it"
        );
        for rate in &windows.sample_rates {
            assert!(
                any_range_holds(&windows.sample_rate_ranges, *rate),
                "{rate} is offered in the menu but sits in no window"
            );
        }
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
                tuning: None,
                gains: vec![gain("LNA", 16.0), gain("VGA", 20.0)],
                antenna: None,
                agc: None,
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
                    tuning: None,
                    gains: vec![gain("LNA", 16.0), gain("VGA", 30.0), gain("AMP", 14.0)],
                    antenna: Some("RX2".to_string()),
                    agc: None,
                },
                StreamSettings {
                    stream: 1,
                    center_hz: Some(433_920_000.0),
                    ..StreamSettings::default()
                },
            ]
        );
    }

    const RATE: f64 = 2_400_000.0;

    fn tuner(min: f64, max: f64) -> Capabilities {
        caps(vec![range(min, max)], vec![RATE], Duplex::RxOnly)
    }

    fn tuned(center_hz: f64) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(center_hz),
            sample_rate: Some(RATE),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn hardware_never_sees_the_front_end_controls() {
        let mut settings = tuned(100e6);
        settings.dc_block = Some(true);
        settings.tuning = Some(Tuning::Manual);
        settings.streams = vec![StreamSettings {
            stream: 1,
            center_hz: Some(433_920_000.0),
            tuning: Some(Tuning::Manual),
            ..StreamSettings::default()
        }];
        let hardware = settings.to_hardware();
        assert_eq!(hardware.center_hz, Some(100e6));
        assert_eq!(hardware.streams[0].center_hz, Some(433_920_000.0));
        assert_eq!(hardware.dc_block, None);
        assert_eq!(hardware.tuning, None);
        assert_eq!(hardware.streams[0].tuning, None);
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
                tuning: None,
                gains: vec![gain("VGA", 30.0)],
                antenna: Some("RX2".to_string()),
                agc: None,
            }],
            ..DeviceSettings::default()
        };

        let tuning_only = StreamScope {
            tuning: true,
            gain: false,
            antenna: false,
            agc: false,
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
            agc: false,
        };
        let lane = settings.for_stream(1, &gain_only);
        assert_eq!(lane.center_hz, Some(100_000_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0), gain("VGA", 30.0)]);
        assert_eq!(lane.antenna.as_deref(), Some("RX"));

        let antenna_only = StreamScope {
            tuning: false,
            gain: false,
            antenna: true,
            agc: false,
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
            agc: false,
        };
        let lane = settings.for_stream(0, &scope);
        assert_eq!(lane.center_hz, Some(100_000_000.0));
        assert_eq!(lane.gains, vec![gain("LNA", 16.0)]);
        assert_eq!(lane.antenna, None);
    }

    #[test]
    fn a_stream_held_by_hand_resolves_manual_while_the_rest_follow_the_radio() {
        let settings = DeviceSettings {
            tuning: Some(Tuning::Auto),
            streams: vec![StreamSettings {
                stream: 1,
                tuning: Some(Tuning::Manual),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        let apart = StreamScope {
            tuning: true,
            gain: false,
            antenna: false,
            agc: false,
        };
        assert!(settings.for_stream(0, &apart).tunes_itself());
        assert!(!settings.for_stream(1, &apart).tunes_itself());
        assert!(
            settings
                .for_stream(1, &StreamScope::default())
                .tunes_itself(),
            "a radio with one synthesizer has one tuning mode"
        );
    }

    #[test]
    fn a_stream_whose_only_override_is_its_tuning_mode_survives_replay() {
        let mut capabilities = tuner(24e6, 1_766e6);
        capabilities.rx_streams = 2;
        capabilities.per_stream = StreamScope {
            tuning: true,
            gain: false,
            antenna: false,
            agc: false,
        };
        let stored = DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                tuning: Some(Tuning::Manual),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        };
        assert_eq!(stored.supported_by(&capabilities).streams, stored.streams);

        capabilities.per_stream = StreamScope::default();
        assert!(
            stored.supported_by(&capabilities).streams.is_empty(),
            "a stream mode means nothing where tuning is shared"
        );
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
            bandwidth: Some(BandwidthSetting::Manual { hz: 2_500_000.0 }),
            ..DeviceSettings::default()
        };
        settings.merge_from(&DeviceSettings {
            bandwidth: Some(BandwidthSetting::Auto),
            ..DeviceSettings::default()
        });
        assert_eq!(settings.center_hz, Some(100_000_000.0));
        assert_eq!(settings.bandwidth, Some(BandwidthSetting::Auto));

        settings.merge_from(&DeviceSettings::default());
        assert_eq!(settings.bandwidth, Some(BandwidthSetting::Auto));
        assert_eq!(settings.bandwidth_hz(), None);
    }

    #[test]
    fn a_recording_takes_only_the_part_of_a_receivers_settings_it_has() {
        let stored = DeviceSettings {
            center_hz: Some(460_802_929.0),
            sample_rate: Some(2.048e6),
            ppm: Some(3.0),
            antenna: Some("RX".to_string()),
            bandwidth: Some(BandwidthSetting::Manual { hz: 1_750_000.0 }),
            gains: vec![GainValue::new(GainKind::Tuner, 30.0)],
            bias_tee: Some(true),
            agc: Some(AgcSetting::switched(false)),
            extra: vec![ExtraValue {
                name: "loop".to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        let mut recording = caps(
            vec![range(460_802_929.0, 460_802_929.0)],
            vec![2.4e6],
            Duplex::RxOnly,
        );
        recording.gains = Vec::new();
        recording.antennas = Vec::new();
        recording.extra = vec![ExtraSetting::bool("loop", "Loop", true)];
        recording.dc_artifact = DcArtifact::None;

        let taken = stored.supported_by(&recording);
        assert_eq!(taken.center_hz, Some(460_802_929.0));
        assert_eq!(taken.sample_rate, None, "a recording plays at its own rate");
        assert_eq!(taken.ppm, None);
        assert_eq!(taken.antenna, None);
        assert_eq!(taken.bandwidth, None);
        assert_eq!(taken.bias_tee, None);
        assert_eq!(taken.agc, None);
        assert!(taken.gains.is_empty());
        assert_eq!(
            taken
                .extra
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>(),
            vec!["loop"]
        );
    }

    fn converted(center_hz: f64, offset_hz: f64) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(center_hz),
            offset_hz: Some(offset_hz),
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(center_hz + 1e6),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn the_radio_is_tuned_below_a_downconverter() {
        let hardware = converted(9.85e9, 9.75e9).to_hardware();
        assert_eq!(hardware.center_hz, Some(100e6));
        assert_eq!(hardware.streams[0].center_hz, Some(101e6));
        assert_eq!(hardware.offset_hz, None);
    }

    #[test]
    fn the_radio_is_tuned_above_an_upconverter() {
        let hardware = converted(7.1e6, -125e6).to_hardware();
        assert_eq!(hardware.center_hz, Some(132.1e6));
    }

    #[test]
    fn readback_is_seen_through_the_converter_again() {
        let shown = converted(9.85e9, 9.75e9);
        let back = DeviceSettings::from_hardware(shown.to_hardware(), shown.offset_hz);
        assert_eq!(
            back,
            DeviceSettings {
                tuning: None,
                ..shown
            }
        );
    }

    #[test]
    fn a_delta_without_an_offset_inherits_the_stored_one() {
        let mut delta = DeviceSettings {
            center_hz: Some(9.9e9),
            ..DeviceSettings::default()
        };
        converted(9.85e9, 9.75e9).carry_offset(&mut delta);
        assert_eq!(delta.offset_hz, Some(9.75e9));
        assert_eq!(delta.to_hardware().center_hz, Some(150e6));
    }

    #[test]
    fn moving_the_offset_alone_leaves_the_radio_where_it_is() {
        let stored = converted(100e6, 0.0);
        let mut delta = DeviceSettings {
            offset_hz: Some(9.75e9),
            ..DeviceSettings::default()
        };
        stored.carry_offset(&mut delta);
        assert_eq!(delta.center_hz, Some(9.85e9));
        assert_eq!(delta.streams[0].center_hz, Some(9.851e9));
        assert_eq!(delta.to_hardware(), stored.to_hardware());
    }

    #[test]
    fn a_named_frequency_wins_over_following_the_offset() {
        let mut delta = DeviceSettings {
            offset_hz: Some(9.75e9),
            center_hz: Some(10e9),
            ..DeviceSettings::default()
        };
        converted(100e6, 0.0).carry_offset(&mut delta);
        assert_eq!(delta.center_hz, Some(10e9));
    }

    #[test]
    fn a_stored_frequency_is_checked_against_the_radio_through_its_offset() {
        let receiver = caps(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);
        let taken = converted(9.85e9, 9.75e9).supported_by(&receiver);
        assert_eq!(taken.center_hz, Some(9.85e9));
        assert_eq!(taken.offset_hz, Some(9.75e9));
        let unreachable = converted(9.85e9, 0.0).supported_by(&receiver);
        assert_eq!(unreachable.center_hz, None);
    }

    #[test]
    fn a_recording_takes_no_converter_offset() {
        let recording = caps(vec![range(9.85e9, 9.85e9)], vec![2.4e6], Duplex::RxOnly);
        assert!(!recording.tunes());
        let taken = converted(9.85e9, 9.75e9).supported_by(&recording);
        assert_eq!(taken.offset_hz, None);
        assert_eq!(taken.center_hz, Some(9.85e9));
    }

    #[test]
    fn a_radio_reaches_further_through_a_converter() {
        let receiver = caps(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);
        let seen = receiver.shifted_by(9.75e9);
        assert_eq!(seen.freq_ranges[0].min, 9.774e9);
        assert_eq!(seen.freq_ranges[0].max, 11.516e9);
        assert_eq!(seen.shifted_by(-9.75e9), receiver);
    }

    #[test]
    fn a_receiver_keeps_every_setting_it_declares() {
        let mut receiver = caps(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);
        receiver.ppm = true;
        receiver.bias_tee = true;
        receiver.agc = Agc::Switch;
        receiver.bandwidth_auto = true;
        receiver.extra = vec![ExtraSetting::bool("offset_tuning", "Offset tuning", false)];
        let stored = DeviceSettings {
            center_hz: Some(145_500_000.0),
            sample_rate: Some(2.048e6),
            ppm: Some(3.0),
            antenna: Some("RX".to_string()),
            bandwidth: Some(BandwidthSetting::Auto),
            gains: vec![GainValue::new(GainKind::Tuner, 30.0)],
            bias_tee: Some(true),
            agc: Some(AgcSetting::switched(true)),
            extra: vec![ExtraValue {
                name: "offset_tuning".to_string(),
                value: true.into(),
            }],
            dc_block: Some(true),
            ..DeviceSettings::default()
        };
        assert_eq!(stored.supported_by(&receiver), stored);
    }

    #[test]
    fn a_shared_tuning_radio_drops_a_per_stream_centre_it_cannot_take() {
        let mut radio = caps(vec![range(24e6, 1.766e9)], vec![2.048e6], Duplex::RxOnly);
        radio.rx_streams = 2;
        radio.per_stream = StreamScope {
            tuning: false,
            gain: true,
            antenna: false,
            agc: false,
        };
        let stored = DeviceSettings {
            streams: vec![
                StreamSettings {
                    stream: 1,
                    center_hz: Some(433_920_000.0),
                    tuning: None,
                    gains: vec![GainValue::new(GainKind::Tuner, 20.0)],
                    antenna: Some("RX".to_string()),
                    agc: None,
                },
                StreamSettings {
                    stream: 7,
                    center_hz: Some(145_500_000.0),
                    ..StreamSettings::default()
                },
            ],
            ..DeviceSettings::default()
        };
        let taken = stored.supported_by(&radio);
        assert_eq!(
            taken.streams.len(),
            1,
            "stream 7 is not a lane this radio has"
        );
        let lane = &taken.streams[0];
        assert_eq!(lane.stream, 1);
        assert_eq!(lane.center_hz, None, "this radio's lanes share one tuning");
        assert_eq!(lane.antenna, None);
        assert_eq!(lane.gains.len(), 1);
    }
}
