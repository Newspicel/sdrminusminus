//! Device capability model and settings. `Capabilities` is the backbone of the
//! backend-driven UI (): the client auto-renders controls from it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A discovered receiver, produced by a driver's probe ().
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceInfo {
    /// Driver id that produced this entry: `"virtual"`, `"soapy"`, `"rtlsdr"`, …
    pub driver: String,
    /// Stable per-device key within a driver (serial, index, or file path).
    pub key: String,
    /// Human label for the device picker.
    pub label: String,
    /// Serial number when the driver exposes one (used to collapse probe duplicates, ).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
    /// What the driver can say about this radio *without opening it* — enough to tell whether a
    /// workspace template could run here, so the picker offers only the devices that fit.
    ///
    /// `None` means "not known until it is opened", which is what a backend that has to ask the
    /// hardware reports (Soapy). Unknown is never read as capable: a device with no profile is
    /// offered with that said plainly, rather than filtered in on a guess.
    ///
    /// It is deliberately narrower than [`Capabilities`]: everything here is a fact about the
    /// *model*, so a driver can answer from a table. Gain stages, antennas and the extras are
    /// the unit's and are read when it opens — reporting an empty gain list at probe time would
    /// not read as "not asked yet", it would read as "no gain control".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<DeviceProfile>,
}

/// The part of a radio's capability that is a property of the model rather than the unit.
///
/// Split out so it can be answered twice from one declaration: by a probe, which has only a USB
/// descriptor, and by [`Capabilities::profile`] on a radio that is already open. Nothing
/// constructs it by hand — it is always a projection of a full capability set, which is what
/// keeps the two from drifting.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DeviceProfile {
    /// Tunable centre-frequency ranges in Hz (multiple = discontiguous tuner ranges).
    pub freq_ranges: Vec<Range>,
    /// Supported sample rates in samples/s. Empty means "continuous", use `sample_rate_range`.
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_range: Option<Range>,
    pub duplex: Duplex,
    pub rx_streams: u32,
    pub tx_streams: u32,
    /// Which settings each stream holds on its own — a property of the model, so the picker can
    /// tell a coherent array from a bank of independent tuners without opening the radio.
    #[serde(default)]
    pub per_stream: StreamScope,
}

impl DeviceProfile {
    /// Whether this radio can be tuned to `hz`. A radio that advertises no range at all is not
    /// claiming to reach everything — the virtual devices and any backend that reports nothing
    /// land here — so it answers `true` and is filtered on nothing.
    #[must_use]
    pub fn reaches(&self, hz: f64) -> bool {
        self.freq_ranges.is_empty()
            || self
                .freq_ranges
                .iter()
                .any(|range| hz >= range.min && hz <= range.max)
    }

    /// Whether this radio can run at `rate`, within `tolerance` as a fraction.
    ///
    /// Two readings of `sample_rates`, because it carries two meanings. A rate *on* the menu is
    /// selectable and settles the question. A rate that is merely inside the menu's span is
    /// accepted too, at any non-zero tolerance: a driver may deliberately under-advertise —
    /// the RTL-SDR ships "the conventional menu every other tool offers" and validates anything
    /// inside its windows — so reading the list as exhaustive would hide templates the radio can
    /// actually run. Erring toward offering is the right way to be wrong here: the engine still
    /// refuses on apply, with its own reason, while a false refusal is invisible.
    ///
    /// A zero tolerance is the caller saying only this exact rate decodes. Then the menu *is* the
    /// answer, because the rate picker offers nothing else — an unlisted rate cannot be selected
    /// whatever the hardware would accept.
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
    /// The `driver:key` handle used by `POST /api/devicesets` ().
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}:{}", self.driver, self.key)
    }

    /// This device stripped of its probe-time [`profile`](Self::profile).
    ///
    /// What a [`crate::DeviceSet`] stores: the set carries the opened radio's real capabilities
    /// in its own field, and keeping the model's datasheet beside them invites reading whichever
    /// one is nearer.
    #[must_use]
    pub fn identity(&self) -> Self {
        Self {
            profile: None,
            ..self.clone()
        }
    }
}

/// One signal direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Radio to host.
    Rx,
    /// Host to radio.
    Tx,
}

impl Direction {
    /// The other one.
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

/// What a radio's hardware can do, and whether it can do it at once.
///
/// It lives here rather than in the device crate because it is the answer to "what *kind* of
/// radio is this" that a workspace template filters on and the canvas draws ports from — the
/// arbitration that uses it ([`sdrmm-device`'s `DuplexState`]) is a separate thing, and stays
/// with the backends.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Duplex {
    /// Receive only: RTL-SDR and every backend with no transmitter (virtual and playback).
    /// The default, so a backend that says nothing
    /// cannot accidentally advertise a transmitter.
    #[default]
    RxOnly,
    /// Transmit only: a bench signal generator with no receive path.
    TxOnly,
    /// Both, one at a time: the HackRF, whose LPC transceiver mode selects a single data path,
    /// so changing direction means stopping the other first.
    Half,
    /// Both at once: USRP, LimeSDR, PlutoSDR, bladeRF.
    Full,
}

impl Duplex {
    /// Whether the hardware has this direction at all.
    #[must_use]
    pub const fn supports(self, direction: Direction) -> bool {
        match (self, direction) {
            (Self::RxOnly, Direction::Rx)
            | (Self::TxOnly, Direction::Tx)
            | (Self::Half | Self::Full, _) => true,
            (Self::RxOnly, Direction::Tx) | (Self::TxOnly, Direction::Rx) => false,
        }
    }

    /// Whether both directions can be live together.
    #[must_use]
    pub const fn simultaneous(self) -> bool {
        matches!(self, Self::Full)
    }
}

/// An inclusive numeric range with an optional step, in the setting's native unit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
}

/// A named gain stage with its range in dB (e.g. RTL-SDR tuner gain, HackRF LNA/VGA).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainStage {
    pub name: String,
    pub range: Range,
}

/// A typed device-specific setting the client renders generically when it has no
/// first-class UI (: "typed extra settings").
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
        /// The values the setting accepts, each carrying the driver's own words for it where it
        /// has them: `direct_samp` is written as `0`/`1`/`2` and read as Off/I-ADC/Q-ADC.
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

/// The exact type SoapySDR declares for an argument. This stays separate from
/// [`ExtraSetting`], which is the compact control shape used by the existing receiver UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentType {
    Bool,
    Float,
    Int,
    String,
}

/// One labelled value in a driver's discrete option list.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ArgumentOption {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl ArgumentOption {
    /// A value whose driver offers no words for it, so the value is what the client shows.
    #[must_use]
    pub fn plain(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: None,
        }
    }
}

/// Lossless wire representation of SoapySDR's `ArgInfo`.
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

/// Capabilities of one hardware channel in one signal direction.
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

/// Directional and runtime capabilities that cannot be represented by the legacy channel-0
/// receiver fields on [`Capabilities`].
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

/// Which device settings each receive stream holds on its own, rather than sharing with the rest
/// of the radio. All-false — the default, and what every capability set from before this field
/// describes — is the single-stream radio.
///
/// Sample rate is deliberately absent and stays shared: one device, one clock domain. Channel
/// rate validation, the recorder's SigMF `core:sample_rate` and the DSP lanes all assume it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct StreamScope {
    /// Each stream tunes independently. False on a coherent array — the streams share one
    /// tuner reference and tuning them apart is what a coherent array must never do — and true
    /// on a radio with a synthesizer per stream.
    #[serde(default)]
    pub tuning: bool,
    /// Each stream has its own gain stages. True on nearly every multi-stream radio, including
    /// the coherent arrays: per-channel gain calibration is how an array is levelled.
    #[serde(default)]
    pub gain: bool,
    /// Each stream selects its own antenna port.
    #[serde(default)]
    pub antenna: bool,
}

/// Everything the client needs to render device controls without hand-written DTOs ().
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Capabilities {
    /// Tunable center-frequency ranges in Hz (multiple = discontiguous tuner ranges).
    pub freq_ranges: Vec<Range>,
    /// Supported sample rates in samples/s. Empty means "continuous", use `sample_rate_range`.
    pub sample_rates: Vec<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_rate_range: Option<Range>,
    pub gains: Vec<GainStage>,
    pub antennas: Vec<String>,
    pub bandwidths: Vec<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraSetting>,
    /// Whether the radio has a frequency-correction setting at all. Several do not — HackRF has
    /// no correction register, SpyServer's protocol carries no such field, a Soapy tuner without
    /// a `CORR` component cannot apply one — and their backends already refuse `ppm` outright,
    /// while a recording and the signal generator swallow it and do nothing. Without this the
    /// client drew the control on every device, so a knob that could only error, or only lie,
    /// looked exactly like one that worked.
    ///
    /// Defaults to *unsupported* for the same reason `duplex` defaults to receive-only: a
    /// capability must be declared, never advertised by omission.
    #[serde(default)]
    pub ppm: bool,
    /// Which directions this radio has, and whether it can run them together. Receive-only
    /// unless a backend says otherwise, so a device cannot advertise a transmitter by omission.
    /// This is the *hardware's* shape, not a permission:  gates transmit behind an
    /// authorized-use switch that does not exist, so a `half` radio still transmits nothing.
    #[serde(default)]
    pub duplex: Duplex,
    /// How many independent receive streams this radio delivers at once. One for an ordinary
    /// SDR; a KrakenSDR is five coherent channels on one USB device, which is why this is a
    /// count and not a bool. A device node draws one IQ output per stream.
    #[serde(default = "one_stream")]
    pub rx_streams: u32,
    /// How many independent transmit streams it accepts. Reported for symmetry and for the
    /// device picker; the canvas still draws a single reserved transmit input, because there is
    /// nothing to wire into it until  lands.
    #[serde(default)]
    pub tx_streams: u32,
    /// Which settings each receive stream holds on its own. Which settings are per-stream is a
    /// property of the *radio* — a coherent array shares one tuning by definition while a
    /// two-daughterboard USRP genuinely has two — so the radio declares it, the same way it
    /// declares gains and antennas.
    #[serde(default)]
    pub per_stream: StreamScope,
    /// Full per-direction, per-channel capability data. Older backends and stored payloads omit
    /// it; Soapy-backed devices always populate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directional: Option<DirectionalCapabilities>,
}

/// A radio with no declared stream count still has one receiver: the field was added when
/// multi-stream devices were, and every stored or peer-sent capability set that predates it
/// describes a single-stream radio.
const fn one_stream() -> u32 {
    1
}

impl Capabilities {
    /// The model-level part of this capability set, for a probe result to carry.
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

/// A gain-stage value in dB, keyed by the stage name from [`GainStage`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct GainValue {
    pub stage: String,
    pub value_db: f64,
}

/// A value for one [`ExtraSetting`], keyed by its name. `value` is bool/number/string.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ExtraValue {
    pub name: String,
    pub value: serde_json::Value,
}

/// A mutation applied to a device. Absent fields are left unchanged ( PATCH device).
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
    /// Hardware baseband filter bandwidth in Hz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gains: Vec<GainValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<ExtraValue>,
    /// Per-stream overrides, by stream index. Only the fields [`Capabilities::per_stream`] marks
    /// as per-stream are read here; the rest are the radio's and live above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub streams: Vec<StreamSettings>,
}

/// One stream's overrides of the radio-wide [`DeviceSettings`]. Absent fields fall back to the
/// radio-wide value — [`DeviceSettings::for_stream`] is the one place that resolution lives.
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

/// Per stage name, not positional: the capability UI sends one control's value at a time, so a
/// delta carrying one stage must patch only that stage.
fn merge_gains(gains: &mut Vec<GainValue>, delta: &[GainValue]) {
    for gain in delta {
        match gains.iter_mut().find(|g| g.stage == gain.stage) {
            Some(existing) => existing.value_db = gain.value_db,
            None => gains.push(gain.clone()),
        }
    }
}

impl DeviceSettings {
    /// Overlay the present fields of `delta` onto `self` ( PATCH: absent fields are
    /// unchanged). Gains and extras merge per name, so a delta carrying one stage patches only
    /// that stage — required for the capability UI, which sends one control's value at a time.
    /// Stream overrides merge by stream index, and each entry's gains per stage name again.
    /// The single merge implementation: engine and device backends both use this so applied
    /// settings and reported state can never disagree on merge semantics.
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

    /// What stream `index` is actually set to: its own override where the radio declares that
    /// setting per-stream, the radio-wide value otherwise. The result carries no `streams` of its
    /// own — it is one lane's resolved view, not the overrides table.
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

    /// The profile is a projection, never a second declaration: what a probe reports and what an
    /// opened radio reports come from one place, so they cannot drift.
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

        // A radio that advertises nothing is filtered on nothing, rather than filtered out.
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

        // Neither declared: unconstrained, not unusable.
        assert!(DeviceProfile::default().runs_at(2.4e6, 0.0));
    }

    /// A payload from a peer that predates multi-stream radios describes a single-stream one, and
    /// a backend that says nothing about transmit still has none.
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

    /// An open radio's identity must not carry the model's datasheet beside the capabilities it
    /// actually reported, or a reader picks whichever is nearer.
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
                    // Absent in the delta, so the stream keeps its centre — the same PATCH
                    // semantics the radio-wide fields follow.
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

    /// Each `StreamScope` flag gates exactly its own setting: a scoped setting resolves to the
    /// stream's override, an unscoped one to the radio-wide value even when an override is
    /// present (a client bug the engine refuses separately).
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
        // The stage the stream did not touch keeps the radio-wide value: overrides are
        // per stage, not a wholesale gain replacement.
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

    /// A stream with no override entry is set to the radio-wide values whatever the scope —
    /// retuning the radio-wide centre is the default for such streams (edge case: it must not
    /// take overrides that exist on other streams with it).
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

    /// A payload from before per-stream settings existed is today's single-stream radio: no
    /// scope, no overrides, and serializing back adds neither key.
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
