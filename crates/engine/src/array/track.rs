use sdrmm_channels::array_processor::MAX_LANES;
use sdrmm_dsp::array_sync::{
    DRIFT_MIN_POINTS, DRIFT_MIN_SPAN_S, DriftClass, DriftTrack, SLIP_SAMPLES,
};
use sdrmm_wire::SyncState;

pub(crate) const LOCK_SAMPLES: f64 = 0.25;
pub(crate) const DRIFT_FAIL_PPM: f64 = 0.05;
pub(crate) const SLIPS_REPEATED: usize = 3;
pub(crate) const SLIP_WINDOW_S: f64 = 600.0;
const CLOCK_POINTS: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Residual {
    pub(crate) delay: f64,
    pub(crate) phase_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Verdict {
    pub(crate) state: SyncState,
    pub(crate) lanes: [SyncState; MAX_LANES],
    pub(crate) residuals: [Option<Residual>; MAX_LANES],
    pub(crate) slipped: u32,
    pub(crate) worst_slip: f64,
    pub(crate) recentre: bool,
    pub(crate) drift_ppm: Option<f64>,
    pub(crate) clock_drift: Option<f64>,
    pub(crate) slips_repeated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Measured {
    pub(crate) delay: f64,
    pub(crate) phase_deg: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Observation<'a> {
    pub(crate) t_s: f64,
    pub(crate) lanes: &'a [Option<Measured>],
    pub(crate) sample_rate: f64,
    pub(crate) devices: usize,
    pub(crate) hold: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClockTrack {
    points: [(f64, f64); CLOCK_POINTS],
    len: usize,
}

impl ClockTrack {
    const fn new() -> Self {
        Self {
            points: [(0.0, 0.0); CLOCK_POINTS],
            len: 0,
        }
    }

    fn push(&mut self, t_s: f64, delay: f64) {
        if !(t_s.is_finite() && delay.is_finite()) {
            return;
        }
        if self.len == CLOCK_POINTS {
            self.points.copy_within(1.., 0);
            self.len -= 1;
        }
        self.points[self.len] = (t_s, delay);
        self.len += 1;
        if self.fit().is_some_and(|(_, worst)| worst > SLIP_SAMPLES) {
            self.points[0] = (t_s, delay);
            self.len = 1;
        }
    }

    const fn clear(&mut self) {
        self.len = 0;
    }

    fn slope(&self) -> Option<f64> {
        let points = &self.points[..self.len];
        let span = points.last()?.0 - points.first()?.0;
        if span < DRIFT_MIN_SPAN_S {
            return None;
        }
        self.fit().map(|(slope, _)| slope)
    }

    fn fit(&self) -> Option<(f64, f64)> {
        let points = &self.points[..self.len];
        if points.len() < DRIFT_MIN_POINTS {
            return None;
        }
        let count = points.len() as f64;
        let mean_t = points.iter().map(|point| point.0).sum::<f64>() / count;
        let mean_d = points.iter().map(|point| point.1).sum::<f64>() / count;
        let (mut spread, mut covariance) = (0.0, 0.0);
        for &(t, d) in points {
            spread += (t - mean_t) * (t - mean_t);
            covariance += (t - mean_t) * (d - mean_d);
        }
        if spread <= f64::MIN_POSITIVE {
            return None;
        }
        let slope = covariance / spread;
        let worst = points
            .iter()
            .map(|&(t, d)| (d - mean_d - slope * (t - mean_t)).abs())
            .fold(0.0, f64::max);
        Some((slope, worst))
    }
}

pub(crate) struct Tracker {
    lanes: usize,
    tracks: [DriftTrack; MAX_LANES],
    clocks: [ClockTrack; MAX_LANES],
    anchors: [Option<f64>; MAX_LANES],
    held: [Option<Measured>; MAX_LANES],
    states: [SyncState; MAX_LANES],
    slips: [Option<f64>; SLIPS_REPEATED],
    slip_at: usize,
}

impl Tracker {
    pub(crate) fn new(lanes: usize) -> Self {
        Self {
            lanes: lanes.min(MAX_LANES),
            tracks: [DriftTrack::new(); MAX_LANES],
            clocks: [ClockTrack::new(); MAX_LANES],
            anchors: [None; MAX_LANES],
            held: [None; MAX_LANES],
            states: [SyncState::Searching; MAX_LANES],
            slips: [None; SLIPS_REPEATED],
            slip_at: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        for lane in 0..MAX_LANES {
            self.forget(lane);
        }
        self.states = [SyncState::Searching; MAX_LANES];
    }

    pub(crate) fn forget(&mut self, lane: usize) {
        if lane >= MAX_LANES {
            return;
        }
        self.tracks[lane].clear();
        self.clocks[lane].clear();
        self.anchors[lane] = None;
        self.held[lane] = None;
        self.states[lane] = SyncState::Lost;
    }

    #[cfg(test)]
    fn held(&self, lane: usize) -> Option<Measured> {
        self.held.get(lane).copied().flatten()
    }

    pub(crate) fn observe(&mut self, seen: &Observation<'_>) -> Verdict {
        let mut verdict = Verdict {
            state: SyncState::Locked,
            lanes: self.states,
            residuals: [None; MAX_LANES],
            slipped: 0,
            worst_slip: 0.0,
            recentre: false,
            drift_ppm: None,
            clock_drift: None,
            slips_repeated: false,
        };
        verdict.lanes[0] = SyncState::Locked;
        self.states[0] = SyncState::Locked;
        for lane in 1..self.lanes {
            if let Some(measured) = seen.lanes.get(lane).copied().flatten() {
                self.lane(lane, measured, seen, &mut verdict);
            }
        }
        verdict.state = worst(&verdict.lanes[..self.lanes]);
        verdict.slips_repeated = self.repeated(seen.t_s);
        if seen.devices > 1 {
            verdict.drift_ppm = self.drift_ppm(seen.sample_rate);
            verdict.clock_drift = verdict.drift_ppm.filter(|ppm| ppm.abs() > DRIFT_FAIL_PPM);
        }
        verdict
    }

    fn lane(
        &mut self,
        lane: usize,
        measured: Measured,
        seen: &Observation<'_>,
        verdict: &mut Verdict,
    ) {
        let anchor = *self.anchors[lane].get_or_insert(measured.delay);
        self.clocks[lane].push(seen.t_s, measured.delay);
        let residual = self.held[lane].map(|held| Residual {
            delay: measured.delay - held.delay,
            phase_deg: match (measured.phase_deg, held.phase_deg) {
                (Some(now), Some(before)) => wrap_deg(now - before),
                _ => 0.0,
            },
        });
        verdict.residuals[lane] = residual;
        let state = match self.tracks[lane].push(seen.t_s, measured.delay - anchor) {
            DriftClass::Slipped { by } => {
                verdict.slipped |= 1 << lane;
                if by.abs() > verdict.worst_slip.abs() {
                    verdict.worst_slip = by;
                }
                self.slip(seen.t_s);
                self.anchors[lane] = Some(measured.delay);
                SyncState::Lost
            }
            DriftClass::Drifting { .. } => SyncState::Drifting,
            DriftClass::Locked => SyncState::Locked,
        };
        if residual.is_some_and(|residual| residual.delay.abs() > LOCK_SAMPLES) {
            verdict.recentre = true;
        }
        self.states[lane] = state;
        verdict.lanes[lane] = state;
        if seen.hold || self.held[lane].is_none() {
            self.held[lane] = Some(measured);
        }
    }

    fn slip(&mut self, t_s: f64) {
        self.slips[self.slip_at] = Some(t_s);
        self.slip_at = (self.slip_at + 1) % SLIPS_REPEATED;
    }

    fn repeated(&self, t_s: f64) -> bool {
        self.slips
            .iter()
            .all(|slip| slip.is_some_and(|at| t_s - at <= SLIP_WINDOW_S))
    }

    fn drift_ppm(&self, sample_rate: f64) -> Option<f64> {
        if !(sample_rate.is_finite() && sample_rate > 0.0) {
            return None;
        }
        self.clocks[1..self.lanes]
            .iter()
            .filter_map(ClockTrack::slope)
            .map(|slope| slope / sample_rate * 1e6)
            .max_by(|a, b| a.abs().total_cmp(&b.abs()))
    }
}

pub(crate) fn worst(states: &[SyncState]) -> SyncState {
    let rank = |state: &SyncState| match state {
        SyncState::Lost => 4,
        SyncState::Searching => 3,
        SyncState::Drifting => 2,
        SyncState::Locked => 1,
        SyncState::Idle => 0,
    };
    states
        .iter()
        .copied()
        .max_by_key(rank)
        .unwrap_or(SyncState::Idle)
}

pub(crate) fn wrap_deg(degrees: f64) -> f64 {
    (degrees + 180.0).rem_euclid(360.0) - 180.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 2_400_000.0;

    fn at(delays: &[f64]) -> Vec<Option<Measured>> {
        delays
            .iter()
            .map(|delay| {
                Some(Measured {
                    delay: *delay,
                    phase_deg: Some(10.0),
                })
            })
            .collect()
    }

    fn seen(t_s: f64, lanes: &[Option<Measured>], devices: usize) -> Observation<'_> {
        Observation {
            t_s,
            lanes,
            sample_rate: RATE,
            devices,
            hold: true,
        }
    }

    #[test]
    fn a_steady_lane_stays_locked() {
        let mut tracker = Tracker::new(3);
        for step in 0..6 {
            let wobble = if step % 2 == 0 { 0.02 } else { -0.02 };
            let lanes = at(&[0.0, 12.3 + wobble, -40.1]);
            let verdict = tracker.observe(&seen(60.0 * f64::from(step), &lanes, 1));
            assert_eq!(verdict.state, SyncState::Locked, "step {step}");
            assert!(!verdict.recentre);
        }
        let residual = tracker
            .observe(&seen(400.0, &at(&[0.0, 12.32, -40.1]), 1))
            .residuals[1]
            .expect("a residual");
        assert!((residual.delay - 0.04).abs() < 1e-9, "{residual:?}");
    }

    #[test]
    fn a_residual_jump_is_a_slip_and_loses_the_lane() {
        let mut tracker = Tracker::new(2);
        tracker.observe(&seen(0.0, &at(&[0.0, 5.0]), 1));
        let verdict = tracker.observe(&seen(60.0, &at(&[0.0, 8.0]), 1));
        assert_eq!(verdict.state, SyncState::Lost);
        assert_eq!(verdict.lanes[1], SyncState::Lost);
        assert_eq!(verdict.slipped, 0b10);
        assert!((verdict.worst_slip - 3.0).abs() < 1e-9);
        assert!(verdict.recentre);
        let verdict = tracker.observe(&seen(120.0, &at(&[0.0, 8.0]), 1));
        assert_eq!(verdict.state, SyncState::Locked);
    }

    #[test]
    fn a_slow_walk_is_drifting() {
        let mut tracker = Tracker::new(2);
        let mut state = SyncState::Locked;
        for step in 0..8 {
            let t = 20.0 * f64::from(step);
            state = tracker
                .observe(&seen(t, &at(&[0.0, 3.0 + 0.02 * t]), 1))
                .state;
        }
        assert_eq!(state, SyncState::Drifting);
    }

    #[test]
    fn three_slips_in_ten_minutes_repeat() {
        let mut tracker = Tracker::new(2);
        let mut delay = 0.0;
        let mut repeated = Vec::new();
        for step in 0..7 {
            delay += if step % 2 == 1 { 4.0 } else { 0.0 };
            let verdict = tracker.observe(&seen(60.0 * f64::from(step), &at(&[0.0, delay]), 1));
            repeated.push(verdict.slips_repeated);
        }
        assert!(!repeated[3]);
        assert!(repeated[5]);
        let quiet = tracker.observe(&seen(420.0 + SLIP_WINDOW_S, &at(&[0.0, delay]), 1));
        assert!(!quiet.slips_repeated);
    }

    #[test]
    fn drift_between_devices_fails_the_clock() {
        let rate = 48_000.0;
        let walk = 0.01;
        let run = |devices: usize| {
            let mut tracker = Tracker::new(2);
            let mut last = None;
            for step in 0..5 {
                let t = 10.0 * f64::from(step);
                let lanes = at(&[0.0, 100.0 + walk * t]);
                last = Some(tracker.observe(&Observation {
                    sample_rate: rate,
                    ..seen(t, &lanes, devices)
                }));
            }
            last.expect("a verdict")
        };
        let ppm = run(2).clock_drift.expect("a drift");
        assert!((ppm - walk / rate * 1e6).abs() < 1e-6, "{ppm}");
        assert_eq!(run(1).clock_drift, None);
        assert_eq!(run(1).drift_ppm, None);
    }

    #[test]
    fn a_clock_walk_that_slips_every_check_is_still_clock_drift() {
        let ppm = 1.0;
        let walk = ppm * RATE / 1e6;
        let mut tracker = Tracker::new(2);
        let verdicts: Vec<Verdict> = (0..4)
            .map(|step| {
                let t = 60.0 * f64::from(step);
                tracker.observe(&seen(t, &at(&[0.0, 50.0 + walk * t]), 2))
            })
            .collect();
        assert!(verdicts[1..].iter().all(|verdict| verdict.slipped == 0b10));
        assert_eq!(verdicts[1].clock_drift, None);
        let measured = verdicts[2].clock_drift.expect("a clock drift");
        assert!((measured - ppm).abs() < 1e-9, "{measured}");
    }

    #[test]
    fn one_jump_between_radios_is_a_slip_not_clock_drift() {
        let mut tracker = Tracker::new(2);
        for step in 0..8 {
            let delay = if step < 4 { 50.0 } else { 90.0 };
            let verdict = tracker.observe(&seen(60.0 * f64::from(step), &at(&[0.0, delay]), 2));
            assert_eq!(verdict.clock_drift, None, "step {step}");
            assert_eq!(verdict.slipped != 0, step == 4, "step {step}");
        }
    }

    #[test]
    fn a_forgotten_lane_starts_over_without_a_slip() {
        let mut tracker = Tracker::new(2);
        tracker.observe(&seen(0.0, &at(&[0.0, 5.0]), 1));
        tracker.forget(1);
        assert!(tracker.held(1).is_none());
        let verdict = tracker.observe(&seen(60.0, &at(&[0.0, 45.0]), 1));
        assert_eq!(verdict.slipped, 0);
        assert_eq!(verdict.state, SyncState::Locked);
        assert!(verdict.residuals[1].is_none());
    }

    #[test]
    fn a_skipped_lane_keeps_its_state() {
        let mut tracker = Tracker::new(3);
        tracker.observe(&seen(0.0, &at(&[0.0, 1.0, 2.0]), 1));
        let lanes = [at(&[0.0])[0], None, at(&[2.0])[0]];
        let verdict = tracker.observe(&seen(60.0, &lanes, 1));
        assert_eq!(verdict.lanes[1], SyncState::Locked);
        assert!(verdict.residuals[1].is_none());
        assert!(tracker.held(1).is_some());
    }

    #[test]
    fn a_check_does_not_move_the_held_delay() {
        let mut tracker = Tracker::new(2);
        tracker.observe(&seen(0.0, &at(&[0.0, 1.0]), 1));
        let lanes = at(&[0.0, 1.2]);
        let verdict = tracker.observe(&Observation {
            hold: false,
            ..seen(60.0, &lanes, 1)
        });
        assert!(!verdict.recentre);
        assert_eq!(tracker.held(1).map(|held| held.delay), Some(1.0));
        assert_eq!(wrap_deg(190.0), -170.0);
    }
}
