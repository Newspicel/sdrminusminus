use sdrmm_wire::{MAX_SCAN_TARGETS, Range, ScanSettings};

use crate::EngineError;

pub(crate) struct ScanPlan {
    pub(crate) targets: Vec<f64>,
}

impl ScanPlan {
    pub(crate) fn build(settings: &ScanSettings) -> Result<Self, EngineError> {
        let bad = |msg: String| EngineError::Scan(msg);
        if !settings.threshold_db.is_finite() {
            return Err(bad("threshold_db must be finite".to_string()));
        }
        if let Some(bw_hz) = settings.measure_bw_hz
            && (!bw_hz.is_finite() || bw_hz <= 0.0)
        {
            return Err(bad(format!("measure_bw_hz must be positive, got {bw_hz}")));
        }
        let mut targets: Vec<f64> = Vec::new();
        for range in &settings.ranges {
            if !range.start_hz.is_finite() || !range.stop_hz.is_finite() {
                return Err(bad("scan range bounds must be finite".to_string()));
            }
            if !range.step_hz.is_finite() || range.step_hz <= 0.0 {
                return Err(bad(format!(
                    "scan range step must be positive, got {}",
                    range.step_hz
                )));
            }
            if range.stop_hz < range.start_hz {
                return Err(bad(format!(
                    "scan range {} Hz–{} Hz ends before it starts",
                    range.start_hz, range.stop_hz
                )));
            }
            let steps = ((range.stop_hz - range.start_hz) / range.step_hz).floor();
            let too_many = !steps.is_finite()
                || steps < 0.0
                || steps >= MAX_SCAN_TARGETS as f64
                || targets.len() + (steps as usize) + 1 > MAX_SCAN_TARGETS;
            if too_many {
                return Err(bad(format!(
                    "scan expands to more than {MAX_SCAN_TARGETS} targets; widen the step or \
                     narrow the range"
                )));
            }
            let count = steps as usize + 1;
            for i in 0..count {
                targets.push(range.start_hz + range.step_hz * i as f64);
            }
        }
        for &freq in &settings.frequencies {
            if !freq.is_finite() || freq <= 0.0 {
                return Err(bad(format!(
                    "scan frequency {freq} is not a usable Hz value"
                )));
            }
            targets.push(freq);
        }
        if targets.len() > MAX_SCAN_TARGETS {
            return Err(bad(format!(
                "scan expands to more than {MAX_SCAN_TARGETS} targets"
            )));
        }
        for t in &mut targets {
            *t = t.round();
        }
        targets.sort_by(f64::total_cmp);
        targets.dedup();
        if targets.is_empty() {
            return Err(bad(
                "a scan needs at least one range or frequency".to_string()
            ));
        }
        Ok(Self { targets })
    }
}

impl ScanPlan {
    pub(crate) fn check_reach(&self, ranges: &[Range]) -> Result<(), EngineError> {
        match self
            .targets
            .iter()
            .find(|&&hz| !ranges.is_empty() && !ranges.iter().any(|r| r.holds(hz)))
        {
            Some(hz) => Err(EngineError::Scan(format!(
                "{hz} Hz is outside the tuning range of this radio"
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::ScanRange;

    use super::*;

    fn settings(ranges: Vec<ScanRange>, frequencies: Vec<f64>) -> ScanSettings {
        ScanSettings {
            ranges,
            frequencies,
            ..ScanSettings::for_channel(1)
        }
    }

    #[test]
    fn plan_expands_ranges_inclusively_and_dedups() {
        let plan = ScanPlan::build(&settings(
            vec![
                ScanRange {
                    start_hz: 144_000_000.0,
                    stop_hz: 144_100_000.0,
                    step_hz: 25_000.0,
                },
                ScanRange {
                    start_hz: 144_100_000.0,
                    stop_hz: 144_150_000.0,
                    step_hz: 25_000.0,
                },
            ],
            vec![145_500_000.0],
        ))
        .expect("plan");
        assert_eq!(
            plan.targets,
            vec![
                144_000_000.0,
                144_025_000.0,
                144_050_000.0,
                144_075_000.0,
                144_100_000.0,
                144_125_000.0,
                144_150_000.0,
                145_500_000.0,
            ]
        );
    }

    #[test]
    fn plan_stops_at_the_last_whole_step() {
        let plan = ScanPlan::build(&settings(
            vec![ScanRange {
                start_hz: 100.0,
                stop_hz: 249.0,
                step_hz: 50.0,
            }],
            Vec::new(),
        ))
        .expect("plan");
        assert_eq!(plan.targets, vec![100.0, 150.0, 200.0]);
    }

    #[test]
    fn plan_rejects_unusable_settings() {
        for bad in [
            settings(Vec::new(), Vec::new()),
            settings(
                vec![ScanRange {
                    start_hz: 100.0,
                    stop_hz: 200.0,
                    step_hz: 0.0,
                }],
                Vec::new(),
            ),
            settings(
                vec![ScanRange {
                    start_hz: 200.0,
                    stop_hz: 100.0,
                    step_hz: 10.0,
                }],
                Vec::new(),
            ),
            settings(Vec::new(), vec![f64::NAN]),
            settings(
                vec![ScanRange {
                    start_hz: 0.0,
                    stop_hz: 1e9,
                    step_hz: 1.0,
                }],
                Vec::new(),
            ),
        ] {
            assert!(ScanPlan::build(&bad).is_err(), "accepted {bad:?}");
        }
    }

    fn band(min: f64, max: f64) -> Vec<Range> {
        vec![Range {
            min,
            max,
            step: None,
        }]
    }

    #[test]
    fn a_target_the_radio_cannot_reach_is_refused_by_name() {
        let plan = ScanPlan::build(&settings(Vec::new(), vec![100e6, 2.4e9])).expect("plan");
        let err = plan
            .check_reach(&band(90e6, 200e6))
            .expect_err("out of reach");
        assert!(
            err.to_string().contains("2400000000"),
            "unhelpful message: {err}"
        );
        plan.check_reach(&[])
            .expect("a radio that declares no range is taken at its word");
        plan.check_reach(&band(1e6, 6e9)).expect("in reach");
    }
}
