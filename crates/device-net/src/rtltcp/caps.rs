use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{
    Capabilities, DeviceSettings, Duplex, ExtraSetting, ExtraValue, GainStage, GainValue, Range,
    StreamScope,
};

use crate::rtltcp::proto::{Command, Tuner, ordered};

pub(crate) const TUNER_STAGE: &str = "TUNER";
pub(crate) const BIAS_TEE: &str = "bias_tee";
pub(crate) const AGC: &str = "agc";
pub(crate) const RTL_AGC: &str = "rtl_agc";

const RATE_MENU: [f64; 9] = [
    250_000.0,
    1_024_000.0,
    1_536_000.0,
    1_920_000.0,
    2_048_000.0,
    2_400_000.0,
    2_560_000.0,
    2_880_000.0,
    3_200_000.0,
];
const RATE_WINDOWS: [(f64, f64); 2] = [(225_001.0, 300_000.0), (900_001.0, 3_200_000.0)];

const PPM_MAX: f64 = 200.0;

const MAX_PARAM_HZ: f64 = u32::MAX as f64;

const E4000_GAINS: [i32; 14] = [
    -10, 15, 40, 65, 90, 115, 140, 165, 190, 215, 240, 290, 340, 420,
];
const FC0012_GAINS: [i32; 5] = [-99, -40, 71, 179, 192];
const FC0013_GAINS: [i32; 23] = [
    -99, -73, -65, -63, -60, -58, -54, 58, 61, 63, 65, 67, 68, 70, 71, 179, 181, 182, 184, 186,
    188, 191, 197,
];
const FC2580_GAINS: [i32; 1] = [0];
const R82XX_GAINS: [i32; 29] = [
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372,
    386, 402, 421, 434, 439, 445, 480, 496,
];

pub(crate) fn gain_table(tuner: Tuner, reported_steps: u32) -> &'static [i32] {
    let table: &'static [i32] = match tuner {
        Tuner::E4000 => &E4000_GAINS,
        Tuner::Fc0012 => &FC0012_GAINS,
        Tuner::Fc0013 => &FC0013_GAINS,
        Tuner::Fc2580 => &FC2580_GAINS,
        Tuner::R820T | Tuner::R828D => &R82XX_GAINS,
        Tuner::Unknown => &[],
    };
    if table.len() == reported_steps as usize {
        table
    } else {
        &[]
    }
}

fn freq_ranges(tuner: Tuner) -> Vec<Range> {
    let bounds: &[(f64, f64)] = match tuner {
        Tuner::E4000 => &[(52e6, 2.2e9)],
        Tuner::Fc0012 => &[(22e6, 948e6)],
        Tuner::Fc0013 => &[(22e6, 1.1e9)],
        Tuner::Fc2580 => &[(146e6, 308e6), (438e6, 924e6)],
        Tuner::R820T | Tuner::R828D => &[(24e6, 1766e6)],
        Tuner::Unknown => &[],
    };
    bounds
        .iter()
        .map(|(min, max)| Range {
            min: *min,
            max: *max,
            step: None,
        })
        .collect()
}

pub(crate) fn capabilities(tuner: Tuner, gains: &[i32]) -> Capabilities {
    let range = match (gains.iter().copied().min(), gains.iter().copied().max()) {
        (Some(min), Some(max)) => Range {
            min: tenths_to_db(min),
            max: tenths_to_db(max),
            step: None,
        },
        _ => Range {
            min: 0.0,
            max: 50.0,
            step: None,
        },
    };
    Capabilities {
        freq_ranges: freq_ranges(tuner),
        sample_rates: RATE_MENU.to_vec(),
        sample_rate_ranges: Vec::new(),
        gains: vec![GainStage {
            name: TUNER_STAGE.to_string(),
            range,
            values: Vec::new(),
        }],
        antennas: vec!["RX".to_string()],
        bandwidths: Vec::new(),
        bandwidth_ranges: Vec::new(),
        extra: extra_settings(),
        ppm: true,
        duplex: Duplex::RxOnly,
        rx_streams: 1,
        tx_streams: 0,
        per_stream: StreamScope::default(),
        directional: None,
    }
}

fn extra_settings() -> Vec<ExtraSetting> {
    vec![
        ExtraSetting::Bool {
            name: BIAS_TEE.to_string(),
            default: false,
        },
        ExtraSetting::Bool {
            name: AGC.to_string(),
            default: true,
        },
        ExtraSetting::Bool {
            name: RTL_AGC.to_string(),
            default: false,
        },
    ]
}

fn tenths_to_db(tenths: i32) -> f64 {
    f64::from(tenths) / 10.0
}

fn nearest_gain(table: &[i32], tenths: i32) -> Option<i32> {
    table
        .iter()
        .copied()
        .min_by_key(|g| ((i64::from(*g) - i64::from(tenths)).abs(), i64::from(*g)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GainMode {
    Auto,
    Manual(i32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Remote {
    sample_rate: u32,
    center_hz: u32,
    ppm: i32,
    gain: GainMode,
    manual_tenths: i32,
    bias_tee: bool,
    rtl_agc: bool,
}

const DEFAULT_SAMPLE_RATE_HZ: u32 = 2_048_000;
const DEFAULT_CENTER_HZ: u32 = 100_000_000;

impl Remote {
    pub(crate) fn new(table: &[i32]) -> Self {
        Self {
            sample_rate: DEFAULT_SAMPLE_RATE_HZ,
            center_hz: DEFAULT_CENTER_HZ,
            ppm: 0,
            gain: GainMode::Auto,
            manual_tenths: table.iter().copied().max().unwrap_or(0),
            bias_tee: false,
            rtl_agc: false,
        }
    }

    pub(crate) fn replay(&self) -> Vec<(Command, u32)> {
        let mut batch = vec![
            (Command::SampleRate, self.sample_rate),
            (Command::CenterFreq, self.center_hz),
            (Command::FreqCorrection, self.ppm as u32),
            (Command::RtlAgc, u32::from(self.rtl_agc)),
            (Command::BiasTee, u32::from(self.bias_tee)),
        ];
        batch.extend(gain_commands(self.gain));
        ordered(batch)
    }

    pub(crate) fn wire(&self) -> DeviceSettings {
        let mut settings = DeviceSettings {
            center_hz: Some(f64::from(self.center_hz)),
            sample_rate: Some(f64::from(self.sample_rate)),
            ppm: Some(f64::from(self.ppm)),
            antenna: Some("RX".to_string()),
            extra: vec![
                ExtraValue {
                    name: BIAS_TEE.to_string(),
                    value: self.bias_tee.into(),
                },
                ExtraValue {
                    name: AGC.to_string(),
                    value: matches!(self.gain, GainMode::Auto).into(),
                },
                ExtraValue {
                    name: RTL_AGC.to_string(),
                    value: self.rtl_agc.into(),
                },
            ],
            ..DeviceSettings::default()
        };
        if let GainMode::Manual(tenths) = self.gain {
            settings.gains.push(GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: tenths_to_db(tenths),
            });
        }
        settings
    }
}

fn gain_commands(gain: GainMode) -> Vec<(Command, u32)> {
    match gain {
        GainMode::Auto => vec![(Command::GainMode, 0)],
        GainMode::Manual(tenths) => {
            vec![(Command::GainMode, 1), (Command::Gain, tenths as u32)]
        }
    }
}

pub(crate) fn validate(
    delta: &DeviceSettings,
    caps: &Capabilities,
    current: Remote,
    table: &[i32],
) -> Result<(Remote, Vec<(Command, u32)>), DeviceError> {
    check_stream_settings(delta, caps)?;
    let mut next = current;
    let mut batch: Vec<(Command, u32)> = Vec::new();

    if let Some(rate) = delta.sample_rate {
        if !RATE_WINDOWS
            .iter()
            .any(|(lo, hi)| *lo <= rate && rate <= *hi)
        {
            return Err(DeviceError::Unsupported(format!(
                "sample_rate {rate} outside the RTL2832U windows \
                 225001-300000 and 900001-3200000 Hz"
            )));
        }
        next.sample_rate = rate.round() as u32;
        batch.push((Command::SampleRate, next.sample_rate));
    }

    if let Some(hz) = delta.center_hz {
        let reachable = caps.freq_ranges.is_empty()
            || caps.freq_ranges.iter().any(|r| r.min <= hz && hz <= r.max);
        if !reachable {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside tuner range"
            )));
        }
        if !hz.is_finite() || !(0.0..=MAX_PARAM_HZ).contains(&hz) {
            return Err(DeviceError::Unsupported(format!(
                "center_hz {hz} outside 0..{MAX_PARAM_HZ} Hz, which is all rtl_tcp can carry"
            )));
        }
        next.center_hz = hz.round() as u32;
        batch.push((Command::CenterFreq, next.center_hz));
    }

    if let Some(ppm) = delta.ppm {
        if !ppm.is_finite() || !(-PPM_MAX..=PPM_MAX).contains(&ppm) {
            return Err(DeviceError::Unsupported(format!(
                "ppm {ppm} outside ±{PPM_MAX}"
            )));
        }
        next.ppm = ppm.round() as i32;
        batch.push((Command::FreqCorrection, next.ppm as u32));
    }

    if let Some(antenna) = &delta.antenna
        && !caps.antennas.contains(antenna)
    {
        return Err(DeviceError::Unsupported(format!("antenna {antenna}")));
    }

    if delta.bandwidth.is_some() {
        return Err(DeviceError::Unsupported(
            "bandwidth: rtl_tcp has no filter-width command".to_string(),
        ));
    }

    let mut requested_gain = None;
    for gain in &delta.gains {
        let stage = caps
            .gains
            .iter()
            .find(|s| s.name == gain.stage)
            .ok_or_else(|| DeviceError::Unsupported(format!("gain stage {}", gain.stage)))?;
        if !gain.value_db.is_finite()
            || !(stage.range.min..=stage.range.max).contains(&gain.value_db)
        {
            return Err(DeviceError::Unsupported(format!(
                "gain {} {} dB outside {}..{} dB",
                gain.stage, gain.value_db, stage.range.min, stage.range.max
            )));
        }
        let tenths = (gain.value_db * 10.0).round() as i32;
        requested_gain = Some(nearest_gain(table, tenths).unwrap_or(tenths));
    }

    let mut agc = None;
    for value in &delta.extra {
        let setting = caps
            .extra
            .iter()
            .find(|setting| setting.name() == value.name)
            .ok_or_else(|| DeviceError::Unsupported(format!("extra setting {}", value.name)))?;
        let on = extra_bool(setting, value)?;
        match value.name.as_str() {
            BIAS_TEE => {
                next.bias_tee = on;
                batch.push((Command::BiasTee, u32::from(on)));
            }
            RTL_AGC => {
                next.rtl_agc = on;
                batch.push((Command::RtlAgc, u32::from(on)));
            }
            AGC => agc = Some(on),
            other => return Err(DeviceError::Unsupported(format!("extra setting {other}"))),
        }
    }

    let gain = match (agc, requested_gain) {
        (Some(true), _) => Some(GainMode::Auto),
        (_, Some(tenths)) => Some(GainMode::Manual(tenths)),
        (Some(false), None) => Some(GainMode::Manual(next.manual_tenths)),
        (None, None) => None,
    };
    if let Some(gain) = gain {
        next.gain = gain;
        if let GainMode::Manual(tenths) = gain {
            next.manual_tenths = tenths;
        }
        batch.extend(gain_commands(gain));
    }

    Ok((next, ordered(batch)))
}

fn extra_bool(setting: &ExtraSetting, value: &ExtraValue) -> Result<bool, DeviceError> {
    match setting {
        ExtraSetting::Bool { .. } => value.value.as_bool().ok_or_else(|| {
            DeviceError::Unsupported(format!(
                "extra setting {}: bad value {}",
                value.name, value.value
            ))
        }),
        _ => Err(DeviceError::Unsupported(format!(
            "extra setting {}: not a boolean",
            value.name
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r820t() -> (Capabilities, Remote, &'static [i32]) {
        let table = gain_table(Tuner::R820T, 29);
        (capabilities(Tuner::R820T, table), Remote::new(table), table)
    }

    fn delta(settings: DeviceSettings) -> DeviceSettings {
        settings
    }

    fn apply(settings: DeviceSettings) -> Result<(Remote, Vec<(Command, u32)>), DeviceError> {
        let (caps, current, table) = r820t();
        validate(&settings, &caps, current, table)
    }

    #[test]
    fn a_tuner_the_greeting_names_brings_its_table_and_range() {
        let table = gain_table(Tuner::R820T, 29);
        assert_eq!(table.len(), 29);
        let caps = capabilities(Tuner::R820T, table);
        assert_eq!(caps.gains[0].range.min, 0.0);
        assert_eq!(caps.gains[0].range.max, 49.6);
        assert_eq!(caps.freq_ranges.len(), 1);
        assert_eq!(caps.freq_ranges[0].min, 24e6);
    }

    #[test]
    fn a_step_count_that_disagrees_drops_the_table_and_widens_the_stage() {
        assert!(gain_table(Tuner::R820T, 28).is_empty());
        assert!(gain_table(Tuner::Unknown, 29).is_empty());
        let caps = capabilities(Tuner::R820T, gain_table(Tuner::R820T, 28));
        assert_eq!(caps.gains[0].range.max, 50.0);
        let (next, batch) = validate(
            &DeviceSettings {
                gains: vec![GainValue {
                    stage: TUNER_STAGE.to_string(),
                    value_db: 31.7,
                }],
                ..DeviceSettings::default()
            },
            &caps,
            Remote::new(&[]),
            &[],
        )
        .expect("accepted");
        assert_eq!(next.gain, GainMode::Manual(317));
        assert_eq!(batch, vec![(Command::GainMode, 1), (Command::Gain, 317)]);
    }

    #[test]
    fn an_unknown_tuner_advertises_no_range_and_is_held_only_to_the_wire_format() {
        let caps = capabilities(Tuner::Unknown, &[]);
        assert!(caps.freq_ranges.is_empty());
        let accepted = validate(
            &DeviceSettings {
                center_hz: Some(3.5e6),
                ..DeviceSettings::default()
            },
            &caps,
            Remote::new(&[]),
            &[],
        );
        assert!(accepted.is_ok(), "{accepted:?}");
        let refused = validate(
            &DeviceSettings {
                center_hz: Some(6e9),
                ..DeviceSettings::default()
            },
            &caps,
            Remote::new(&[]),
            &[],
        );
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn a_gain_request_snaps_to_the_tuner_table_and_is_reported_snapped() {
        let (next, batch) = apply(delta(DeviceSettings {
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 24.0,
            }],
            ..DeviceSettings::default()
        }))
        .expect("accepted");
        assert_eq!(next.gain, GainMode::Manual(229));
        assert_eq!(batch, vec![(Command::GainMode, 1), (Command::Gain, 229)]);
        assert_eq!(next.wire().gains[0].value_db, 22.9);

        let (tie, _) = apply(delta(DeviceSettings {
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 36.8,
            }],
            ..DeviceSettings::default()
        }))
        .expect("accepted");
        assert_eq!(tie.gain, GainMode::Manual(364));
    }

    #[test]
    fn a_rate_and_gain_change_together_arrive_in_the_radios_order() {
        let (_, batch) = apply(delta(DeviceSettings {
            sample_rate: Some(2_400_000.0),
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 0.0,
            }],
            center_hz: Some(433_920_000.0),
            ..DeviceSettings::default()
        }))
        .expect("accepted");
        assert_eq!(
            batch,
            vec![
                (Command::SampleRate, 2_400_000),
                (Command::CenterFreq, 433_920_000),
                (Command::GainMode, 1),
                (Command::Gain, 0),
            ]
        );
    }

    #[test]
    fn leaving_agc_restores_the_last_manual_gain() {
        let (manual, _) = apply(delta(DeviceSettings {
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 16.6,
            }],
            ..DeviceSettings::default()
        }))
        .expect("manual");
        let (caps, _, table) = r820t();
        let auto = DeviceSettings {
            extra: vec![ExtraValue {
                name: AGC.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        };
        let (agc_on, batch) = validate(&auto, &caps, manual, table).expect("agc on");
        assert_eq!(agc_on.gain, GainMode::Auto);
        assert_eq!(batch, vec![(Command::GainMode, 0)]);
        assert!(agc_on.wire().gains.is_empty(), "auto reports no live value");

        let off = DeviceSettings {
            extra: vec![ExtraValue {
                name: AGC.to_string(),
                value: false.into(),
            }],
            ..DeviceSettings::default()
        };
        let (agc_off, batch) = validate(&off, &caps, agc_on, table).expect("agc off");
        assert_eq!(agc_off.gain, GainMode::Manual(166));
        assert_eq!(batch, vec![(Command::GainMode, 1), (Command::Gain, 166)]);
    }

    #[test]
    fn a_setting_the_protocol_cannot_carry_is_refused_by_name() {
        for (settings, needle) in [
            (
                DeviceSettings {
                    sample_rate: Some(500_000.0),
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
                    ppm: Some(500.0),
                    ..DeviceSettings::default()
                },
                "ppm",
            ),
            (
                DeviceSettings {
                    bandwidth: Some(1.5e6),
                    ..DeviceSettings::default()
                },
                "bandwidth",
            ),
            (
                DeviceSettings {
                    antenna: Some("TX".to_string()),
                    ..DeviceSettings::default()
                },
                "antenna",
            ),
            (
                DeviceSettings {
                    gains: vec![GainValue {
                        stage: "LNA".to_string(),
                        value_db: 8.0,
                    }],
                    ..DeviceSettings::default()
                },
                "gain stage",
            ),
            (
                DeviceSettings {
                    extra: vec![ExtraValue {
                        name: "offset_tuning".to_string(),
                        value: true.into(),
                    }],
                    ..DeviceSettings::default()
                },
                "extra setting",
            ),
            (
                DeviceSettings {
                    extra: vec![ExtraValue {
                        name: BIAS_TEE.to_string(),
                        value: 1.into(),
                    }],
                    ..DeviceSettings::default()
                },
                "bad value",
            ),
        ] {
            match apply(settings) {
                Err(DeviceError::Unsupported(message)) => {
                    assert!(message.contains(needle), "{message} lacks {needle}");
                }
                other => panic!("must be refused naming {needle}, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_replay_carries_every_setting_in_order() {
        let (next, _) = apply(delta(DeviceSettings {
            center_hz: Some(433_920_000.0),
            sample_rate: Some(2_400_000.0),
            ppm: Some(-12.0),
            gains: vec![GainValue {
                stage: TUNER_STAGE.to_string(),
                value_db: 25.4,
            }],
            extra: vec![ExtraValue {
                name: BIAS_TEE.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        }))
        .expect("accepted");
        assert_eq!(
            next.replay(),
            vec![
                (Command::SampleRate, 2_400_000),
                (Command::CenterFreq, 433_920_000),
                (Command::FreqCorrection, -12i32 as u32),
                (Command::GainMode, 1),
                (Command::Gain, 254),
                (Command::RtlAgc, 0),
                (Command::BiasTee, 1),
            ]
        );
    }

    #[test]
    fn the_reported_settings_are_what_was_asked_for() {
        let (next, _) = apply(delta(DeviceSettings {
            center_hz: Some(433_920_000.0),
            ppm: Some(2.4),
            extra: vec![ExtraValue {
                name: RTL_AGC.to_string(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        }))
        .expect("accepted");
        let wire = next.wire();
        assert_eq!(wire.center_hz, Some(433_920_000.0));
        assert_eq!(wire.ppm, Some(2.0), "a fractional ppm is reported rounded");
        assert_eq!(wire.sample_rate, Some(f64::from(DEFAULT_SAMPLE_RATE_HZ)));
        let rtl_agc = wire
            .extra
            .iter()
            .find(|e| e.name == RTL_AGC)
            .expect("reported");
        assert_eq!(rtl_agc.value, true);
    }
}
