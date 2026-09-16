use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    ArgumentOption, Capabilities, Coherence, DcArtifact, DeviceSettings, Duplex, ExtraSetting,
    ExtraValue, Range, StreamScope,
};

use crate::{
    proto::{Command, Event, NetworkMode, Property, Recording, flag},
    session::Snapshot,
};

pub(crate) const LNA_STATE: &str = "lna_state";
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

pub(crate) fn capabilities(snapshot: &Snapshot) -> Capabilities {
    let steerable = snapshot.boolean(Property::CanControl).unwrap_or(true);
    let receivers = snapshot.list(Property::ValidDevices);

    let mut extra = Vec::new();
    if steerable && let Some((min, max)) = lna_range(snapshot) {
        extra.push(ExtraSetting::Range {
            name: LNA_STATE.to_string(),
            range: Range {
                min,
                max,
                step: Some(1.0),
            },
            unit: "state".to_string(),
        });
    }
    if !receivers.is_empty() {
        let active = snapshot
            .text(Property::ActiveDevice)
            .filter(|name| receivers.iter().any(|known| known == name))
            .unwrap_or(&receivers[0])
            .to_string();
        extra.push(ExtraSetting::Enum {
            name: RECEIVER.to_string(),
            options: receivers
                .iter()
                .map(|name| ArgumentOption::plain(name.as_str()))
                .collect(),
            default: active,
        });
        extra.push(ExtraSetting::Enum {
            name: NETWORK_MODE.to_string(),
            options: std::iter::once(ArgumentOption::plain(AUTOMATIC))
                .chain(
                    NetworkMode::ALL
                        .into_iter()
                        .map(|mode| ArgumentOption::plain(mode.name())),
                )
                .collect(),
            default: AUTOMATIC.to_string(),
        });
    }
    extra.push(ExtraSetting::String {
        name: PROFILE.to_string(),
        default: String::new(),
    });
    extra.push(ExtraSetting::Enum {
        name: RECORDING.to_string(),
        options: std::iter::once(ArgumentOption::plain(OFF))
            .chain(
                Recording::ALL
                    .into_iter()
                    .map(|kind| ArgumentOption::plain(kind.name())),
            )
            .collect(),
        default: OFF.to_string(),
    });

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
        gains: Vec::new(),
        antennas: snapshot.list(Property::ValidAntennas),
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra,
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

fn lna_range(snapshot: &Snapshot) -> Option<(f64, f64)> {
    let min = snapshot.count(Property::LnaStateMin).unwrap_or(0);
    let max = snapshot.count(Property::LnaStateMax)?;
    (max > min).then_some((min as f64, max as f64))
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Remote {
    center_hz: Option<u64>,
    sample_rate: Option<f64>,
    lna_state: Option<u64>,
    antenna: Option<String>,
    receiver: Option<String>,
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
            antenna: snapshot
                .text(Property::ActiveAntenna)
                .map(ToString::to_string),
            receiver: snapshot
                .text(Property::ActiveDevice)
                .map(ToString::to_string),
            network_mode: None,
            profile: None,
            recording: None,
            streaming: snapshot.boolean(Property::Started).unwrap_or(false),
        }
    }

    fn selection(&self) -> Option<String> {
        let receiver = self.receiver.clone()?;
        Some(match self.network_mode {
            Some(mode) => format!("{receiver}:{}", mode.name()),
            None => receiver,
        })
    }

    /// Everything the server has to be told again on a connection that replaced a lost one.
    pub(crate) fn replay(&self) -> Vec<Command> {
        let mut batch = Vec::new();
        if let Some(selection) = self.selection() {
            batch.push(Command::Emit(Event::SelectedDeviceName, selection));
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
        if let Some(antenna) = &self.antenna {
            batch.push(Command::Set(Property::ActiveAntenna, antenna.clone()));
        }
        batch
    }

    /// Turns the IQ on, and nothing else the link would otherwise carry.
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
        let mut extra = Vec::new();
        let offered = |name: &str| caps.extra.iter().any(|setting| setting.name() == name);
        if let Some(state) = self.lna_state.filter(|_| offered(LNA_STATE)) {
            extra.push(ExtraValue {
                name: LNA_STATE.to_string(),
                value: state.into(),
            });
        }
        if let Some(receiver) = self.receiver.clone().filter(|_| offered(RECEIVER)) {
            extra.push(ExtraValue {
                name: RECEIVER.to_string(),
                value: receiver.into(),
            });
        }
        if offered(NETWORK_MODE) {
            extra.push(ExtraValue {
                name: NETWORK_MODE.to_string(),
                value: self
                    .network_mode
                    .map_or(AUTOMATIC, NetworkMode::name)
                    .into(),
            });
        }
        extra.push(ExtraValue {
            name: PROFILE.to_string(),
            value: self.profile.clone().unwrap_or_default().into(),
        });
        extra.push(ExtraValue {
            name: RECORDING.to_string(),
            value: self.recording.map_or(OFF, Recording::name).into(),
        });
        DeviceSettings {
            center_hz: self.center_hz.map(|hz| hz as f64),
            sample_rate: self.sample_rate,
            antenna: self.antenna.clone(),
            extra,
            ..DeviceSettings::default()
        }
    }
}

fn within(ranges: &[Range], value: f64) -> bool {
    ranges
        .iter()
        .any(|range| range.min <= value && value <= range.max)
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
    if delta.bandwidth.is_some() {
        return Err(DeviceError::Unsupported(
            "bandwidth: the SDRconnect API's filter width is the VFO's, not the IQ stream's"
                .to_string(),
        ));
    }
    if let Some(gain) = delta.gains.first() {
        return Err(DeviceError::Unsupported(format!(
            "gain stage {}: this receiver's RF gain is a state, offered as the `{LNA_STATE}` setting",
            gain.stage
        )));
    }

    for value in &delta.extra {
        apply_extra(&mut next, &mut batch, caps, value)?;
    }
    Ok((next, batch))
}

fn apply_extra(
    next: &mut Remote,
    batch: &mut Vec<Command>,
    caps: &Capabilities,
    value: &ExtraValue,
) -> Result<(), DeviceError> {
    let setting = caps
        .extra
        .iter()
        .find(|setting| setting.name() == value.name)
        .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
    let refuse = || {
        DeviceError::Unsupported(format!(
            "extra setting {}: {} is not one this receiver takes",
            value.name, value.value
        ))
    };
    match (setting, value.name.as_str()) {
        (ExtraSetting::Range { range, .. }, LNA_STATE) => {
            let state = value
                .value
                .as_f64()
                .filter(|state| state.is_finite() && (range.min..=range.max).contains(state))
                .ok_or_else(refuse)?;
            next.lna_state = Some(state.round() as u64);
            batch.push(Command::Set(Property::LnaState, state.round().to_string()));
        }
        (ExtraSetting::Enum { options, .. }, RECEIVER) => {
            let receiver = value.value.as_str().ok_or_else(refuse)?;
            if !options.iter().any(|option| option.value == receiver) {
                return Err(refuse());
            }
            next.receiver = Some(receiver.to_string());
            if let Some(selection) = next.selection() {
                batch.push(Command::Emit(Event::SelectedDeviceName, selection));
            }
        }
        (ExtraSetting::Enum { .. }, NETWORK_MODE) => {
            let name = value.value.as_str().ok_or_else(refuse)?;
            next.network_mode = if name == AUTOMATIC {
                None
            } else {
                Some(NetworkMode::parse(name).ok_or_else(refuse)?)
            };
            if let Some(selection) = next.selection() {
                batch.push(Command::Emit(Event::SelectedDeviceName, selection));
            }
        }
        (ExtraSetting::String { .. }, PROFILE) => {
            let profile = value.value.as_str().ok_or_else(refuse)?.trim();
            next.profile = (!profile.is_empty()).then(|| profile.to_string());
            if let Some(profile) = &next.profile {
                batch.push(Command::Emit(Event::ApplyDeviceProfile, profile.clone()));
            }
        }
        (ExtraSetting::Enum { .. }, RECORDING) => {
            let name = value.value.as_str().ok_or_else(refuse)?;
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
        _ => {
            return Err(DeviceError::Unsupported(format!(
                "extra setting {}",
                value.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::GainValue;

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

    #[test]
    fn a_steerable_receiver_offers_the_whole_rsp_range_and_its_gain_state() {
        let (caps, _) = opened(true);
        assert_eq!(caps.freq_ranges[0].min, MIN_FREQUENCY_HZ);
        assert_eq!(caps.freq_ranges[0].max, MAX_FREQUENCY_HZ);
        assert_eq!(caps.sample_rate_ranges[0].min, MIN_SAMPLE_RATE);
        assert!(caps.sample_rates.is_empty(), "the API names no rate menu");
        assert_eq!(caps.antennas, vec!["Antenna A", "Antenna B"]);
        let lna = caps
            .extra
            .iter()
            .find(|setting| setting.name() == LNA_STATE)
            .expect("the RF gain state");
        let ExtraSetting::Range { range, unit, .. } = lna else {
            panic!("the gain state is a range, not {lna:?}");
        };
        assert_eq!((range.min, range.max), (0.0, 9.0));
        assert_eq!(unit, "state");
    }

    #[test]
    fn a_receiver_that_will_not_be_steered_reports_only_where_it_already_is() {
        let (caps, _) = opened(false);
        assert_eq!(caps.freq_ranges, pinned(Some(100e6)));
        assert_eq!(caps.sample_rate_ranges, pinned(Some(2e6)));
        assert!(
            !caps.extra.iter().any(|setting| setting.name() == LNA_STATE),
            "a gain the server will refuse is not offered"
        );
    }

    #[test]
    fn the_receiver_list_becomes_the_choice_of_radio_on_the_host() {
        let (caps, remote) = opened(true);
        let receivers = caps
            .extra
            .iter()
            .find(|setting| setting.name() == RECEIVER)
            .expect("the receiver choice");
        let ExtraSetting::Enum {
            options, default, ..
        } = receivers
        else {
            panic!("the receiver choice is an enum, not {receivers:?}");
        };
        assert_eq!(
            options.iter().map(|o| o.value.as_str()).collect::<Vec<_>>(),
            vec!["RSPduo 1234", "RSP1B 5678"]
        );
        assert_eq!(default, "RSP1B 5678", "the one already in use");
        assert_eq!(remote.wire(&caps).antenna.as_deref(), Some("Antenna A"));
    }

    #[test]
    fn a_fresh_device_starts_where_the_server_already_is() {
        let (caps, remote) = opened(true);
        let wire = remote.wire(&caps);
        assert_eq!(wire.center_hz, Some(100e6));
        assert_eq!(wire.sample_rate, Some(2e6));
        let value = |name: &str| {
            wire.extra
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.value.clone())
        };
        assert_eq!(value(LNA_STATE), Some(serde_json::json!(4)));
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
            "audio the engine never reads is not asked for: {names:?}"
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
        assert!(
            refused(DeviceSettings {
                bandwidth: Some(200e3),
                ..DeviceSettings::default()
            })
            .contains("VFO")
        );
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
        assert!(
            refused(DeviceSettings {
                gains: vec![GainValue {
                    stage: "LNA".to_string(),
                    value_db: 20.0,
                }],
                ..DeviceSettings::default()
            })
            .contains(LNA_STATE)
        );
        assert!(refused(extra(LNA_STATE, 99)).contains(LNA_STATE));
        assert!(refused(extra("warp", 1)).contains("warp"));
    }

    #[test]
    fn choosing_a_receiver_and_a_network_mode_names_both_in_one_selection() {
        let (caps, remote) = opened(true);
        let (next, batch) = validate(&extra(RECEIVER, "RSPduo 1234"), &caps, &remote)
            .expect("a receiver on the host");
        assert_eq!(
            batch,
            vec![Command::Emit(
                Event::SelectedDeviceName,
                "RSPduo 1234".to_string()
            )]
        );
        let (with_mode, batch) =
            validate(&extra(NETWORK_MODE, "IQ Lite"), &caps, &next).expect("a mode the API names");
        assert_eq!(
            batch,
            vec![Command::Emit(
                Event::SelectedDeviceName,
                "RSPduo 1234:IQ Lite".to_string()
            )]
        );
        let (back, _) = validate(&extra(NETWORK_MODE, AUTOMATIC), &caps, &with_mode)
            .expect("back to the server's own choice");
        assert_eq!(back.selection().as_deref(), Some("RSPduo 1234"));
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
