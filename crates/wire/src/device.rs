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

    #[must_use]
    pub fn runs_at(&self, rate: f64, tolerance: f64) -> bool {
        // Declared windows are the whole answer, and a veto: no tolerance reaches a rate the
        // resampler cannot produce, and any rate in the menu is inside a window by construction.
        if !self.sample_rate_ranges.is_empty() {
            return any_range_holds(&self.sample_rate_ranges, rate);
        }
        if self
            .sample_rates
            .iter()
            .any(|have| (have - rate).abs() <= rate * tolerance)
        {
            return true;
        }
        if self.sample_rates.is_empty() {
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

/// Who places and removes the receiver's own DC artifact.
///
/// A zero-IF front end lands an impulse at the tuned frequency. Moving the LO clear of every
/// channel is the only correction that works, because a signal genuinely at 0 Hz is
/// arithmetically indistinguishable from the offset, and the blocker must not run until the
/// artifact has somewhere harmless to sit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DcArtifact {
    /// Hardware the engine does not recognise. The operator says where the LO sits and whether
    /// the term is removed, because only they know what the front end already does for itself.
    #[default]
    Operator,
    /// A front end whose artifact the engine knows how to handle. It parks the LO clear of every
    /// channel and removes the term without asking, and neither is an operator setting.
    Managed,
}

impl DcArtifact {
    #[must_use]
    pub const fn is_managed(self) -> bool {
        matches!(self, Self::Managed)
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

/// Whether a value lies in any of `ranges`. An empty list holds nothing, so callers that mean
/// "unconstrained" have to say so themselves.
#[must_use]
pub fn any_range_holds(ranges: &[Range], value: f64) -> bool {
    ranges.iter().any(|range| range.holds(value))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainStage {
    pub name: String,
    pub range: Range,
    /// The settings this stage can actually hold, for hardware whose gain is a table rather than
    /// an even step — the R82xx's 29 irregular entries, say. Empty means every value `range`
    /// admits is reachable. A client renders a control that can only land on real settings, and a
    /// driver still snaps whatever it is asked for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<f64>,
}

impl GainStage {
    /// A switched amplifier — an RF amp that is on or off — rather than a continuous control.
    /// The convention for those is a stage with exactly two settings, so they join the gain
    /// budget the client shows instead of hiding in an unrelated boolean.
    #[must_use]
    pub fn is_switch(&self) -> bool {
        match self.values.len() {
            0 => self
                .range
                .step
                .is_some_and(|step| step > 0.0 && (self.range.max - self.range.min) == step),
            count => count == 2,
        }
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
    /// Continuous windows the radio resamples across. A radio with holes in its rate coverage —
    /// the RTL2832U aliases between 300 kHz and 900 kHz — needs more than one, which is why this
    /// is a list and not the single range it replaced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_rate_ranges: Vec<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub bandwidths: Vec<f64>,
    /// Continuous analog filter widths, for hardware whose IF filter is not a discrete menu.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bandwidth_ranges: Vec<Range>,
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
    /// Whether the engine handles this front end's DC artifact itself. Managed hardware hides
    /// `dc_block` and `lo_offset_hz`, which it overrides.
    #[serde(default)]
    pub dc_artifact: DcArtifact,
    /// Whether the radio sweeps in its own firmware, delivering blocks stamped with the frequency
    /// each was taken at instead of a stream at one tuning.
    #[serde(default)]
    pub hardware_sweep: bool,
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
            sample_rate_ranges: self.sample_rate_ranges.clone(),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dc_block: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lo_offset_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<StreamSettings>,
}

/// How far off centre the hardware LO may be parked, as a fraction of the sample rate.
///
/// Beyond this the wanted signal falls into the tuner's analog filter roll-off.
pub const MAX_LO_OFFSET_FRACTION: f64 = 0.4;

/// How far the engine parks the LO on hardware whose artifact it manages.
///
/// Far enough that the artifact clears a channel sitting at the tune frequency, well inside
/// [`MAX_LO_OFFSET_FRACTION`] so the wanted signal stays out of the tuner's filter roll-off.
pub const MANAGED_LO_OFFSET_FRACTION: f64 = 0.25;

#[must_use]
pub fn managed_lo_offset_hz(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate * MANAGED_LO_OFFSET_FRACTION
    } else {
        0.0
    }
}

#[must_use]
pub fn lo_offset_limit_hz(sample_rate: f64) -> f64 {
    if sample_rate.is_finite() && sample_rate > 0.0 {
        sample_rate * MAX_LO_OFFSET_FRACTION
    } else {
        0.0
    }
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
        if delta.dc_block.is_some() {
            self.dc_block = delta.dc_block;
        }
        if delta.lo_offset_hz.is_some() {
            self.lo_offset_hz = delta.lo_offset_hz;
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

    /// The LO offset that is actually in force, which is not always the one that was asked for.
    ///
    /// A request survives only if it stays inside the tuner's analog passband and every centre it
    /// displaces is still reachable; otherwise the front end falls back to tuning dead centre.
    /// The request itself is left untouched so it can take effect again elsewhere in the band.
    #[must_use]
    pub fn effective_lo_offset_hz(&self, capabilities: &Capabilities, sample_rate: f64) -> f64 {
        let wanted = self.lo_offset_hz.unwrap_or(0.0);
        if !wanted.is_finite() || wanted == 0.0 {
            return 0.0;
        }
        let limit = lo_offset_limit_hz(sample_rate);
        let offset = wanted.clamp(-limit, limit);
        if offset == 0.0 {
            return 0.0;
        }
        let reachable = |hz: Option<f64>| {
            hz.is_none_or(|hz| {
                capabilities.freq_ranges.is_empty()
                    || any_range_holds(&capabilities.freq_ranges, hz - offset)
            })
        };
        if reachable(self.center_hz) && self.streams.iter().all(|s| reachable(s.center_hz)) {
            offset
        } else {
            0.0
        }
    }

    /// Rewrites the operator's tuning into the tuning the hardware is given.
    ///
    /// The offset is subtracted here and added back by the front end's mixer, so every frequency
    /// outside this function stays the one the operator asked for.
    #[must_use]
    pub fn to_hardware(&self, lo_offset_hz: f64) -> DeviceSettings {
        let mut hardware = self.shifted(-lo_offset_hz);
        hardware.dc_block = None;
        hardware.lo_offset_hz = None;
        hardware
    }

    /// Reads hardware tuning back in the operator's frame, undoing [`Self::to_hardware`].
    #[must_use]
    pub fn to_operator(&self, lo_offset_hz: f64) -> DeviceSettings {
        self.shifted(lo_offset_hz)
    }

    fn shifted(&self, by_hz: f64) -> DeviceSettings {
        let mut shifted = self.clone();
        if by_hz != 0.0 {
            shifted.center_hz = shifted.center_hz.map(|hz| hz + by_hz);
            for stream in &mut shifted.streams {
                stream.center_hz = stream.center_hz.map(|hz| hz + by_hz);
            }
        }
        shifted
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
            sample_rate_ranges: Vec::new(),
            gains: vec![GainStage {
                name: "TUNER".to_string(),
                range: Range {
                    min: 0.0,
                    max: 49.6,
                    step: None,
                },
                values: Vec::new(),
            }],
            antennas: vec!["RX".to_string()],
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

    fn stage(range: Range, values: Vec<f64>) -> GainStage {
        GainStage {
            name: "TEST".to_string(),
            range,
            values,
        }
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
    fn a_two_setting_stage_is_a_switch_however_it_was_declared() {
        let mut stepped = range(0.0, 14.0);
        stepped.step = Some(14.0);
        assert!(stage(stepped, Vec::new()).is_switch());
        assert!(stage(range(0.0, 14.0), vec![0.0, 14.0]).is_switch());
        assert!(!stage(range(0.0, 14.0), vec![0.0, 7.0, 14.0]).is_switch());
    }

    #[test]
    fn a_rate_menu_is_exact_and_a_range_is_a_bound() {
        let menu = caps(Vec::new(), vec![1.024e6, 2.0e6, 2.4e6], Duplex::RxOnly).profile();
        assert!(menu.runs_at(2.0e6, 0.0), "ADS-B needs exactly 2 Msps");
        assert!(!menu.runs_at(2.2e6, 0.0));
        assert!(menu.runs_at(2.2e6, 0.1), "within tolerance");

        let mut continuous = caps(Vec::new(), Vec::new(), Duplex::RxOnly).profile();
        continuous.sample_rate_ranges = vec![range(2e6, 20e6)];
        assert!(continuous.runs_at(8e6, 0.0));
        assert!(!continuous.runs_at(1e6, 0.0));

        assert!(DeviceProfile::default().runs_at(2.4e6, 0.0));
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
        assert!(windows.runs_at(250_000.0, 0.0));
        assert!(windows.runs_at(1.8e6, 0.0), "inside the upper window");
        assert!(
            !windows.runs_at(500_000.0, 0.5),
            "the RTL2832U aliases between the two windows, and no tolerance makes that reachable"
        );
        assert!(!windows.runs_at(4e6, 0.5), "past the top of every window");
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

    const RATE: f64 = 2_400_000.0;

    fn tuner(min: f64, max: f64) -> Capabilities {
        caps(vec![range(min, max)], vec![RATE], Duplex::RxOnly)
    }

    fn tuned(center_hz: f64, lo_offset_hz: Option<f64>) -> DeviceSettings {
        DeviceSettings {
            center_hz: Some(center_hz),
            sample_rate: Some(RATE),
            lo_offset_hz,
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn an_lo_offset_is_capped_at_a_fraction_of_the_sample_rate() {
        let caps = tuner(24e6, 1_766e6);
        let limit = lo_offset_limit_hz(RATE);
        assert_eq!(limit, RATE * MAX_LO_OFFSET_FRACTION);

        let asked = tuned(100e6, Some(limit * 4.0));
        assert_eq!(asked.effective_lo_offset_hz(&caps, RATE), limit);

        let negative = tuned(100e6, Some(-limit * 4.0));
        assert_eq!(negative.effective_lo_offset_hz(&caps, RATE), -limit);
    }

    #[test]
    fn an_lo_offset_that_would_leave_the_tuning_range_is_dropped() {
        let caps = tuner(24e6, 1_766e6);
        let at_the_edge = tuned(24_100_000.0, Some(300_000.0));
        assert_eq!(
            at_the_edge.effective_lo_offset_hz(&caps, RATE),
            0.0,
            "the front end kept an offset the tuner cannot reach"
        );
        let inside = tuned(100e6, Some(300_000.0));
        assert_eq!(inside.effective_lo_offset_hz(&caps, RATE), 300_000.0);
    }

    #[test]
    fn an_unset_or_impossible_lo_offset_is_no_offset() {
        let caps = tuner(24e6, 1_766e6);
        assert_eq!(tuned(100e6, None).effective_lo_offset_hz(&caps, RATE), 0.0);
        assert_eq!(
            tuned(100e6, Some(f64::NAN)).effective_lo_offset_hz(&caps, RATE),
            0.0
        );
        assert_eq!(
            tuned(100e6, Some(200e3)).effective_lo_offset_hz(&caps, 0.0),
            0.0,
            "no sample rate leaves no room to move the LO"
        );
    }

    #[test]
    fn a_stream_that_cannot_follow_the_offset_holds_the_whole_radio_back() {
        let caps = tuner(24e6, 1_766e6);
        let mut settings = tuned(100e6, Some(300_000.0));
        settings.streams = vec![StreamSettings {
            stream: 1,
            center_hz: Some(24_100_000.0),
            ..StreamSettings::default()
        }];
        assert_eq!(settings.effective_lo_offset_hz(&caps, RATE), 0.0);
    }

    #[test]
    fn hardware_tuning_displaces_every_centre_and_comes_back_unchanged() {
        let mut settings = tuned(100e6, Some(300_000.0));
        settings.dc_block = Some(true);
        settings.streams = vec![StreamSettings {
            stream: 1,
            center_hz: Some(433_920_000.0),
            ..StreamSettings::default()
        }];

        let hardware = settings.to_hardware(300_000.0);
        assert_eq!(hardware.center_hz, Some(99_700_000.0));
        assert_eq!(hardware.streams[0].center_hz, Some(433_620_000.0));
        assert_eq!(
            (hardware.dc_block, hardware.lo_offset_hz),
            (None, None),
            "the front end's own settings were handed to the driver"
        );

        let back = hardware.to_operator(300_000.0);
        assert_eq!(back.center_hz, settings.center_hz);
        assert_eq!(back.streams[0].center_hz, settings.streams[0].center_hz);
    }

    #[test]
    fn no_offset_leaves_the_tuning_exactly_as_it_was() {
        let settings = tuned(100e6, None);
        assert_eq!(settings.to_hardware(0.0).center_hz, Some(100e6));
        assert_eq!(settings.to_operator(0.0).center_hz, Some(100e6));
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
