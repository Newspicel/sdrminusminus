use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    ArgumentOption, Capabilities, DcArtifact, DeviceSettings, Duplex, ExtraSetting, ExtraValue,
    Range, StreamScope,
};

use crate::spyserver::proto::{ClientSync, DeviceInfo, IqFormat, Setting, ordered};

pub(crate) const GAIN: &str = "gain";
pub(crate) const IQ_FORMAT: &str = "iq_format";

const MAX_DECIMATION_STAGES: u32 = 16;

const DIGITAL_GAIN_PER_STAGE_TENTHS: u32 = 30;

fn sample_rates(info: DeviceInfo) -> Vec<f64> {
    let stages = info.decimation_stages.min(MAX_DECIMATION_STAGES);
    if info.max_sample_rate == 0 || info.min_decimation > stages {
        return Vec::new();
    }
    (info.min_decimation..=stages)
        .rev()
        .map(|stage| f64::from(info.max_sample_rate >> stage))
        .collect()
}

fn decimation_for(info: DeviceInfo, rate: f64) -> Option<u32> {
    let stages = info.decimation_stages.min(MAX_DECIMATION_STAGES);
    (info.min_decimation..=stages)
        .find(|stage| (f64::from(info.max_sample_rate >> stage) - rate).abs() < 0.5)
}

pub(crate) fn capabilities(
    info: DeviceInfo,
    sync: ClientSync,
    formats: &[IqFormat],
) -> Capabilities {
    let (min, max) = if sync.can_control {
        (info.min_frequency, info.max_frequency)
    } else {
        (sync.min_iq_center_hz, sync.max_iq_center_hz)
    };
    let freq_ranges = if min < max {
        vec![Range {
            min: f64::from(min),
            max: f64::from(max),
            step: None,
        }]
    } else {
        Vec::new()
    };

    let mut extra = Vec::with_capacity(2);
    if sync.can_control && info.max_gain_index > 0 {
        extra.push(ExtraSetting::Range {
            name: GAIN.to_string(),
            range: Range {
                min: 0.0,
                max: f64::from(info.max_gain_index),
                step: Some(1.0),
            },
            unit: "index".to_string(),
        });
    }
    if formats.len() > 1 {
        extra.push(ExtraSetting::Enum {
            name: IQ_FORMAT.to_string(),
            options: formats
                .iter()
                .map(|f| ArgumentOption::plain(f.name()))
                .collect(),
            default: IqFormat::default().name().to_string(),
        });
    }

    Capabilities {
        freq_ranges,
        sample_rates: sample_rates(info),
        sample_rate_ranges: Vec::new(),
        gains: Vec::new(),
        antennas: Vec::new(),
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
        coherence: sdrmm_wire::Coherence::None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Remote {
    decimation: u32,
    center_hz: u32,
    gain: u32,
    format: IqFormat,
}

impl Remote {
    pub(crate) fn new(info: DeviceInfo, sync: ClientSync, format: IqFormat) -> Self {
        Self {
            decimation: info
                .min_decimation
                .min(info.decimation_stages.min(MAX_DECIMATION_STAGES)),
            center_hz: sync.iq_center_hz,
            gain: sync.gain.min(info.max_gain_index),
            format,
        }
    }

    fn sample_rate(self, info: DeviceInfo) -> f64 {
        f64::from(info.max_sample_rate >> self.decimation.min(MAX_DECIMATION_STAGES))
    }

    fn digital_gain(self, info: DeviceInfo) -> u32 {
        if self.format == IqFormat::Float32 {
            return 0;
        }
        let per_stage = (self.decimation * DIGITAL_GAIN_PER_STAGE_TENTHS + 5) / 10;
        if info.airspy_one() {
            per_stage + info.max_gain_index.saturating_sub(self.gain)
        } else {
            per_stage
        }
    }

    pub(crate) fn replay(self, info: DeviceInfo) -> Vec<(Setting, u32)> {
        ordered(vec![
            (Setting::IqFormat, self.format.code()),
            (Setting::IqDecimation, self.decimation),
            (Setting::IqFrequency, self.center_hz),
            crate::spyserver::proto::iq_only(),
            (Setting::Gain, self.gain),
            (Setting::IqDigitalGain, self.digital_gain(info)),
            (Setting::StreamingEnabled, 1),
        ])
    }

    pub(crate) fn wire(self, info: DeviceInfo, caps: &Capabilities) -> DeviceSettings {
        let mut extra = Vec::with_capacity(2);
        if caps.extra.iter().any(|setting| setting.name() == GAIN) {
            extra.push(ExtraValue {
                name: GAIN.to_string(),
                value: self.gain.into(),
            });
        }
        if caps.extra.iter().any(|setting| setting.name() == IQ_FORMAT) {
            extra.push(ExtraValue {
                name: IQ_FORMAT.to_string(),
                value: self.format.name().into(),
            });
        }
        DeviceSettings {
            center_hz: Some(f64::from(self.center_hz)),
            sample_rate: Some(self.sample_rate(info)),
            extra,
            ..DeviceSettings::default()
        }
    }
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    info: DeviceInfo,
    current: Remote,
) -> Result<(Remote, Vec<(Setting, u32)>), DeviceError> {
    check_stream_settings(delta, caps)?;
    let mut next = current;
    let mut batch: Vec<(Setting, u32)> = Vec::new();
    let mut rescale = false;

    if let Some(rate) = delta.sample_rate {
        next.decimation = decimation_for(info, rate).ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "sample_rate {rate}: this server offers {:?}",
                caps.sample_rates
            ))
        })?;
        batch.push((Setting::IqDecimation, next.decimation));
        rescale = true;
    }

    if let Some(hz) = delta.center_hz {
        let reachable = caps.freq_ranges.iter().any(|r| r.min <= hz && hz <= r.max);
        if !reachable {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside what this server will tune to"
            )));
        }
        next.center_hz = hz.round() as u32;
        batch.push((Setting::IqFrequency, next.center_hz));
    }

    if delta.ppm.is_some_and(|ppm| ppm != 0.0) {
        return Err(DeviceError::Unsupported(
            "ppm: the SpyServer protocol has no frequency correction; correct it on the server"
                .to_string(),
        ));
    }
    if delta.bandwidth.is_some() {
        return Err(DeviceError::Unsupported(
            "bandwidth: the SpyServer protocol has no filter-width setting".to_string(),
        ));
    }
    if let Some(antenna) = &delta.antenna {
        return Err(DeviceError::Unsupported(format!(
            "antenna {antenna}: the SpyServer protocol has no antenna selection"
        )));
    }
    if let Some(gain) = delta.gains.first() {
        return Err(DeviceError::Unsupported(format!(
            "gain stage {}: this server's gain is an index, offered as the `{GAIN}` setting",
            gain.stage
        )));
    }

    for value in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|setting| setting.name() == value.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
        match setting {
            ExtraSetting::Range { range, .. } => {
                let index = value
                    .value
                    .as_f64()
                    .filter(|v| v.is_finite() && (range.min..=range.max).contains(v))
                    .ok_or_else(|| {
                        DeviceError::Unsupported(format!(
                            "extra setting {}: {} is not an index in {}..{}",
                            value.name, value.value, range.min, range.max
                        ))
                    })?;
                next.gain = index.round() as u32;
                batch.push((Setting::Gain, next.gain));
                rescale = true;
            }
            ExtraSetting::Enum { .. } => {
                let format = value
                    .value
                    .as_str()
                    .and_then(IqFormat::from_name)
                    .ok_or_else(|| {
                        DeviceError::Unsupported(format!(
                            "extra setting {}: {} is not a format this server offers",
                            value.name, value.value
                        ))
                    })?;
                next.format = format;
                batch.push((Setting::IqFormat, format.code()));
                rescale = true;
            }
            ExtraSetting::Bool { name, .. } | ExtraSetting::String { name, .. } => {
                return Err(DeviceError::Unsupported(format!("extra setting {name}")));
            }
        }
    }

    if rescale {
        batch.push((Setting::IqDigitalGain, next.digital_gain(info)));
    }
    Ok((next, ordered(batch)))
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::GainValue;

    use super::*;

    fn info() -> DeviceInfo {
        DeviceInfo {
            device_type: 3,
            serial: 0x00C0_FFEE,
            max_sample_rate: 8_000_000,
            decimation_stages: 4,
            max_gain_index: 28,
            min_frequency: 24_000_000,
            max_frequency: 1_766_000_000,
            min_decimation: 1,
            forced_iq_format: 0,
        }
    }

    fn sync(can_control: bool) -> ClientSync {
        ClientSync {
            can_control,
            gain: 12,
            iq_center_hz: 100_000_000,
            min_iq_center_hz: 99_000_000,
            max_iq_center_hz: 101_000_000,
        }
    }

    fn formats() -> Vec<IqFormat> {
        vec![IqFormat::Uint8, IqFormat::Int16, IqFormat::Float32]
    }

    fn open(can_control: bool) -> (Capabilities, Remote) {
        let caps = capabilities(info(), sync(can_control), &formats());
        let remote = Remote::new(info(), sync(can_control), IqFormat::Int16);
        (caps, remote)
    }

    #[test]
    fn the_rate_menu_is_the_maximum_halved_once_per_stage_it_allows() {
        let caps = capabilities(info(), sync(true), &formats());
        assert_eq!(
            caps.sample_rates,
            vec![500_000.0, 1_000_000.0, 2_000_000.0, 4_000_000.0]
        );
    }

    #[test]
    fn a_server_that_offers_nothing_sane_offers_no_rates_rather_than_zero() {
        let broken = DeviceInfo {
            max_sample_rate: 0,
            ..info()
        };
        assert!(
            capabilities(broken, sync(true), &formats())
                .sample_rates
                .is_empty()
        );
        let inverted = DeviceInfo {
            min_decimation: 9,
            decimation_stages: 2,
            ..info()
        };
        assert!(
            capabilities(inverted, sync(true), &formats())
                .sample_rates
                .is_empty()
        );
    }

    #[test]
    fn a_locked_server_reports_the_window_it_will_move_in() {
        let open = capabilities(info(), sync(true), &formats());
        assert_eq!(open.freq_ranges[0].min, 24e6);
        assert_eq!(open.freq_ranges[0].max, 1.766e9);
        assert!(open.extra.iter().any(|setting| setting.name() == GAIN));

        let locked = capabilities(info(), sync(false), &formats());
        assert_eq!(locked.freq_ranges[0].min, 99e6);
        assert_eq!(locked.freq_ranges[0].max, 101e6);
        assert!(
            !locked.extra.iter().any(|setting| setting.name() == GAIN),
            "a gain control that the server will refuse is not offered"
        );
    }

    #[test]
    fn a_fresh_device_starts_where_the_server_already_is() {
        let (caps, remote) = open(true);
        let wire = remote.wire(info(), &caps);
        assert_eq!(wire.center_hz, Some(100_000_000.0));
        assert_eq!(wire.sample_rate, Some(4_000_000.0), "the lowest decimation");
        let gain = wire.extra.iter().find(|e| e.name == GAIN).expect("offered");
        assert_eq!(gain.value, 12);
    }

    #[test]
    fn a_rate_becomes_the_decimation_stage_that_produces_it() {
        let (caps, remote) = open(true);
        let (next, batch) = validate(
            &DeviceSettings {
                sample_rate: Some(1_000_000.0),
                ..DeviceSettings::default()
            },
            &caps,
            info(),
            remote,
        )
        .expect("on the menu");
        assert_eq!(next.wire(info(), &caps).sample_rate, Some(1_000_000.0));
        assert_eq!(
            batch,
            vec![(Setting::IqDecimation, 3), (Setting::IqDigitalGain, 9)],
            "a decimation change is also a rescale"
        );
    }

    #[test]
    fn float_asks_for_no_digital_gain_and_the_quantised_formats_do() {
        let (caps, remote) = open(true);
        let to_format = |name: &str| DeviceSettings {
            extra: vec![ExtraValue {
                name: IQ_FORMAT.to_string(),
                value: name.into(),
            }],
            ..DeviceSettings::default()
        };
        let (_, batch) = validate(&to_format("float32"), &caps, info(), remote).expect("float");
        assert!(batch.contains(&(Setting::IqDigitalGain, 0)));
        let (_, batch) = validate(&to_format("uint8"), &caps, info(), remote).expect("uint8");
        assert!(batch.contains(&(Setting::IqDigitalGain, 3)), "{batch:?}");
    }

    #[test]
    fn an_airspy_folds_its_tuner_gain_into_the_digital_gain() {
        let airspy = DeviceInfo {
            device_type: 1,
            ..info()
        };
        let caps = capabilities(airspy, sync(true), &formats());
        let remote = Remote::new(airspy, sync(true), IqFormat::Int16);
        let (_, batch) = validate(
            &DeviceSettings {
                extra: vec![ExtraValue {
                    name: GAIN.to_string(),
                    value: 20.into(),
                }],
                ..DeviceSettings::default()
            },
            &caps,
            airspy,
            remote,
        )
        .expect("a gain index");
        assert!(batch.contains(&(Setting::IqDigitalGain, 11)), "{batch:?}");
    }

    #[test]
    fn a_setting_the_protocol_lacks_is_refused_by_name() {
        let (caps, remote) = open(true);
        for (delta, needle) in [
            (
                DeviceSettings {
                    ppm: Some(3.0),
                    ..DeviceSettings::default()
                },
                "ppm",
            ),
            (
                DeviceSettings {
                    bandwidth: Some(1e6),
                    ..DeviceSettings::default()
                },
                "bandwidth",
            ),
            (
                DeviceSettings {
                    antenna: Some("RX".to_string()),
                    ..DeviceSettings::default()
                },
                "antenna",
            ),
            (
                DeviceSettings {
                    gains: vec![GainValue {
                        stage: "TUNER".to_string(),
                        value_db: 20.0,
                    }],
                    ..DeviceSettings::default()
                },
                "index",
            ),
            (
                DeviceSettings {
                    sample_rate: Some(3_000_000.0),
                    ..DeviceSettings::default()
                },
                "sample_rate",
            ),
            (
                DeviceSettings {
                    center_hz: Some(2.4e9),
                    ..DeviceSettings::default()
                },
                "center_hz",
            ),
            (
                DeviceSettings {
                    extra: vec![ExtraValue {
                        name: GAIN.to_string(),
                        value: 99.into(),
                    }],
                    ..DeviceSettings::default()
                },
                "index in 0..28",
            ),
            (
                DeviceSettings {
                    extra: vec![ExtraValue {
                        name: IQ_FORMAT.to_string(),
                        value: "int24".into(),
                    }],
                    ..DeviceSettings::default()
                },
                "not a format",
            ),
            (
                DeviceSettings {
                    extra: vec![ExtraValue {
                        name: "bias_tee".to_string(),
                        value: true.into(),
                    }],
                    ..DeviceSettings::default()
                },
                "extra setting",
            ),
        ] {
            match validate(&delta, &caps, info(), remote) {
                Err(DeviceError::Unsupported(message)) => {
                    assert!(message.contains(needle), "{message} lacks {needle}");
                }
                other => panic!("must be refused naming {needle}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_zero_correction_is_not_a_change_to_refuse() {
        let (caps, remote) = open(true);
        assert!(
            validate(
                &DeviceSettings {
                    ppm: Some(0.0),
                    ..DeviceSettings::default()
                },
                &caps,
                info(),
                remote
            )
            .is_ok()
        );
    }

    #[test]
    fn a_replay_configures_the_stream_before_enabling_it() {
        let (caps, remote) = open(true);
        let (next, _) = validate(
            &DeviceSettings {
                center_hz: Some(433_920_000.0),
                sample_rate: Some(2_000_000.0),
                ..DeviceSettings::default()
            },
            &caps,
            info(),
            remote,
        )
        .expect("accepted");
        assert_eq!(
            next.replay(info()),
            vec![
                (Setting::IqFormat, IqFormat::Int16.code()),
                (Setting::IqDecimation, 2),
                (Setting::IqFrequency, 433_920_000),
                crate::spyserver::proto::iq_only(),
                (Setting::Gain, 12),
                (Setting::IqDigitalGain, 6),
                (Setting::StreamingEnabled, 1),
            ]
        );
    }
}
