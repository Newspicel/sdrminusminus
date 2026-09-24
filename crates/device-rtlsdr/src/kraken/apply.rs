use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{Capabilities, DeviceSettings};

use crate::caps;

/// Pin 0 of the control dongle switches the calibration noise source into every lane, and pins 1
/// and up switch the lanes' bias tees. Every one of them hangs off that one dongle rather than
/// off the lane it feeds.
pub(crate) const NOISE_SOURCE_PIN: u8 = 0;

const CALIBRATION_GAINS: [(f64, usize); 5] = [
    (900e6, 0),
    (1_000e6, 4),
    (1_090e6, 6),
    (1_300e6, 6),
    (1_700e6, 7),
];

pub(crate) fn calibration_gain(center_hz: f64, table: &[i32]) -> Option<i32> {
    let (_, index) = CALIBRATION_GAINS
        .iter()
        .min_by(|a, b| (a.0 - center_hz).abs().total_cmp(&(b.0 - center_hz).abs()))?;
    table.get(*index).or_else(|| table.last()).copied()
}

pub(crate) const fn bias_tee_pin(lane: usize) -> u8 {
    lane as u8 + 1
}

pub(crate) struct Plan {
    pub(crate) lanes: Vec<caps::Plan>,
    pub(crate) gpio: Vec<(u8, bool)>,
    pub(crate) bias_tee: Option<bool>,
}

/// Works out what every lane and the bank's own switches have to be set to.
///
/// Nothing is written here, so a request that one lane cannot meet is refused before any of them
/// has been touched: half an array retuned is not a state worth reaching.
pub(crate) fn plan(
    delta: &DeviceSettings,
    capabilities: &Capabilities,
    lane_caps: &Capabilities,
    current: &[DeviceSettings],
    table: &[i32],
) -> Result<Plan, DeviceError> {
    check_stream_settings(delta, capabilities)?;
    let mut plan = Plan {
        lanes: Vec::with_capacity(current.len()),
        gpio: Vec::new(),
        bias_tee: delta.bias_tee,
    };
    if let Some(value) = delta.extra.first() {
        return Err(DeviceError::Unsupported(format!(
            "extra setting {}",
            value.name
        )));
    }
    if let Some(on) = delta.bias_tee {
        plan.gpio
            .extend((0..current.len()).map(|lane| (bias_tee_pin(lane), on)));
    }
    for (lane, settled) in current.iter().enumerate() {
        let mut want = delta.for_stream(lane as u32, &capabilities.per_stream);
        want.bias_tee = None;
        plan.lanes
            .push(caps::validate(&want, lane_caps, settled, table)?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AgcSetting, ExtraValue, GainKind, GainValue, StreamSettings};

    use super::*;
    use crate::{caps::GainMode, driver::GAIN_VALUES};

    const LANES: usize = 5;

    fn fixture() -> (Capabilities, Capabilities, Vec<DeviceSettings>) {
        let capabilities = caps::kraken_capabilities(LANES as u32, GAIN_VALUES);
        let lane_caps = caps::kraken_lane_capabilities(GAIN_VALUES);
        let settled = vec![
            DeviceSettings {
                center_hz: Some(100e6),
                sample_rate: Some(2.4e6),
                agc: Some(AgcSetting::switched(true)),
                ..DeviceSettings::default()
            };
            LANES
        ];
        (capabilities, lane_caps, settled)
    }

    fn planned(delta: &DeviceSettings) -> Result<Plan, DeviceError> {
        let (capabilities, lane_caps, settled) = fixture();
        plan(delta, &capabilities, &lane_caps, &settled, GAIN_VALUES)
    }

    #[test]
    fn one_tuning_reaches_every_lane() {
        let plan = planned(&DeviceSettings {
            center_hz: Some(433.92e6),
            sample_rate: Some(2.4e6),
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(plan.lanes.len(), LANES);
        for lane in &plan.lanes {
            assert_eq!(lane.center_hz, Some(433_920_000));
            assert_eq!(lane.sample_rate, Some(2_400_000));
        }
    }

    #[test]
    fn a_lanes_gain_stays_on_that_lane() {
        let plan = planned(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 2,
                gains: vec![GainValue::new(GainKind::Tuner, 30.0)],
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert!(matches!(plan.lanes[2].gain, Some(GainMode::Manual(_))));
        for (lane, planned) in plan.lanes.iter().enumerate() {
            if lane != 2 {
                assert_eq!(planned.gain, None, "lane {lane} was not asked for a gain");
            }
        }
    }

    #[test]
    fn the_noise_source_is_taken_in_quietly_low_and_loudly_high() {
        assert_eq!(calibration_gain(98e6, GAIN_VALUES), Some(0));
        assert_eq!(calibration_gain(868e6, GAIN_VALUES), Some(0));
        assert_eq!(calibration_gain(1_090e6, GAIN_VALUES), Some(87));
        assert_eq!(calibration_gain(1_700e6, GAIN_VALUES), Some(125));
    }

    #[test]
    fn a_short_gain_table_still_gives_a_calibration_gain() {
        assert_eq!(calibration_gain(1_700e6, &[0, 100]), Some(100));
        assert_eq!(calibration_gain(1_090e6, &[]), None);
    }

    #[test]
    fn every_lanes_bias_tee_is_a_pin_on_the_control_dongle() {
        let plan = planned(&DeviceSettings {
            bias_tee: Some(true),
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(
            plan.gpio,
            [(1, true), (2, true), (3, true), (4, true), (5, true)]
        );
        assert_eq!(plan.bias_tee, Some(true));
        for lane in &plan.lanes {
            assert_eq!(lane.bias_tee, None, "no lane switches its own feed");
        }
    }

    #[test]
    fn agc_reaches_every_lane_and_is_reported_back() {
        let plan = planned(&DeviceSettings {
            agc: Some(AgcSetting::switched(false)),
            ..DeviceSettings::default()
        })
        .expect("plan");
        for lane in &plan.lanes {
            assert!(matches!(lane.gain, Some(GainMode::Manual(_))));
            assert_eq!(lane.applied.agc, Some(AgcSetting::switched(false)));
        }
    }

    #[test]
    fn the_noise_source_is_not_a_setting_an_operator_can_reach() {
        let refused = planned(&DeviceSettings {
            extra: vec![ExtraValue {
                name: "noise_source".to_owned(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        });
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn a_lane_cannot_be_tuned_away_from_the_others() {
        let refused = planned(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(88e6),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        });
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn the_tuner_cannot_be_bypassed_on_an_array() {
        let refused = planned(&DeviceSettings {
            extra: vec![ExtraValue {
                name: crate::caps::DIRECT_SAMPLING.to_owned(),
                value: "q".into(),
            }],
            ..DeviceSettings::default()
        });
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
        let refused = planned(&DeviceSettings {
            center_hz: Some(5e6),
            ..DeviceSettings::default()
        });
        let Err(DeviceError::Unsupported(message)) = refused else {
            panic!("HF must be refused");
        };
        assert!(!message.contains(crate::caps::DIRECT_SAMPLING), "{message}");
    }

    #[test]
    fn nothing_is_planned_for_a_request_one_lane_cannot_meet() {
        let refused = planned(&DeviceSettings {
            sample_rate: Some(500e3),
            ..DeviceSettings::default()
        });
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
    }
}
