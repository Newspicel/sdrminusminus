use sdrmm_device::{DeviceError, net::Endpoint};
use serde::Serialize;

pub(crate) const DEFAULT_PORT: u16 = 5454;

pub(crate) const PATH: &str = "/";

/// Every binary message opens with the payload type as a little-endian `u16`.
pub(crate) const PAYLOAD_PREFIX: usize = 2;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Tuner {
    #[default]
    Primary,
    Secondary,
}

impl Tuner {
    pub(crate) const ALL: [Self; 2] = [Self::Primary, Self::Secondary];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|tuner| tuner.name().eq_ignore_ascii_case(name.trim()))
    }

    pub(crate) fn enable(self) -> Event {
        match self {
            Self::Primary => Event::PrimaryDeviceEnable,
            Self::Secondary => Event::SecondaryDeviceEnable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    PropertyChanged,
    GetPropertyResponse,
    SetProperty,
    GetProperty,
    IqStreamEnable,
    AudioStreamEnable,
    SpectrumEnable,
    DeviceStreamEnable,
    SelectedDevice,
    SelectedDeviceSerial,
    SelectedDeviceName,
    StartRecording,
    StopRecording,
    ApplyDeviceProfile,
    PrimaryDeviceEnable,
    SecondaryDeviceEnable,
}

impl Event {
    pub(crate) const ALL: [Self; 16] = [
        Self::PropertyChanged,
        Self::GetPropertyResponse,
        Self::SetProperty,
        Self::GetProperty,
        Self::IqStreamEnable,
        Self::AudioStreamEnable,
        Self::SpectrumEnable,
        Self::DeviceStreamEnable,
        Self::SelectedDevice,
        Self::SelectedDeviceSerial,
        Self::SelectedDeviceName,
        Self::StartRecording,
        Self::StopRecording,
        Self::ApplyDeviceProfile,
        Self::PrimaryDeviceEnable,
        Self::SecondaryDeviceEnable,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::PropertyChanged => "property_changed",
            Self::GetPropertyResponse => "get_property_response",
            Self::SetProperty => "set_property",
            Self::GetProperty => "get_property",
            Self::IqStreamEnable => "iq_stream_enable",
            Self::AudioStreamEnable => "audio_stream_enable",
            Self::SpectrumEnable => "spectrum_enable",
            Self::DeviceStreamEnable => "device_stream_enable",
            Self::SelectedDevice => "selected_device",
            Self::SelectedDeviceSerial => "selected_device_serial",
            Self::SelectedDeviceName => "selected_device_name",
            Self::StartRecording => "start_recording",
            Self::StopRecording => "stop_recording",
            Self::ApplyDeviceProfile => "apply_device_profile",
            Self::PrimaryDeviceEnable => "set_primary_device_enable",
            Self::SecondaryDeviceEnable => "set_secondary_device_enable",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.name() == name)
    }

    /// Whether the message is addressed to one tuner of a dual-tuner receiver.
    fn tuned(self) -> bool {
        matches!(
            self,
            Self::SetProperty | Self::GetProperty | Self::DeviceStreamEnable
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ValueKind {
    Boolean,
    Unsigned,
    Decimal,
    Percent,
    Text,
    Mode,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Property {
    DeviceCenterFrequency,
    DeviceSampleRate,
    DeviceVfoFrequency,
    LnaState,
    LnaStateMin,
    LnaStateMax,
    FilterBandwidth,
    Demodulator,
    DemodMaxBandwidth,
    Started,
    Overload,
    CanControl,
    AudioVolumePercent,
    AudioMute,
    AudioLimiters,
    AudioFilter,
    SquelchEnable,
    SquelchThreshold,
    AgcEnable,
    AgcThreshold,
    WfmStereoEnable,
    NoiseReductionEnable,
    NoiseReductionStrength,
    NfmDeemphasisEnable,
    SpectrumRefLevel,
    SpectrumBase,
    RdsPs,
    RdsPi,
    RdsPty,
    RdsRadiotext,
    RdsEnable,
    SignalPower,
    SignalSnr,
    WfmStereo,
    AmLowcutFrequency,
    SsbLowcutFrequency,
    NfmLowcutFrequency,
    ValidAntennas,
    ActiveAntenna,
    ValidDevices,
    ActiveDevice,
    ApiVersion,
}

impl Property {
    pub(crate) const ALL: [Self; 42] = [
        Self::DeviceCenterFrequency,
        Self::DeviceSampleRate,
        Self::DeviceVfoFrequency,
        Self::LnaState,
        Self::LnaStateMin,
        Self::LnaStateMax,
        Self::FilterBandwidth,
        Self::Demodulator,
        Self::DemodMaxBandwidth,
        Self::Started,
        Self::Overload,
        Self::CanControl,
        Self::AudioVolumePercent,
        Self::AudioMute,
        Self::AudioLimiters,
        Self::AudioFilter,
        Self::SquelchEnable,
        Self::SquelchThreshold,
        Self::AgcEnable,
        Self::AgcThreshold,
        Self::WfmStereoEnable,
        Self::NoiseReductionEnable,
        Self::NoiseReductionStrength,
        Self::NfmDeemphasisEnable,
        Self::SpectrumRefLevel,
        Self::SpectrumBase,
        Self::RdsPs,
        Self::RdsPi,
        Self::RdsPty,
        Self::RdsRadiotext,
        Self::RdsEnable,
        Self::SignalPower,
        Self::SignalSnr,
        Self::WfmStereo,
        Self::AmLowcutFrequency,
        Self::SsbLowcutFrequency,
        Self::NfmLowcutFrequency,
        Self::ValidAntennas,
        Self::ActiveAntenna,
        Self::ValidDevices,
        Self::ActiveDevice,
        Self::ApiVersion,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::DeviceCenterFrequency => "device_center_frequency",
            Self::DeviceSampleRate => "device_sample_rate",
            Self::DeviceVfoFrequency => "device_vfo_frequency",
            Self::LnaState => "lna_state",
            Self::LnaStateMin => "lna_state_min",
            Self::LnaStateMax => "lna_state_max",
            Self::FilterBandwidth => "filter_bandwidth",
            Self::Demodulator => "demodulator",
            Self::DemodMaxBandwidth => "demod_max_bandwidth",
            Self::Started => "started",
            Self::Overload => "overload",
            Self::CanControl => "can_control",
            Self::AudioVolumePercent => "audio_volume_percent",
            Self::AudioMute => "audio_mute",
            Self::AudioLimiters => "audio_limiters",
            Self::AudioFilter => "audio_filter",
            Self::SquelchEnable => "squelch_enable",
            Self::SquelchThreshold => "squelch_threshold",
            Self::AgcEnable => "agc_enable",
            Self::AgcThreshold => "agc_threshold",
            Self::WfmStereoEnable => "wfm_stereo_enable",
            Self::NoiseReductionEnable => "noise_reduction_enable",
            Self::NoiseReductionStrength => "noise_reduction_strength",
            Self::NfmDeemphasisEnable => "nfm_deemphasis_enable",
            Self::SpectrumRefLevel => "spectrum_ref_level",
            Self::SpectrumBase => "spectrum_base",
            Self::RdsPs => "rds_ps",
            Self::RdsPi => "rds_pi",
            Self::RdsPty => "rds_pty",
            Self::RdsRadiotext => "rds_radiotext",
            Self::RdsEnable => "rds_enable",
            Self::SignalPower => "signal_power",
            Self::SignalSnr => "signal_snr",
            Self::WfmStereo => "wfm_stereo",
            Self::AmLowcutFrequency => "am_lowcut_frequency",
            Self::SsbLowcutFrequency => "ssb_lowcut_frequency",
            Self::NfmLowcutFrequency => "nfm_lowcut_frequency",
            Self::ValidAntennas => "valid_antennas",
            Self::ActiveAntenna => "active_antenna",
            Self::ValidDevices => "valid_devices",
            Self::ActiveDevice => "active_device",
            Self::ApiVersion => "api_version",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|property| property.name() == name)
    }

    pub(crate) fn read_only(self) -> bool {
        matches!(
            self,
            Self::LnaStateMin
                | Self::LnaStateMax
                | Self::DemodMaxBandwidth
                | Self::Started
                | Self::Overload
                | Self::CanControl
                | Self::RdsPs
                | Self::RdsPi
                | Self::RdsPty
                | Self::RdsRadiotext
                | Self::SignalPower
                | Self::SignalSnr
                | Self::WfmStereo
                | Self::ValidAntennas
                | Self::ValidDevices
                | Self::ActiveDevice
                | Self::ApiVersion
        )
    }

    pub(crate) fn kind(self) -> ValueKind {
        match self {
            Self::DeviceCenterFrequency
            | Self::DeviceVfoFrequency
            | Self::LnaState
            | Self::LnaStateMin
            | Self::LnaStateMax
            | Self::FilterBandwidth
            | Self::DemodMaxBandwidth
            | Self::RdsPi
            | Self::RdsPty
            | Self::AmLowcutFrequency
            | Self::SsbLowcutFrequency
            | Self::NfmLowcutFrequency => ValueKind::Unsigned,
            Self::DeviceSampleRate
            | Self::SquelchThreshold
            | Self::AgcThreshold
            | Self::SpectrumRefLevel
            | Self::SpectrumBase
            | Self::SignalPower
            | Self::SignalSnr => ValueKind::Decimal,
            Self::AudioVolumePercent | Self::NoiseReductionStrength => ValueKind::Percent,
            Self::Started
            | Self::Overload
            | Self::CanControl
            | Self::AudioMute
            | Self::AudioLimiters
            | Self::AudioFilter
            | Self::SquelchEnable
            | Self::AgcEnable
            | Self::WfmStereoEnable
            | Self::NoiseReductionEnable
            | Self::NfmDeemphasisEnable
            | Self::RdsEnable
            | Self::WfmStereo => ValueKind::Boolean,
            Self::Demodulator => ValueKind::Mode,
            Self::RdsPs
            | Self::RdsRadiotext
            | Self::ValidAntennas
            | Self::ActiveAntenna
            | Self::ValidDevices
            | Self::ActiveDevice
            | Self::ApiVersion => ValueKind::Text,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Demodulator {
    Am,
    Usb,
    Lsb,
    Cw,
    Sam,
    Nfm,
    Wfm,
}

impl Demodulator {
    pub(crate) const ALL: [Self; 7] = [
        Self::Am,
        Self::Usb,
        Self::Lsb,
        Self::Cw,
        Self::Sam,
        Self::Nfm,
        Self::Wfm,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Am => "AM",
            Self::Usb => "USB",
            Self::Lsb => "LSB",
            Self::Cw => "CW",
            Self::Sam => "SAM",
            Self::Nfm => "NFM",
            Self::Wfm => "WFM",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// How much of the IQ a networked SDRconnect device sends on, chosen when the device is selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkMode {
    FullIq,
    IqLite,
    Compact,
}

impl NetworkMode {
    pub(crate) const ALL: [Self; 3] = [Self::FullIq, Self::IqLite, Self::Compact];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::FullIq => "Full IQ",
            Self::IqLite => "IQ Lite",
            Self::Compact => "Compact",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|mode| mode.name().eq_ignore_ascii_case(name.trim()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Recording {
    Iq,
    Audio,
    CompressedAudio,
}

impl Recording {
    pub(crate) const ALL: [Self; 3] = [Self::Iq, Self::Audio, Self::CompressedAudio];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Iq => "iq",
            Self::Audio => "audio",
            Self::CompressedAudio => "compressed_audio",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().eq_ignore_ascii_case(name.trim()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PayloadKind {
    Audio,
    Iq,
    Spectrum,
}

impl PayloadKind {
    const ALL: [Self; 3] = [Self::Audio, Self::Iq, Self::Spectrum];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Payload {
    pub(crate) kind: PayloadKind,
    pub(crate) tuner: Tuner,
}

impl Payload {
    pub(crate) fn from_code(code: u16) -> Option<Self> {
        Tuner::ALL
            .into_iter()
            .flat_map(|tuner| PayloadKind::ALL.map(|kind| Self { kind, tuner }))
            .find(|payload| payload.code() == code)
    }

    pub(crate) fn code(self) -> u16 {
        let kind = match self.kind {
            PayloadKind::Audio => 1,
            PayloadKind::Iq => 2,
            PayloadKind::Spectrum => 3,
        };
        match self.tuner {
            Tuner::Primary => kind,
            Tuner::Secondary => kind + 3,
        }
    }

    /// Reads the type off the front of a binary message, leaving the samples behind it.
    pub(crate) fn split(message: &[u8]) -> Option<(Self, &[u8])> {
        let (prefix, body) = message.split_at_checked(PAYLOAD_PREFIX)?;
        let payload = Self::from_code(u16::from_le_bytes([prefix[0], prefix[1]]))?;
        Some((payload, body))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Set(Property, String),
    Get(Property),
    Emit(Event, String),
}

impl Command {
    pub(crate) fn encode(&self, tuner: Tuner) -> Result<String, DeviceError> {
        let (event, property, value) = match self {
            Self::Set(property, value) => (Event::SetProperty, property.name(), value.as_str()),
            Self::Get(property) => (Event::GetProperty, property.name(), ""),
            Self::Emit(event, value) => (*event, "", value.as_str()),
        };
        if let Self::Set(property, value) = self {
            if property.read_only() {
                return Err(DeviceError::Unsupported(format!(
                    "{} is read-only in the SDRconnect API",
                    property.name()
                )));
            }
            check(*property, value)?;
        }
        encode(event, property, value, event.tuned().then_some(tuner))
    }
}

/// Refuses a value the property cannot hold before it costs a round trip to find out.
fn check(property: Property, value: &str) -> Result<(), DeviceError> {
    let holds = match property.kind() {
        ValueKind::Boolean => as_bool(value).is_some(),
        ValueKind::Unsigned => as_u64(value).is_some(),
        ValueKind::Decimal => as_f64(value).is_some(),
        ValueKind::Percent => as_f64(value).is_some_and(|percent| (0.0..=100.0).contains(&percent)),
        ValueKind::Mode => Demodulator::parse(value).is_some(),
        ValueKind::Text => true,
    };
    if holds {
        Ok(())
    } else {
        Err(DeviceError::Unsupported(format!(
            "{} does not take {value:?}",
            property.name()
        )))
    }
}

#[derive(Debug, Serialize)]
struct Envelope<'a> {
    event_type: &'a str,
    property: &'a str,
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    device: Option<&'a str>,
}

pub(crate) fn encode(
    event: Event,
    property: &str,
    value: &str,
    device: Option<Tuner>,
) -> Result<String, DeviceError> {
    serde_json::to_string(&Envelope {
        event_type: event.name(),
        property,
        value,
        device: device.map(Tuner::name),
    })
    .map_err(|e| DeviceError::Io(format!("encoding an SDRconnect message: {e}")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Notification {
    pub(crate) event: Event,
    pub(crate) property: Option<Property>,
    pub(crate) value: String,
    pub(crate) tuner: Tuner,
}

/// Reads one message from SDRconnect, tolerating a value sent as a bare number rather than a
/// string and an envelope that carries fields this does not act on.
pub(crate) fn decode(text: &str) -> Result<Notification, String> {
    let message: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let field = |name: &str| message.get(name).map(scalar).unwrap_or_default();
    let event_type = field("event_type");
    let event = Event::parse(&event_type)
        .ok_or_else(|| format!("event_type {event_type:?} is not one the API defines"))?;
    let property = field("property");
    Ok(Notification {
        event,
        property: Property::parse(&property),
        value: field("value"),
        tuner: Tuner::parse(&field("device")).unwrap_or_default(),
    })
}

fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(crate) fn flag(on: bool) -> String {
    if on { "true" } else { "false" }.to_string()
}

pub(crate) fn as_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "1" | "True" | "TRUE" => Some(true),
        "false" | "0" | "False" | "FALSE" => Some(false),
        _ => None,
    }
}

pub(crate) fn as_f64(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

pub(crate) fn as_u64(value: &str) -> Option<u64> {
    as_f64(value)
        .filter(|v| *v >= 0.0 && *v <= 2f64.powi(53))
        .map(|v| v.round() as u64)
}

pub(crate) fn as_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Where one SDRconnect tuner lives: the host serving the API, and which of its tuners to take.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Address {
    pub(crate) endpoint: Endpoint,
    pub(crate) tuner: Tuner,
}

impl Address {
    pub(crate) fn parse(key: &str) -> Result<Self, DeviceError> {
        let key = key.trim();
        if key.starts_with("wss://") {
            return Err(DeviceError::NotFound(format!(
                "{key}: the SDRconnect API is served over plain ws://, not TLS"
            )));
        }
        let key = key.strip_prefix("ws://").unwrap_or(key);
        let (host, tuner) = match key.rsplit_once('/') {
            Some((host, "")) => (host, Tuner::default()),
            Some((host, suffix)) => {
                let tuner = Tuner::parse(suffix).ok_or_else(|| {
                    DeviceError::NotFound(format!(
                        "{key}: {suffix:?} is not a tuner; use primary or secondary"
                    ))
                })?;
                (host, tuner)
            }
            None => (key, Tuner::default()),
        };
        Ok(Self {
            endpoint: Endpoint::parse(host, DEFAULT_PORT)?,
            tuner,
        })
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.tuner {
            Tuner::Primary => write!(f, "{}", self.endpoint),
            Tuner::Secondary => write!(f, "{}/{}", self.endpoint, self.tuner.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_event_and_property_the_specification_lists_round_trips_through_its_name() {
        for event in Event::ALL {
            assert_eq!(Event::parse(event.name()), Some(event));
        }
        assert_eq!(Event::ALL.len(), 16);
        assert_eq!(Event::parse("teleport"), None);

        for property in Property::ALL {
            assert_eq!(Property::parse(property.name()), Some(property));
        }
        assert_eq!(Property::ALL.len(), 42);
        assert_eq!(Property::parse("warp_factor"), None);
    }

    #[test]
    fn the_read_only_properties_are_the_ones_the_specification_marks() {
        assert_eq!(
            Property::ALL
                .into_iter()
                .filter(|property| property.read_only())
                .map(Property::name)
                .collect::<Vec<_>>(),
            vec![
                "lna_state_min",
                "lna_state_max",
                "demod_max_bandwidth",
                "started",
                "overload",
                "can_control",
                "rds_ps",
                "rds_pi",
                "rds_pty",
                "rds_radiotext",
                "signal_power",
                "signal_snr",
                "wfm_stereo",
                "valid_antennas",
                "valid_devices",
                "active_device",
                "api_version",
            ]
        );
    }

    #[test]
    fn setting_a_read_only_property_is_refused_before_it_reaches_the_wire() {
        let refused = Command::Set(Property::ApiVersion, "9".to_string()).encode(Tuner::Primary);
        assert!(refused.is_err_and(|e| e.to_string().contains("read-only")));
        assert!(
            Command::Get(Property::ApiVersion)
                .encode(Tuner::Primary)
                .is_ok()
        );
    }

    #[test]
    fn a_property_message_carries_the_tuner_and_a_global_one_does_not() {
        let set = Command::Set(Property::DeviceCenterFrequency, "101000000".to_string())
            .encode(Tuner::Secondary)
            .expect("encodes");
        assert_eq!(
            set,
            r#"{"event_type":"set_property","property":"device_center_frequency","value":"101000000","device":"secondary"}"#
        );

        let enable = Command::Emit(Event::AudioStreamEnable, flag(true))
            .encode(Tuner::Secondary)
            .expect("encodes");
        assert_eq!(
            enable, r#"{"event_type":"audio_stream_enable","property":"","value":"true"}"#,
            "a stream switch is not addressed to one tuner"
        );

        let stream = Command::Emit(Event::DeviceStreamEnable, flag(true))
            .encode(Tuner::Secondary)
            .expect("encodes");
        assert!(stream.contains(r#""device":"secondary""#));
    }

    #[test]
    fn a_name_with_a_quote_in_it_is_escaped_rather_than_breaking_the_message() {
        let command = Command::Emit(
            Event::ApplyDeviceProfile,
            "profile \"one\"\\two".to_string(),
        )
        .encode(Tuner::Primary)
        .expect("encodes");
        let back = decode(&command).expect("decodes");
        assert_eq!(back.value, "profile \"one\"\\two");
    }

    #[test]
    fn a_push_message_from_the_specification_is_read_the_way_it_is_written() {
        let notification = decode(
            r#"{"event_type":"property_changed","property":"device_center_frequency","device":"primary","value":"100000000"}"#,
        )
        .expect("decodes");
        assert_eq!(notification.event, Event::PropertyChanged);
        assert_eq!(notification.property, Some(Property::DeviceCenterFrequency));
        assert_eq!(notification.value, "100000000");
        assert_eq!(notification.tuner, Tuner::Primary);

        let response = decode(
            r#"{"event_type":"get_property_response","property":"device_vfo_frequency","device":"secondary","value":"101000000"}"#,
        )
        .expect("decodes");
        assert_eq!(response.event, Event::GetPropertyResponse);
        assert_eq!(response.tuner, Tuner::Secondary);
    }

    #[test]
    fn a_message_without_a_device_field_is_the_primary_tuner() {
        let notification =
            decode(r#"{"event_type":"property_changed","property":"started","value":"true"}"#)
                .expect("decodes");
        assert_eq!(notification.tuner, Tuner::Primary);
        assert_eq!(as_bool(&notification.value), Some(true));
    }

    #[test]
    fn a_value_sent_as_a_number_is_read_as_the_string_the_specification_asks_for() {
        let notification = decode(
            r#"{"event_type":"property_changed","property":"signal_snr","value":12.5,"extra":null}"#,
        )
        .expect("decodes");
        assert_eq!(notification.value, "12.5");
        assert_eq!(as_f64(&notification.value), Some(12.5));
    }

    #[test]
    fn a_message_this_cannot_act_on_is_reported_rather_than_guessed_at() {
        assert!(decode("not json").is_err_and(|e| e.contains("not JSON")));
        assert!(decode(r#"{"event_type":"nonsense"}"#).is_err_and(|e| e.contains("nonsense")));
        let unknown = decode(r#"{"event_type":"property_changed","property":"tea","value":"1"}"#)
            .expect("an unknown property is still a message this frames");
        assert_eq!(unknown.property, None);
    }

    #[test]
    fn the_binary_payload_types_are_the_six_the_specification_numbers() {
        let expected = [
            (1, PayloadKind::Audio, Tuner::Primary),
            (2, PayloadKind::Iq, Tuner::Primary),
            (3, PayloadKind::Spectrum, Tuner::Primary),
            (4, PayloadKind::Audio, Tuner::Secondary),
            (5, PayloadKind::Iq, Tuner::Secondary),
            (6, PayloadKind::Spectrum, Tuner::Secondary),
        ];
        for (code, kind, tuner) in expected {
            let payload = Payload::from_code(code).expect("a defined payload type");
            assert_eq!(payload, Payload { kind, tuner });
            assert_eq!(payload.code(), code);
        }
        assert_eq!(Payload::from_code(0), None);
        assert_eq!(Payload::from_code(7), None);
    }

    #[test]
    fn a_binary_message_splits_into_its_type_and_its_samples() {
        let mut message = 5u16.to_le_bytes().to_vec();
        message.extend_from_slice(&[1, 2, 3, 4]);
        let (payload, body) = Payload::split(&message).expect("a typed message");
        assert_eq!(payload.kind, PayloadKind::Iq);
        assert_eq!(payload.tuner, Tuner::Secondary);
        assert_eq!(body, &[1, 2, 3, 4]);

        assert!(
            Payload::split(&[1]).is_none(),
            "a message shorter than its type"
        );
        assert!(Payload::split(&9u16.to_le_bytes()).is_none());
    }

    #[test]
    fn values_are_read_the_way_the_api_writes_them() {
        assert_eq!(as_bool("true"), Some(true));
        assert_eq!(as_bool(" False "), Some(false));
        assert_eq!(as_bool("yes"), None);
        assert_eq!(as_u64("100000000"), Some(100_000_000));
        assert_eq!(as_u64("2000000.0"), Some(2_000_000));
        assert_eq!(as_u64("-1"), None);
        assert_eq!(as_f64("nan"), None);
        assert_eq!(
            as_list(" Antenna A, Antenna B ,, "),
            vec!["Antenna A", "Antenna B"]
        );
        assert!(as_list("").is_empty());
    }

    #[test]
    fn the_enumerations_round_trip_through_the_names_the_api_uses() {
        for mode in Demodulator::ALL {
            assert_eq!(Demodulator::parse(mode.name()), Some(mode));
        }
        assert_eq!(Demodulator::parse("nfm"), Some(Demodulator::Nfm));
        assert_eq!(Demodulator::parse("FT8"), None);
        for mode in NetworkMode::ALL {
            assert_eq!(NetworkMode::parse(mode.name()), Some(mode));
        }
        assert_eq!(NetworkMode::parse("full iq"), Some(NetworkMode::FullIq));
        for kind in Recording::ALL {
            assert_eq!(Recording::parse(kind.name()), Some(kind));
        }
        assert_eq!(Recording::parse("video"), None);
    }

    #[test]
    fn an_address_names_the_host_and_the_tuner_behind_it() {
        let primary = Address::parse("radio.local").expect("parses");
        assert_eq!(primary.to_string(), "radio.local:5454");
        assert_eq!(primary.tuner, Tuner::Primary);

        let secondary = Address::parse("192.168.1.5:5454/secondary").expect("parses");
        assert_eq!(secondary.to_string(), "192.168.1.5:5454/secondary");
        assert_eq!(secondary.tuner, Tuner::Secondary);

        assert_eq!(
            Address::parse("[::1]/primary").expect("parses").to_string(),
            "[::1]:5454",
            "the primary tuner is the plain endpoint"
        );
        assert_eq!(
            Address::parse("ws://radio.local:5454/")
                .expect("the address the SDRconnect documentation prints")
                .to_string(),
            "radio.local:5454"
        );
        assert!(Address::parse("radio.local/third").is_err());
        assert!(Address::parse("radio.local:0").is_err());
        assert!(
            Address::parse("wss://radio.local").is_err_and(|e| e.to_string().contains("TLS")),
            "a scheme this cannot speak is named rather than dialled"
        );
    }
}
