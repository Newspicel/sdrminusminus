use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{Capabilities, DeviceSettings, ExtraValue};

use crate::caps::{self, AGC, BIAS_TEE};

/// Pin 0 of the control dongle switches the calibration noise source into every lane, and pins 1
/// and up switch the lanes' bias tees. Every one of them hangs off that one dongle rather than
/// off the lane it feeds.
pub(crate) const NOISE_SOURCE_PIN: u8 = 0;

pub(crate) const fn bias_tee_pin(lane: usize) -> u8 {
    lane as u8 + 1
}

pub(crate) struct Plan {
    pub(crate) lanes: Vec<caps::Plan>,
    pub(crate) gpio: Vec<(u8, bool)>,
    pub(crate) extra: Vec<ExtraValue>,
}

fn switch(value: &ExtraValue) -> Result<bool, DeviceError> {
    value.value.as_bool().ok_or_else(|| {
        DeviceError::Unsupported(format!(
            "extra setting {}: bad value {}",
            value.name, value.value
        ))
    })
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
        extra: Vec::new(),
    };
    for value in &delta.extra {
        if !capabilities
            .extra
            .iter()
            .any(|setting| setting.name() == value.name)
        {
            return Err(DeviceError::Unsupported(format!(
                "extra setting {}",
                value.name
            )));
        }
        if value.name == BIAS_TEE {
            let on = switch(value)?;
            plan.gpio
                .extend((0..current.len()).map(|lane| (bias_tee_pin(lane), on)));
            plan.extra.push(value.clone());
        }
    }
    for (lane, settled) in current.iter().enumerate() {
        let mut want = delta.for_stream(lane as u32, &capabilities.per_stream);
        want.extra.retain(|value| value.name == AGC);
        plan.lanes
            .push(caps::validate(&want, lane_caps, settled, table)?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{GainValue, StreamSettings};

    use super::*;
    use crate::{
        caps::{GainMode, TUNER_STAGE},
        driver::{BoardVariant, GAIN_VALUES},
    };

    const LANES: usize = 5;

    fn fixture() -> (Capabilities, Capabilities, Vec<DeviceSettings>) {
        let capabilities = caps::kraken_capabilities(LANES as u32, GAIN_VALUES);
        let lane_caps = caps::capabilities(BoardVariant::Generic, GAIN_VALUES);
        let settled = vec![
            DeviceSettings {
                center_hz: Some(100e6),
                sample_rate: Some(2.4e6),
                extra: vec![ExtraValue {
                    name: AGC.to_owned(),
                    value: true.into(),
                }],
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
                gains: vec![GainValue {
                    stage: TUNER_STAGE.to_owned(),
                    value_db: 30.0,
                }],
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
    fn every_lanes_bias_tee_is_a_pin_on_the_control_dongle() {
        let plan = planned(&DeviceSettings {
            extra: vec![ExtraValue {
                name: BIAS_TEE.to_owned(),
                value: true.into(),
            }],
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(
            plan.gpio,
            [(1, true), (2, true), (3, true), (4, true), (5, true)]
        );
        assert_eq!(plan.extra.len(), 1);
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
        assert!(matches!(refused, Err(DeviceError::Unsupported(_))));
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
