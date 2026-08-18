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
        if !settings.measure_bw_hz.is_finite() || settings.measure_bw_hz <= 0.0 {
            return Err(bad(format!(
                "measure_bw_hz must be positive, got {}",
                settings.measure_bw_hz
            )));
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

    pub(crate) fn tunings(&self, usable_span: f64) -> Vec<Tuning> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.targets.len() {
            let low = self.targets[i];
            let mut j = i + 1;
            while j < self.targets.len() && self.targets[j] - low <= usable_span {
                j += 1;
            }
            out.push(Tuning {
                center_hz: f64::midpoint(low, self.targets[j - 1]),
                first: i,
                last: j - 1,
            });
            i = j;
        }
        out
    }
}

pub(crate) struct Tuning {
    pub(crate) center_hz: f64,
    pub(crate) first: usize,
    pub(crate) last: usize,
}

fn reachable(ranges: &[Range], hz: f64) -> bool {
    ranges.is_empty() || ranges.iter().any(|r| hz >= r.min && hz <= r.max)
}

/// Hands each device set a share of the sweep.
///
/// Contiguous shares are what a set wants: every retune inside a share is a short hop, and the
/// operator reads one band per radio. When the radios do not all reach the same frequencies the
/// split falls back to placing the most constrained targets first and then filling by load, which
/// scatters shares but leaves nothing unswept and no radio carrying the sweep alone.
pub(crate) fn partition(
    targets: &[f64],
    reach: &[Vec<Range>],
) -> Result<Vec<Vec<f64>>, EngineError> {
    let members = reach.len();
    if members == 0 {
        return Err(EngineError::Scan(
            "a scan needs at least one device set".to_string(),
        ));
    }
    if let Some(&hz) = targets
        .iter()
        .find(|&&hz| !reach.iter().any(|ranges| reachable(ranges, hz)))
    {
        return Err(EngineError::Scan(format!(
            "{hz} Hz is outside the tuning range of every device set in the scan"
        )));
    }
    let universal = reach
        .iter()
        .all(|ranges| targets.iter().all(|&hz| reachable(ranges, hz)));
    if universal {
        return Ok(contiguous(targets, members));
    }
    let mut order: Vec<usize> = (0..targets.len()).collect();
    order.sort_by_key(|&i| {
        reach
            .iter()
            .filter(|ranges| reachable(ranges, targets[i]))
            .count()
    });
    let mut shares: Vec<Vec<f64>> = vec![Vec::new(); members];
    for i in order {
        let hz = targets[i];
        let pick = (0..members)
            .filter(|&m| reachable(&reach[m], hz))
            .min_by_key(|&m| (shares[m].len(), m))
            .ok_or_else(|| EngineError::Scan(format!("{hz} Hz is unreachable")))?;
        shares[pick].push(hz);
    }
    for share in &mut shares {
        share.sort_by(f64::total_cmp);
    }
    Ok(shares)
}

fn contiguous(targets: &[f64], members: usize) -> Vec<Vec<f64>> {
    let base = targets.len() / members;
    let extra = targets.len() % members;
    let mut shares = Vec::with_capacity(members);
    let mut cut = 0;
    for i in 0..members {
        let take = base + usize::from(i < extra);
        shares.push(targets[cut..cut + take].to_vec());
        cut += take;
    }
    shares
}

#[cfg(test)]
mod tests {
    use sdrmm_wire::ScanRange;

    use super::*;

    fn settings(ranges: Vec<ScanRange>, frequencies: Vec<f64>) -> ScanSettings {
        ScanSettings {
            ranges,
            frequencies,
            ..ScanSettings::default()
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

    #[test]
    fn tunings_cover_every_target_within_the_usable_span() {
        let plan = ScanPlan::build(&settings(
            vec![ScanRange {
                start_hz: 144_000_000.0,
                stop_hz: 146_000_000.0,
                step_hz: 12_500.0,
            }],
            Vec::new(),
        ))
        .expect("plan");
        let usable = 1_000_000.0;
        let tunings = plan.tunings(usable);
        assert_eq!(
            tunings.len(),
            2,
            "greedy grouping must not split needlessly"
        );
        let mut covered = 0;
        for tuning in &tunings {
            for &target in &plan.targets[tuning.first..=tuning.last] {
                assert!(
                    (target - tuning.center_hz).abs() <= usable / 2.0,
                    "target {target} outside tuning at {}",
                    tuning.center_hz
                );
                covered += 1;
            }
        }
        assert_eq!(covered, plan.targets.len(), "every target scanned once");
    }

    fn wide() -> Vec<Range> {
        vec![Range {
            min: 0.0,
            max: 6e9,
            step: None,
        }]
    }

    fn band(min: f64, max: f64) -> Vec<Range> {
        vec![Range {
            min,
            max,
            step: None,
        }]
    }

    #[test]
    fn matched_radios_each_take_one_contiguous_band() {
        let targets: Vec<f64> = (0..7).map(|i| 100e6 + f64::from(i) * 25e3).collect();
        let shares = partition(&targets, &[wide(), wide()]).expect("split");
        assert_eq!(shares[0], targets[..4]);
        assert_eq!(shares[1], targets[4..]);
    }

    #[test]
    fn a_narrow_radio_only_gets_what_it_can_reach() {
        let targets = vec![100e6, 101e6, 400e6, 401e6];
        let shares = partition(&targets, &[band(90e6, 200e6), wide()]).expect("split");
        assert_eq!(shares[0], vec![100e6, 101e6]);
        assert_eq!(shares[1], vec![400e6, 401e6]);
    }

    #[test]
    fn a_target_no_radio_reaches_is_refused_by_name() {
        let err = partition(&[2.4e9], &[band(90e6, 200e6)]).expect_err("out of reach");
        assert!(
            err.to_string().contains("2400000000"),
            "unhelpful message: {err}"
        );
    }

    #[test]
    fn every_target_lands_in_exactly_one_share() {
        let targets: Vec<f64> = (0..13).map(|i| 100e6 + f64::from(i) * 25e3).collect();
        for members in 1..=5 {
            let reach = vec![wide(); members];
            let shares = partition(&targets, &reach).expect("split");
            let mut seen: Vec<f64> = shares.iter().flatten().copied().collect();
            seen.sort_by(f64::total_cmp);
            assert_eq!(seen, targets, "{members} sets lost or duplicated a target");
            let sizes: Vec<usize> = shares.iter().map(Vec::len).collect();
            let spread = sizes.iter().max().unwrap_or(&0) - sizes.iter().min().unwrap_or(&0);
            assert!(spread <= 1, "{members} sets got lopsided shares: {sizes:?}");
        }
    }
}
