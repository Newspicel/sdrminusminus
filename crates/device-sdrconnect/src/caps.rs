use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    ArgumentOption, BandwidthSetting, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex,
    ExtraSetting, ExtraValue, GainKind, GainStage, GainUnit, GainValue, Range, StreamScope,
};

use crate::{
    proto::{Command, Event, NetworkMode, Property, Recording, flag},
    session::Snapshot,
};

pub(crate) const RECEIVER: &str = "receiver";
pub(crate) const NETWORK_MODE: &str = "network_mode";
pub(crate) const PROFILE: &str = "device_profile";
pub(crate) const RECORDING: &str = "recording";

const OFF: &str = "off";
const AUTOMATIC: &str = "automatic";

/// What every RSP reaches, which is what the API itself never says.
const MIN_FREQUENCY_HZ: f64 = 1_000.0;
const MAX_FREQUENCY_HZ: f64 = 2_000_000_000.0;

const MIN_SAMPLE_RATE: f64 = 62_500.0;
const MAX_SAMPLE_RATE: f64 = 10_000_000.0;

const MIN_FILTER_HZ: f64 = 1.0;

fn pinned(value: Option<f64>) -> Vec<Range> {
    value
        .map(|value| {
            vec![Range {
                min: value,
                max: value,
                step: None,
            }]
        })
        .unwrap_or_default()
}

fn window(min: f64, max: f64) -> Vec<Range> {
    vec![Range {
        min,
        max,
        step: None,
    }]
}

fn choice(
    name: &str,
    label: &str,
    off: &str,
    rest: impl Iterator<Item = &'static str>,
) -> ExtraSetting {
    ExtraSetting::choice(
        name,
        label,
        std::iter::once(ArgumentOption::plain(off))
            .chain(rest.map(ArgumentOption::plain))
            .collect(),
        off,
    )
}

fn stepped(min: f64, max: f64) -> Range {
    Range {
        min,
        max,
        step: Some(1.0),
    }
}

fn lna_stage(snapshot: &Snapshot, steerable: bool) -> Option<GainStage> {
    let min = snapshot.count(Property::LnaStateMin).unwrap_or(0);
    let max = snapshot.count(Property::LnaStateMax)?;
    (steerable && max > min).then(|| {
        GainStage::new(GainKind::Lna, stepped(min as f64, max as f64)).with_unit(GainUnit::Index)
    })
}

fn filter_range(snapshot: &Snapshot) -> Option<Range> {
    let widest = snapshot
        .number(Property::DemodMaxBandwidth)
        .filter(|w| *w >= MIN_FILTER_HZ)?;
    snapshot
        .text(Property::FilterBandwidth)
        .map(|_| stepped(MIN_FILTER_HZ, widest))
}

/// A setting the receiver answered for, or nothing when this build of SDRconnect has no such
/// thing to offer.
fn answered(
    snapshot: &Snapshot,
    property: Property,
    setting: ExtraSetting,
) -> Option<ExtraSetting> {
    snapshot.text(property).map(|_| setting)
}

pub(crate) fn capabilities(snapshot: &Snapshot) -> Capabilities {
    let steerable = snapshot.boolean(Property::CanControl).unwrap_or(true);
    let receivers = snapshot.list(Property::ValidDevices);

    let mut extra = Vec::new();
    extra.extend(answered(
        snapshot,
        Property::DeviceVfoFrequency,
        ExtraSetting::range(
            Property::DeviceVfoFrequency.name(),
            "Device VFO",
            stepped(MIN_FREQUENCY_HZ, MAX_FREQUENCY_HZ),
            "Hz",
        ),
    ));
    if !receivers.is_empty() {
        let active = snapshot
            .text(Property::ActiveDevice)
            .filter(|name| receivers.iter().any(|known| known == name))
            .unwrap_or(&receivers[0])
            .to_string();
        extra.push(ExtraSetting::choice(
            RECEIVER,
            "Receiver",
            receivers
                .iter()
                .map(|name| ArgumentOption::plain(name.as_str()))
                .collect(),
            active,
        ));
        extra.push(choice(
            NETWORK_MODE,
            "Network mode",
            AUTOMATIC,
            NetworkMode::ALL.into_iter().map(NetworkMode::name),
        ));
    }
    extra.push(ExtraSetting::text(PROFILE, "Device profile", ""));
    extra.push(choice(
        RECORDING,
        "Recording",
        OFF,
        Recording::ALL.into_iter().map(Recording::name),
    ));

    Capabilities {
        freq_ranges: if steerable {
            window(MIN_FREQUENCY_HZ, MAX_FREQUENCY_HZ)
        } else {
            pinned(snapshot.number(Property::DeviceCenterFrequency))
        },
        sample_rates: Vec::new(),
        sample_rate_ranges: if steerable {
            window(MIN_SAMPLE_RATE, MAX_SAMPLE_RATE)
        } else {
            pinned(snapshot.number(Property::DeviceSampleRate))
        },
        gains: lna_stage(snapshot, steerable).into_iter().collect(),
        antennas: snapshot.list(Property::ValidAntennas),
        bandwidths: Vec::new(),
        bandwidth_ranges: filter_range(snapshot).into_iter().collect(),
        extra,
        bandwidth_auto: false,
        bias_tee: false,
        agc: sdrmm_wire::Agc::None,
        ppm: false,
        duplex: Duplex::RxOnly,
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

/// Which of the three ways the API offers was used to name the radio on the host.
#[derive(Clone, Debug, PartialEq)]
enum Selection {
    Name(String),
    Index(u32),
    Serial(String),
}

impl Selection {
    /// A slot in the list the receiver published is an index; a serial number that happens to be
    /// all digits is longer than that list and stays a serial.
    fn of(value: &str, offered: &[ArgumentOption]) -> Self {
        let slot = value
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|index| (*index as usize) < offered.len());
        match (offered.iter().any(|option| option.value == value), slot) {
            (true, _) => Self::Name(value.to_string()),
            (false, Some(index)) => Self::Index(index),
            (false, None) => Self::Serial(value.to_string()),
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Name(name) | Self::Serial(name) => name.clone(),
            Self::Index(index) => index.to_string(),
        }
    }

    /// An index names a slot rather than a radio, so the network mode rides only on the two
    /// forms the specification attaches it to.
    fn command(&self, mode: Option<NetworkMode>) -> Command {
        let with_mode = |name: &str| match mode {
            Some(mode) => format!("{name}:{}", mode.name()),
            None => name.to_string(),
        };
        match self {
            Self::Name(name) => Command::Emit(Event::SelectedDeviceName, with_mode(name)),
            Self::Serial(serial) => Command::Emit(Event::SelectedDeviceSerial, with_mode(serial)),
            Self::Index(index) => Command::Emit(Event::SelectedDevice, index.to_string()),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Remote {
    center_hz: Option<u64>,
    sample_rate: Option<f64>,
    lna_state: Option<u64>,
    vfo_hz: Option<u64>,
    filter_hz: Option<u64>,
    antenna: Option<String>,
    receiver: Option<Selection>,
    network_mode: Option<NetworkMode>,
    profile: Option<String>,
    recording: Option<Recording>,
    /// Whether the server was already streaming when SDR-- arrived, so stopping leaves the
    /// operator's own session the way they left it.
    streaming: bool,
}

impl Remote {
    pub(crate) fn new(snapshot: &Snapshot) -> Self {
        Self {
            center_hz: snapshot.count(Property::DeviceCenterFrequency),
            sample_rate: snapshot.number(Property::DeviceSampleRate),
            lna_state: snapshot.count(Property::LnaState),
            vfo_hz: snapshot.count(Property::DeviceVfoFrequency),
            filter_hz: snapshot.count(Property::FilterBandwidth),
            antenna: snapshot
                .text(Property::ActiveAntenna)
                .map(ToString::to_string),
            receiver: snapshot
                .text(Property::ActiveDevice)
                .map(|name| Selection::Name(name.to_string())),
            network_mode: None,
            profile: None,
            recording: None,
            streaming: snapshot.boolean(Property::Started).unwrap_or(false),
        }
    }

    /// Everything the server has to be told again on a connection that replaced a lost one.
    pub(crate) fn replay(&self) -> Vec<Command> {
        let mut batch = Vec::new();
        if let Some(receiver) = &self.receiver {
            batch.push(receiver.command(self.network_mode));
        }
        if let Some(profile) = &self.profile {
            batch.push(Command::Emit(Event::ApplyDeviceProfile, profile.clone()));
        }
        if let Some(rate) = self.sample_rate {
            batch.push(Command::Set(Property::DeviceSampleRate, rate.to_string()));
        }
        if let Some(hz) = self.center_hz {
            batch.push(Command::Set(
                Property::DeviceCenterFrequency,
                hz.to_string(),
            ));
        }
        if let Some(state) = self.lna_state {
            batch.push(Command::Set(Property::LnaState, state.to_string()));
        }
        if let Some(hz) = self.vfo_hz {
            batch.push(Command::Set(Property::DeviceVfoFrequency, hz.to_string()));
        }
        if let Some(hz) = self.filter_hz {
            batch.push(Command::Set(Property::FilterBandwidth, hz.to_string()));
        }
        if let Some(antenna) = &self.antenna {
            batch.push(Command::Set(Property::ActiveAntenna, antenna.clone()));
        }
        batch
    }

    /// Turns the IQ on, and switches off the streams SDR-- has nowhere to put.
    pub(crate) fn start(&self) -> Vec<Command> {
        let mut batch = self.replay();
        batch.push(Command::Emit(Event::AudioStreamEnable, flag(false)));
        batch.push(Command::Emit(Event::SpectrumEnable, flag(false)));
        batch.push(Command::Emit(Event::DeviceStreamEnable, flag(true)));
        batch.push(Command::Emit(Event::IqStreamEnable, flag(true)));
        if let Some(recording) = self.recording {
            batch.push(Command::Emit(
                Event::StartRecording,
                recording.name().to_string(),
            ));
        }
        batch
    }

    pub(crate) fn stop(&self) -> Vec<Command> {
        let mut batch = vec![Command::Emit(Event::IqStreamEnable, flag(false))];
        if self.recording.is_some() {
            batch.push(Command::Emit(Event::StopRecording, String::new()));
        }
        if !self.streaming {
            batch.push(Command::Emit(Event::DeviceStreamEnable, flag(false)));
        }
        batch
    }

    pub(crate) fn wire(&self, caps: &Capabilities) -> DeviceSettings {
        let gains = self
            .lna_state
            .filter(|_| caps.stage(GainKind::Lna.name()).is_some())
            .map(|state| GainValue::new(GainKind::Lna, state as f64))
            .into_iter()
            .collect();
        let bandwidth = self
            .filter_hz
            .filter(|_| caps.has_filter())
            .map(|hz| BandwidthSetting::Manual { hz: hz as f64 });
        let mut extra = Vec::new();
        if let Some(vfo_hz) = self
            .vfo_hz
            .filter(|_| offers(caps, Property::DeviceVfoFrequency.name()).is_some())
        {
            extra.push(value(Property::DeviceVfoFrequency.name(), vfo_hz));
        }
        if let Some(receiver) = self
            .receiver
            .as_ref()
            .filter(|_| offers(caps, RECEIVER).is_some())
        {
            extra.push(value(RECEIVER, receiver.label()));
        }
        if offers(caps, NETWORK_MODE).is_some() {
            extra.push(value(
                NETWORK_MODE,
                self.network_mode.map_or(AUTOMATIC, NetworkMode::name),
            ));
        }
        extra.push(value(PROFILE, self.profile.clone().unwrap_or_default()));
        extra.push(value(
            RECORDING,
            self.recording.map_or(OFF, Recording::name),
        ));
        DeviceSettings {
            center_hz: self.center_hz.map(|hz| hz as f64),
            sample_rate: self.sample_rate,
            antenna: self.antenna.clone(),
            bandwidth,
            gains,
            extra,
            ..DeviceSettings::default()
        }
    }
}

fn value(name: &str, value: impl Into<serde_json::Value>) -> ExtraValue {
    ExtraValue {
        name: name.to_string(),
        value: value.into(),
    }
}

fn offers<'a>(caps: &'a Capabilities, name: &str) -> Option<&'a ExtraSetting> {
    caps.extra.iter().find(|setting| setting.name() == name)
}

fn within(ranges: &[Range], value: f64) -> bool {
    ranges.iter().any(|range| range.holds(value))
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    current: &Remote,
) -> Result<(Remote, Vec<Command>), DeviceError> {
    check_stream_settings(delta, caps)?;
    let mut next = current.clone();
    let mut batch = Vec::new();

    if let Some(hz) = delta.center_hz {
        if !within(&caps.freq_ranges, hz) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside what this receiver tunes to"
            )));
        }
        next.center_hz = Some(hz.round() as u64);
        batch.push(Command::Set(
            Property::DeviceCenterFrequency,
            hz.round().to_string(),
        ));
    }

    if let Some(rate) = delta.sample_rate {
        if !within(&caps.sample_rate_ranges, rate) {
            return Err(DeviceError::Unsupported(format!(
                "sample_rate {rate} outside what this receiver samples at"
            )));
        }
        next.sample_rate = Some(rate);
        batch.push(Command::Set(Property::DeviceSampleRate, rate.to_string()));
    }

    if let Some(antenna) = &delta.antenna {
        if !caps.antennas.iter().any(|known| known == antenna) {
            return Err(DeviceError::Unsupported(format!(
                "antenna {antenna}: this receiver offers {:?}",
                caps.antennas
            )));
        }
        next.antenna = Some(antenna.clone());
        batch.push(Command::Set(Property::ActiveAntenna, antenna.clone()));
    }

    if delta.ppm.is_some_and(|ppm| ppm != 0.0) {
        return Err(DeviceError::Unsupported(
            "ppm: the SDRconnect API has no frequency correction; correct it on the server"
                .to_string(),
        ));
    }
    if let Some(bandwidth) = delta.bandwidth {
        apply_bandwidth(&mut next, &mut batch, caps, bandwidth)?;
    }
    for gain in &delta.gains {
        apply_gain(&mut next, &mut batch, caps, gain)?;
    }
    for value in &delta.extra {
        apply_extra(&mut next, &mut batch, caps, value)?;
    }
    Ok((next, batch))
}

fn apply_bandwidth(
    next: &mut Remote,
    batch: &mut Vec<Command>,
    caps: &Capabilities,
    bandwidth: BandwidthSetting,
) -> Result<(), DeviceError> {
    let BandwidthSetting::Manual { hz } = bandwidth else {
        return Err(DeviceError::Unsupported(
            "bandwidth: the SDRconnect channel filter has no automatic width".to_string(),
        ));
    };
    if !hz.is_finite() || !caps.admits_bandwidth(bandwidth) {
        return Err(DeviceError::Unsupported(format!(
            "bandwidth {hz}: this receiver's channel filter spans {:?}",
            caps.bandwidth_ranges
        )));
    }
    let hz = hz.round();
    next.filter_hz = Some(hz as u64);
    batch.push(Command::Set(Property::FilterBandwidth, hz.to_string()));
    Ok(())
}

fn apply_gain(
    next: &mut Remote,
    batch: &mut Vec<Command>,
    caps: &Capabilities,
    gain: &GainValue,
) -> Result<(), DeviceError> {
    let stage = caps.stage(&gain.stage).ok_or_else(|| {
        DeviceError::Unsupported(format!(
            "gain stage {}: this receiver's RF gain is the {} state",
            gain.stage,
            GainKind::Lna.name()
        ))
    })?;
    if !gain.value_db.is_finite() || !stage.range.holds(gain.value_db) {
        return Err(DeviceError::Unsupported(format!(
            "gain stage {}: {} is not a state in {}..{}",
            gain.stage, gain.value_db, stage.range.min, stage.range.max
        )));
    }
    let state = gain.value_db.round();
    next.lna_state = Some(state as u64);
    batch.push(Command::Set(Property::LnaState, state.to_string()));
    Ok(())
}

fn apply_extra(
    next: &mut Remote,
    batch: &mut Vec<Command>,
    caps: &Capabilities,
    asked: &ExtraValue,
) -> Result<(), DeviceError> {
    let offered = offers(caps, &asked.name)
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", asked.name)))?;
    let refuse = || {
        DeviceError::Unsupported(format!(
            "extra setting {}: {} is not one this receiver takes",
            asked.name, asked.value
        ))
    };

    match asked.name.as_str() {
        name if name == Property::DeviceVfoFrequency.name() => {
            let ExtraSetting::Range { range, .. } = offered else {
                return Err(refuse());
            };
            let hz = asked
                .value
                .as_f64()
                .filter(|hz| hz.is_finite() && range.holds(*hz))
                .ok_or_else(refuse)?
                .round();
            next.vfo_hz = Some(hz as u64);
            batch.push(Command::Set(Property::DeviceVfoFrequency, hz.to_string()));
        }
        RECEIVER => {
            let ExtraSetting::Enum { options, .. } = offered else {
                return Err(refuse());
            };
            let receiver = Selection::of(asked.value.as_str().ok_or_else(refuse)?, options);
            batch.push(receiver.command(next.network_mode));
            next.receiver = Some(receiver);
        }
        NETWORK_MODE => {
            let name = asked.value.as_str().ok_or_else(refuse)?;
            next.network_mode = if name == AUTOMATIC {
                None
            } else {
                Some(NetworkMode::parse(name).ok_or_else(refuse)?)
            };
            if let Some(receiver) = &next.receiver {
                batch.push(receiver.command(next.network_mode));
            }
        }
        PROFILE => {
            let profile = asked.value.as_str().ok_or_else(refuse)?.trim();
            next.profile = (!profile.is_empty()).then(|| profile.to_string());
            if let Some(profile) = &next.profile {
                batch.push(Command::Emit(Event::ApplyDeviceProfile, profile.clone()));
            }
        }
        RECORDING => {
            let name = asked.value.as_str().ok_or_else(refuse)?;
            next.recording = if name == OFF {
                None
            } else {
                Some(Recording::parse(name).ok_or_else(refuse)?)
            };
            batch.push(match next.recording {
                Some(kind) => Command::Emit(Event::StartRecording, kind.name().to_string()),
                None => Command::Emit(Event::StopRecording, String::new()),
            });
        }
        name => return Err(DeviceError::Unsupported(format!("extra setting {name}"))),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Tuner;

    fn snapshot(steerable: bool) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let mut put = |property: Property, value: &str| snapshot.put(property, value);
        put(Property::ApiVersion, "1.0.3");
        put(
            Property::CanControl,
            if steerable { "true" } else { "false" },
        );
        put(Property::DeviceCenterFrequency, "100000000");
        put(Property::DeviceSampleRate, "2000000");
        put(Property::LnaState, "4");
        put(Property::LnaStateMin, "0");
        put(Property::LnaStateMax, "9");
        put(Property::DeviceVfoFrequency, "100100000");
        put(Property::FilterBandwidth, "12500");
        put(Property::DemodMaxBandwidth, "200000");
        put(Property::Demodulator, "NFM");
        put(Property::AudioVolumePercent, "80");
        put(Property::ValidAntennas, "Antenna A,Antenna B");
        put(Property::ActiveAntenna, "Antenna A");
        put(Property::ValidDevices, "RSPduo 1234,RSP1B 5678");
        put(Property::ActiveDevice, "RSP1B 5678");
        put(Property::Started, "false");
        snapshot
    }

    fn opened(steerable: bool) -> (Capabilities, Remote) {
        let snapshot = snapshot(steerable);
        (capabilities(&snapshot), Remote::new(&snapshot))
    }

    fn extra(name: &str, value: impl Into<serde_json::Value>) -> DeviceSettings {
        DeviceSettings {
            extra: vec![ExtraValue {
                name: name.to_string(),
                value: value.into(),
            }],
            ..DeviceSettings::default()
        }
    }

    fn lna(state: f64) -> DeviceSettings {
        DeviceSettings {
            gains: vec![GainValue::new(GainKind::Lna, state)],
            ..DeviceSettings::default()
        }
    }

    fn filter(hz: f64) -> DeviceSettings {
        DeviceSettings {
            bandwidth: Some(BandwidthSetting::Manual { hz }),
            ..DeviceSettings::default()
        }
    }

    #[test]
    fn a_steerable_receiver_offers_the_whole_rsp_range_and_its_gain_state() {
        let (caps, _) = opened(true);
        assert_eq!(caps.freq_ranges[0].min, MIN_FREQUENCY_HZ);
        assert_eq!(caps.freq_ranges[0].max, MAX_FREQUENCY_HZ);
        assert_eq!(caps.sample_rate_ranges[0].min, MIN_SAMPLE_RATE);
        assert!(caps.sample_rates.is_empty(), "the API names no rate menu");
        assert_eq!(caps.antennas, vec!["Antenna A", "Antenna B"]);
        let lna = caps.stage(GainKind::Lna.name()).expect("the RF gain state");
        assert_eq!((lna.range.min, lna.range.max), (0.0, 9.0));
        assert_eq!(lna.range.step, Some(1.0));
        assert_eq!(lna.unit, GainUnit::Index);
    }

    #[test]
    fn the_controls_are_the_ones_that_shape_the_iq_and_no_others() {
        let (caps, _) = opened(true);
        assert_eq!(
            caps.extra
                .iter()
                .map(ExtraSetting::name)
                .collect::<Vec<_>>(),
            vec![
                "device_vfo_frequency",
                RECEIVER,
                NETWORK_MODE,
                PROFILE,
                RECORDING
            ],
            "this receiver answered for its whole audio chain, and none of that is a control"
        );
        assert!(
            caps.extra.iter().all(|setting| setting.label().is_some()),
            "every oddity carries a label"
        );
        assert_eq!(
            caps.bandwidth_ranges,
            vec![stepped(MIN_FILTER_HZ, 200_000.0)],
            "as wide as the receiver's own demod_max_bandwidth, no wider"
        );
        assert!(
            !caps.bandwidth_auto,
            "the channel filter has no automatic width"
        );
    }

    #[test]
    fn a_channel_filter_with_no_published_ceiling_is_not_a_control() {
        let mut snapshot = snapshot(true);
        snapshot.put(Property::DemodMaxBandwidth, "");
        assert!(
            !capabilities(&snapshot).has_filter(),
            "a width with no known ceiling is a control that cannot be bounded"
        );
    }

    #[test]
    fn the_vfo_and_the_channel_filter_become_the_properties_the_api_names() {
        let (caps, remote) = opened(true);
        let (tuned, batch) = validate(
            &extra("device_vfo_frequency", 100_200_000.0),
            &caps,
            &remote,
        )
        .expect("inside the receiver's range");
        assert_eq!(
            batch,
            vec![Command::Set(
                Property::DeviceVfoFrequency,
                "100200000".to_string()
            )]
        );
        let (narrower, batch) = validate(&filter(12_500.0), &caps, &tuned).expect("a width");
        assert_eq!(
            batch,
            vec![Command::Set(Property::FilterBandwidth, "12500".to_string())]
        );
        assert_eq!(
            narrower.wire(&caps).bandwidth,
            Some(BandwidthSetting::Manual { hz: 12_500.0 })
        );
        assert!(
            narrower.replay().contains(&Command::Set(
                Property::FilterBandwidth,
                "12500".to_string()
            )),
            "the channel filter comes back with the connection"
        );
        assert!(
            validate(&filter(400_000.0), &caps, &remote).is_err(),
            "wider than the receiver said it demodulates"
        );
        assert!(
            validate(
                &DeviceSettings {
                    bandwidth: Some(BandwidthSetting::Auto),
                    ..DeviceSettings::default()
                },
                &caps,
                &remote
            )
            .is_err(),
            "the channel filter has no automatic width"
        );
    }

    #[test]
    fn the_lna_state_becomes_the_property_the_api_names() {
        let (caps, remote) = opened(true);
        let (next, batch) = validate(&lna(2.0), &caps, &remote).expect("a state");
        assert_eq!(
            batch,
            vec![Command::Set(Property::LnaState, "2".to_string())]
        );
        assert_eq!(next.wire(&caps).gain(GainKind::Lna.name()), Some(2.0));
        assert!(
            next.replay()
                .contains(&Command::Set(Property::LnaState, "2".to_string()))
        );
    }

    #[test]
    fn a_receiver_that_will_not_be_steered_reports_only_where_it_already_is() {
        let (caps, _) = opened(false);
        assert_eq!(caps.freq_ranges, pinned(Some(100e6)));
        assert_eq!(caps.sample_rate_ranges, pinned(Some(2e6)));
        assert!(
            caps.gains.is_empty(),
            "a gain the server will refuse is not offered"
        );
    }

    #[test]
    fn a_fresh_device_starts_where_the_server_already_is() {
        let (caps, remote) = opened(true);
        let wire = remote.wire(&caps);
        assert_eq!(wire.center_hz, Some(100e6));
        assert_eq!(wire.sample_rate, Some(2e6));
        assert_eq!(wire.antenna.as_deref(), Some("Antenna A"));
        let value = |name: &str| {
            wire.extra
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.value.clone())
        };
        assert_eq!(wire.gain(GainKind::Lna.name()), Some(4.0));
        assert_eq!(
            wire.bandwidth,
            Some(BandwidthSetting::Manual { hz: 12_500.0 })
        );
        assert_eq!(
            value("device_vfo_frequency"),
            Some(serde_json::json!(100_100_000u64))
        );
        assert_eq!(value(RECEIVER), Some(serde_json::json!("RSP1B 5678")));
        assert_eq!(value(RECORDING), Some(serde_json::json!("off")));
        assert_eq!(value(NETWORK_MODE), Some(serde_json::json!("automatic")));
    }

    #[test]
    fn starting_sets_the_receiver_up_before_it_turns_the_iq_on() {
        let (_, remote) = opened(true);
        let names: Vec<String> = remote
            .start()
            .iter()
            .map(|command| command.encode(Tuner::Primary).expect("encodes"))
            .collect();
        let at = |needle: &str| {
            names
                .iter()
                .position(|message| message.contains(needle))
                .unwrap_or_else(|| panic!("{needle} was never sent"))
        };
        assert!(at("device_sample_rate") < at("iq_stream_enable"));
        assert!(at("device_center_frequency") < at("iq_stream_enable"));
        assert!(at("selected_device_name") < at("device_sample_rate"));
        assert!(at("device_stream_enable") < at("iq_stream_enable"));
        assert!(
            names
                .iter()
                .any(|m| m.contains(r#""audio_stream_enable","property":"","value":"false""#)),
            "the link carries IQ and nothing SDR-- would throw away: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|m| m.contains(r#""spectrum_enable","property":"","value":"false""#))
        );
    }

    #[test]
    fn stopping_leaves_a_session_that_was_already_running_alone() {
        let (_, mut remote) = opened(true);
        assert!(
            remote.stop().contains(&Command::Emit(
                Event::DeviceStreamEnable,
                "false".to_string()
            )),
            "a receiver SDR-- started is stopped again"
        );
        remote.streaming = true;
        assert!(
            !remote
                .stop()
                .iter()
                .any(|command| matches!(command, Command::Emit(Event::DeviceStreamEnable, _))),
            "an operator's own session is left running"
        );
    }

    #[test]
    fn a_retune_becomes_the_property_the_api_names() {
        let (caps, remote) = opened(true);
        let (next, batch) = validate(
            &DeviceSettings {
                center_hz: Some(144_800_000.0),
                sample_rate: Some(6_000_000.0),
                ..DeviceSettings::default()
            },
            &caps,
            &remote,
        )
        .expect("in range");
        assert_eq!(
            batch,
            vec![
                Command::Set(Property::DeviceCenterFrequency, "144800000".to_string()),
                Command::Set(Property::DeviceSampleRate, "6000000".to_string()),
            ]
        );
        assert_eq!(next.wire(&caps).center_hz, Some(144.8e6));
    }

    #[test]
    fn settings_this_receiver_has_no_control_for_are_refused_by_name() {
        let (caps, remote) = opened(true);
        let refused = |delta: DeviceSettings| {
            validate(&delta, &caps, &remote)
                .expect_err("refused")
                .to_string()
        };
        assert!(
            refused(DeviceSettings {
                center_hz: Some(3e9),
                ..DeviceSettings::default()
            })
            .contains("outside what this receiver tunes to")
        );
        assert!(
            refused(DeviceSettings {
                sample_rate: Some(20e6),
                ..DeviceSettings::default()
            })
            .contains("outside what this receiver samples at")
        );
        assert!(refused(filter(400e3)).contains("channel filter"));
        assert!(
            refused(DeviceSettings {
                ppm: Some(2.0),
                ..DeviceSettings::default()
            })
            .contains("no frequency correction")
        );
        assert!(
            refused(DeviceSettings {
                antenna: Some("Antenna Z".to_string()),
                ..DeviceSettings::default()
            })
            .contains("Antenna Z")
        );
        assert!(refused(lna(99.0)).contains("LNA"));
        assert!(refused(extra("lna", 2)).contains("lna"));
        assert!(refused(extra("filter_bandwidth", 12_500)).contains("filter_bandwidth"));
        assert!(
            refused(extra("audio_volume_percent", 50)).contains("audio_volume_percent"),
            "SDRconnect's audio chain is not a setting on a device that hands over IQ"
        );
        assert!(refused(extra("demodulator", "WFM")).contains("demodulator"));
        assert!(refused(extra("warp", 1)).contains("warp"));
    }

    #[test]
    fn a_receiver_is_named_the_way_the_operator_named_it() {
        let (caps, remote) = opened(true);
        let select = |value: &str| {
            validate(&extra(RECEIVER, value), &caps, &remote)
                .expect("a radio")
                .1
        };
        assert_eq!(
            select("RSPduo 1234"),
            vec![Command::Emit(
                Event::SelectedDeviceName,
                "RSPduo 1234".to_string()
            )],
            "a name from the list is sent as a name"
        );
        assert_eq!(
            select("1"),
            vec![Command::Emit(Event::SelectedDevice, "1".to_string())],
            "a slot in the published list is sent as an index"
        );
        assert_eq!(
            select("1801010101"),
            vec![Command::Emit(
                Event::SelectedDeviceSerial,
                "1801010101".to_string()
            )],
            "a serial number that is all digits is longer than the list, and stays a serial"
        );
        assert_eq!(
            select("2001T02AB3"),
            vec![Command::Emit(
                Event::SelectedDeviceSerial,
                "2001T02AB3".to_string()
            )]
        );
    }

    #[test]
    fn choosing_a_receiver_and_a_network_mode_names_both_in_one_selection() {
        let (caps, remote) = opened(true);
        let (next, _) = validate(&extra(RECEIVER, "RSPduo 1234"), &caps, &remote).expect("a radio");
        let (with_mode, batch) =
            validate(&extra(NETWORK_MODE, "IQ Lite"), &caps, &next).expect("a mode the API names");
        assert_eq!(
            batch,
            vec![Command::Emit(
                Event::SelectedDeviceName,
                "RSPduo 1234:IQ Lite".to_string()
            )]
        );
        let (by_index, _) = validate(&extra(RECEIVER, "0"), &caps, &with_mode).expect("a slot");
        assert_eq!(
            by_index.replay().first(),
            Some(&Command::Emit(Event::SelectedDevice, "0".to_string())),
            "an index names a slot, and carries no mode"
        );
        assert!(validate(&extra(NETWORK_MODE, "Fast"), &caps, &next).is_err());
    }

    #[test]
    fn a_recording_starts_and_stops_with_the_setting() {
        let (caps, remote) = opened(true);
        let (recording, batch) =
            validate(&extra(RECORDING, "iq"), &caps, &remote).expect("a kind the API names");
        assert_eq!(
            batch,
            vec![Command::Emit(Event::StartRecording, "iq".to_string())]
        );
        assert!(
            recording
                .start()
                .contains(&Command::Emit(Event::StartRecording, "iq".to_string()))
        );
        assert!(
            recording
                .stop()
                .contains(&Command::Emit(Event::StopRecording, String::new()))
        );
        let (off, batch) =
            validate(&extra(RECORDING, OFF), &caps, &recording).expect("switching it off");
        assert_eq!(
            batch,
            vec![Command::Emit(Event::StopRecording, String::new())]
        );
        assert!(
            !off.stop()
                .contains(&Command::Emit(Event::StopRecording, String::new()))
        );
        assert!(validate(&extra(RECORDING, "video"), &caps, &remote).is_err());
    }

    #[test]
    fn a_profile_is_applied_once_and_replayed_onto_a_new_connection() {
        let (caps, remote) = opened(true);
        let (next, batch) =
            validate(&extra(PROFILE, "Airband"), &caps, &remote).expect("a profile name");
        assert_eq!(
            batch,
            vec![Command::Emit(
                Event::ApplyDeviceProfile,
                "Airband".to_string()
            )]
        );
        assert!(next.replay().contains(&Command::Emit(
            Event::ApplyDeviceProfile,
            "Airband".to_string()
        )));
        let (cleared, batch) = validate(&extra(PROFILE, " "), &caps, &next).expect("cleared");
        assert!(batch.is_empty());
        assert!(
            !cleared
                .replay()
                .iter()
                .any(|command| matches!(command, Command::Emit(Event::ApplyDeviceProfile, _)))
        );
    }
}
