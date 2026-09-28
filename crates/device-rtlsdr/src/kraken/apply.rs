use sdrmm_device::{DeviceError, check_stream_settings};
use sdrmm_wire::{Capabilities, DeviceSettings};

use super::Model;
use crate::caps::{self, KRAKEN_MAX_RATE_HZ};

pub(crate) const NOISE_SOURCE_PIN: u8 = 0;

pub(crate) const fn bias_tee_pin(lane: usize) -> u8 {
    lane as u8 + 1
}

pub(crate) struct Plan {
    pub(crate) lanes: Vec<caps::Plan>,
    pub(crate) gpio: Vec<(u8, bool)>,
    pub(crate) bias_tee: Option<bool>,
}

pub(crate) struct Limits<'a> {
    pub(crate) model: Model,
    pub(crate) capabilities: &'a Capabilities,
    pub(crate) lane_caps: &'a Capabilities,
    pub(crate) table: &'a [i32],
}

fn check_limits(delta: &DeviceSettings, limits: &Limits<'_>) -> Result<(), DeviceError> {
    check_stream_settings(delta, limits.capabilities)?;
    if delta
        .sample_rate
        .is_some_and(|rate| rate > KRAKEN_MAX_RATE_HZ)
    {
        return Err(DeviceError::Unsupported(format!(
            "{} runs at most 2.56 MS/s",
            limits.model.name()
        )));
    }
    if delta.bias_tee.is_some() && !limits.capabilities.bias_tee {
        return Err(DeviceError::Unsupported(format!(
            "bias_tee: the {} bias tees are not mapped",
            limits.model.name()
        )));
    }
    Ok(())
}

pub(crate) fn plan(
    delta: &DeviceSettings,
    limits: &Limits<'_>,
    current: &[DeviceSettings],
) -> Result<Plan, DeviceError> {
    check_limits(delta, limits)?;
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
        let mut want = delta.for_stream(lane as u32, &limits.capabilities.per_stream);
        want.bias_tee = None;
        plan.lanes.push(caps::validate(
            &want,
            limits.lane_caps,
            settled,
            limits.table,
        )?);
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::{AgcSetting, ExtraValue, GainKind, GainValue, StreamSettings};

    use super::*;
    use crate::{caps::GainMode, driver::GAIN_VALUES};

    fn fixture(model: Model) -> (Capabilities, Capabilities, Vec<DeviceSettings>) {
        let capabilities = caps::kraken_capabilities(model, model.lanes(), GAIN_VALUES);
        let lane_caps = caps::kraken_lane_capabilities(GAIN_VALUES);
        let settled = vec![
            DeviceSettings {
                center_hz: Some(100e6),
                sample_rate: Some(2.4e6),
                agc: Some(AgcSetting::switched(true)),
                ..DeviceSettings::default()
            };
            model.lanes() as usize
        ];
        (capabilities, lane_caps, settled)
    }

    fn planned_on(model: Model, delta: &DeviceSettings) -> Result<Plan, DeviceError> {
        let (capabilities, lane_caps, settled) = fixture(model);
        let limits = Limits {
            model,
            capabilities: &capabilities,
            lane_caps: &lane_caps,
            table: GAIN_VALUES,
        };
        plan(delta, &limits, &settled)
    }

    fn planned(delta: &DeviceSettings) -> Result<Plan, DeviceError> {
        planned_on(Model::Kraken, delta)
    }

    #[test]
    fn rates_above_2_56_ms_are_refused() {
        for rate in [2.88e6, 3.2e6, 2_560_001.0] {
            let Err(DeviceError::Unsupported(message)) = planned(&DeviceSettings {
                sample_rate: Some(rate),
                ..DeviceSettings::default()
            }) else {
                panic!("{rate} must be refused");
            };
            assert_eq!(message, "KrakenSDR runs at most 2.56 MS/s");
        }
        let plan = planned(&DeviceSettings {
            sample_rate: Some(KRAKEN_MAX_RATE_HZ),
            ..DeviceSettings::default()
        })
        .expect("the cap itself is allowed");
        assert!(
            plan.lanes
                .iter()
                .all(|lane| lane.sample_rate == Some(2_560_000))
        );
    }

    #[test]
    fn a_kerberos_refuses_a_bias_tee_it_cannot_map() {
        let refused = planned_on(
            Model::Kerberos,
            &DeviceSettings {
                bias_tee: Some(true),
                ..DeviceSettings::default()
            },
        );
        assert!(
            matches!(refused, Err(DeviceError::Unsupported(message)) if message.contains("KerberosSDR"))
        );
        let plan = planned_on(
            Model::Kerberos,
            &DeviceSettings {
                center_hz: Some(433.92e6),
                ..DeviceSettings::default()
            },
        )
        .expect("a retune");
        assert_eq!(plan.lanes.len(), 4);
        assert!(plan.gpio.is_empty());
    }

    #[test]
    fn one_tuning_reaches_every_lane() {
        let plan = planned(&DeviceSettings {
            center_hz: Some(433.92e6),
            sample_rate: Some(2.4e6),
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(plan.lanes.len(), 5);
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
    fn a_lanes_agc_stays_on_that_lane() {
        let plan = planned(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 3,
                agc: Some(AgcSetting::switched(false)),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(plan.lanes[3].applied.agc, Some(AgcSetting::switched(false)));
        for (lane, planned) in plan.lanes.iter().enumerate() {
            if lane != 3 {
                assert_eq!(planned.gain, None, "lane {lane} kept its AGC");
            }
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
    fn a_lane_tunes_on_its_own() {
        let plan = planned(&DeviceSettings {
            streams: vec![StreamSettings {
                stream: 1,
                center_hz: Some(88e6),
                ..StreamSettings::default()
            }],
            ..DeviceSettings::default()
        })
        .expect("plan");
        assert_eq!(plan.lanes[1].center_hz, Some(88_000_000));
        for (lane, planned) in plan.lanes.iter().enumerate() {
            if lane != 1 {
                assert_eq!(planned.center_hz, None, "lane {lane} was not retuned");
            }
        }
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
