use std::f64::consts::SQRT_2;

use sdrmm_dsp::{
    special::{erf, norm_deg},
    sweep::{
        HeadingSample, LevelSample, SweepBearing, SweepConfig, SweepEstimator, SweepPhase,
        SweepReject,
    },
};
use sdrmm_wire::{
    BearingSource, DfBearing, HuntSweep, HuntSweepParams, PositionFix, SweepState,
    hunt::HUNT_SWEEP_BINS, patch::MAX_NODE_ID_LEN, processor::df::MAX_STATION_ID_LEN,
};

use crate::{ChannelError, pose_clock::PoseClock};

const DEFAULT_HEADING_SIGMA_DEG: f32 = 10.0;
const MAX_HEADING_AGE_S: f64 = 2.0;
const MOVING_MPS: f64 = 1.0;
const CONFIDENCE_WINDOW_DEG: f64 = 5.0;
const LIKELIHOOD_FLOOR: f32 = 0.02;
const NANOS_PER_S: f64 = 1e9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MarkRefusal {
    #[error("No heading")]
    NoHeading,
    #[error("No position")]
    NoPosition,
}

pub struct SweepDf {
    node: String,
    station_id: String,
    params: HuntSweepParams,
    estimator: SweepEstimator,
    clock: PoseClock,
    on: bool,
    last_fix: Option<PositionFix>,
    last_heading: Option<HeadingSample>,
    bearing: SweepBearing,
    status: HuntSweep,
}

struct Estimate {
    bearing_deg: f64,
    sigma_deg: f64,
    confidence: f64,
    source: BearingSource,
    likelihood: Vec<u8>,
}

impl SweepDf {
    pub fn new(
        node: String,
        station_id: String,
        params: &HuntSweepParams,
    ) -> Result<Self, ChannelError> {
        checked(params)?;
        named(&node, MAX_NODE_ID_LEN, "Bad hunt node")?;
        named(&station_id, MAX_STATION_ID_LEN, "Bad station id")?;
        Ok(Self {
            node,
            station_id,
            params: *params,
            estimator: SweepEstimator::new(config(params)),
            clock: PoseClock::new(),
            on: false,
            last_fix: None,
            last_heading: None,
            bearing: SweepBearing::default(),
            status: HuntSweep {
                bins: vec![0; HUNT_SWEEP_BINS],
                ..HuntSweep::default()
            },
        })
    }

    pub fn apply(&mut self, params: &HuntSweepParams) -> Result<(), ChannelError> {
        checked(params)?;
        let turn = params.mount_offset_deg - self.params.mount_offset_deg;
        if let Some(heading) = &mut self.last_heading {
            heading.heading_deg = norm_deg(heading.heading_deg + turn);
        }
        self.params = *params;
        self.estimator.configure(config(params));
        Ok(())
    }

    pub fn set_on(&mut self, on: bool) {
        self.on = on;
        if !on {
            self.status.state = SweepState::Off;
            return;
        }
        self.estimator.reset();
        if let Some(heading) = self.last_heading {
            self.estimator.push_heading(heading);
        }
        self.status.bins.fill(0);
        self.status.peak_deg = None;
        self.status.sigma_deg = None;
        self.status.fit = None;
        self.status.contrast_db = None;
        self.status.covered_deg = 0.0;
        self.status.rate_dps = 0.0;
        self.status.state = if self.last_heading.is_some() {
            SweepState::Idle
        } else {
            SweepState::NoHeading
        };
    }

    #[must_use]
    pub const fn is_on(&self) -> bool {
        self.on
    }

    pub fn pose(&mut self, fix: &PositionFix, received_s: f64) -> Result<(), ChannelError> {
        fix.validate()
            .map_err(|problem| ChannelError::InvalidPayload(problem.to_owned()))?;
        let fix_ns = fix_time_ns(&fix.time)?;
        let mapped_ns = self.clock.map(fix_ns, (received_s * NANOS_PER_S) as i64);
        let t_s = mapped_ns as f64 / NANOS_PER_S;
        self.last_heading = fix.attitude.heading_deg.map(|heading| HeadingSample {
            t_s,
            heading_deg: norm_deg(heading + self.params.mount_offset_deg),
            sigma_deg: fix
                .attitude
                .heading_accuracy_deg
                .map_or(DEFAULT_HEADING_SIGMA_DEG, |deg| deg as f32),
        });
        self.last_fix = Some(fix.clone());
        self.status.heading_deg = self.last_heading.map(|heading| heading.heading_deg as f32);
        match self.last_heading {
            Some(heading) => self.estimator.push_heading(heading),
            None if self.on => self.status.state = SweepState::NoHeading,
            None => {}
        }
        Ok(())
    }

    pub fn level(&mut self, level_db: f32, at_s: f64, freq_hz: f64) -> Option<DfBearing> {
        if !self.on {
            return None;
        }
        let sample = LevelSample {
            t_s: at_s,
            level_db,
        };
        let outcome = self.estimator.push_level(sample, &mut self.bearing);
        self.refresh(at_s);
        match outcome {
            Some(Ok(())) => self.accept(freq_hz),
            Some(Err(reject)) => {
                self.status.state = rejected(reject);
                None
            }
            None => {
                self.status.state = self.waiting_state(at_s);
                None
            }
        }
    }

    pub fn mark(&self, at_s: f64, freq_hz: f64) -> Result<DfBearing, MarkRefusal> {
        let fix = self.last_fix.as_ref().ok_or(MarkRefusal::NoPosition)?;
        let heading = self.current_heading(at_s).ok_or(MarkRefusal::NoHeading)?;
        let sigma_deg = (self.params.beamwidth_deg / 4.0).max(f64::from(heading.sigma_deg));
        let estimate = Estimate {
            bearing_deg: heading.heading_deg,
            sigma_deg,
            confidence: erf(CONFIDENCE_WINDOW_DEG / (sigma_deg * SQRT_2)),
            source: BearingSource::Mark,
            likelihood: Vec::new(),
        };
        Ok(self.event(fix, Some(heading), freq_hz, estimate))
    }

    #[must_use]
    pub const fn status(&self) -> &HuntSweep {
        &self.status
    }

    fn accept(&mut self, freq_hz: f64) -> Option<DfBearing> {
        self.status.state = SweepState::Done;
        self.status.peak_deg = Some(self.bearing.bearing_deg as f32);
        self.status.sigma_deg = Some(self.bearing.sigma_deg);
        self.status.fit = Some(self.bearing.fit);
        self.status.contrast_db = Some(self.bearing.contrast_db);
        let fix = self.last_fix.as_ref()?;
        let estimate = Estimate {
            bearing_deg: self.bearing.bearing_deg,
            sigma_deg: f64::from(self.bearing.sigma_deg),
            confidence: f64::from(self.bearing.confidence),
            source: BearingSource::Sweep,
            likelihood: quantized(&self.bearing.likelihood),
        };
        Some(self.event(fix, self.last_heading, freq_hz, estimate))
    }

    fn event(
        &self,
        fix: &PositionFix,
        heading: Option<HeadingSample>,
        freq_hz: f64,
        estimate: Estimate,
    ) -> DfBearing {
        DfBearing {
            bearing_deg: estimate.bearing_deg as f32,
            confidence: estimate.confidence as f32,
            lat: Some(fix.latitude),
            lon: Some(fix.longitude),
            station_id: Some(self.station_id.clone()),
            node: self.node.clone(),
            sigma_deg: estimate.sigma_deg as f32,
            accuracy_m: fix.accuracy_m.map(|metres| metres as f32),
            heading_deg: heading.map(|heading| heading.heading_deg as f32),
            heading_sigma_deg: heading.map(|heading| heading.sigma_deg),
            relative_deg: None,
            mirror_deg: None,
            freq_hz: Some(freq_hz),
            source: estimate.source,
            moving: fix.speed_mps.is_some_and(|speed| speed > MOVING_MPS),
            others: Vec::new(),
            likelihood: estimate.likelihood,
        }
    }

    fn current_heading(&self, at_s: f64) -> Option<HeadingSample> {
        self.last_heading
            .filter(|heading| at_s - heading.t_s <= MAX_HEADING_AGE_S)
    }

    fn waiting_state(&self, at_s: f64) -> SweepState {
        if self.current_heading(at_s).is_none() {
            return SweepState::NoHeading;
        }
        match self.estimator.phase() {
            SweepPhase::Sweeping { .. } => SweepState::Sweeping,
            SweepPhase::Idle if is_outcome(self.status.state) => self.status.state,
            SweepPhase::Idle => SweepState::Idle,
        }
    }

    fn refresh(&mut self, at_s: f64) {
        let bins = self.estimator.bins();
        let (low, high) = bins
            .iter()
            .filter(|value| value.is_finite())
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), &value| {
                (low.min(value), high.max(value))
            });
        let range = high - low;
        for (scaled, &value) in self.status.bins.iter_mut().zip(bins) {
            *scaled = match (value.is_finite(), range > 0.0) {
                (false, _) => 0,
                (true, false) => u8::MAX,
                (true, true) => (255.0 * (value - low) / range).round() as u8,
            };
        }
        self.status.covered_deg = self.estimator.covered_deg();
        self.status.rate_dps = match self.estimator.phase() {
            SweepPhase::Sweeping { rate_dps, .. } => rate_dps,
            SweepPhase::Idle => 0.0,
        };
        self.status.lag_ms = (self.estimator.lag_s() * 1_000.0) as f32;
        self.status.heading_deg = self
            .current_heading(at_s)
            .map(|heading| heading.heading_deg as f32);
    }
}

fn checked(params: &HuntSweepParams) -> Result<(), ChannelError> {
    params.problem().map_or(Ok(()), |problem| {
        Err(ChannelError::InvalidSettings(problem.to_owned()))
    })
}

fn named(id: &str, longest: usize, problem: &str) -> Result<(), ChannelError> {
    if id.is_empty() || id.len() > longest {
        return Err(ChannelError::InvalidSettings(problem.to_owned()));
    }
    Ok(())
}

fn config(params: &HuntSweepParams) -> SweepConfig {
    SweepConfig {
        beamwidth_deg: params.beamwidth_deg,
        front_back_db: params.front_back_db,
        min_span_deg: params.min_span_deg,
        min_contrast_db: params.min_contrast_db,
        ..SweepConfig::default()
    }
}

fn fix_time_ns(time: &str) -> Result<i64, ChannelError> {
    let unusable = || ChannelError::InvalidPayload(format!("fix time {time:?} is unusable"));
    let stamp: jiff::Timestamp = time.parse().map_err(|_| unusable())?;
    i64::try_from(stamp.as_nanosecond()).map_err(|_| unusable())
}

const fn rejected(reject: SweepReject) -> SweepState {
    match reject {
        SweepReject::ShortSpan => SweepState::ShortSpan,
        SweepReject::LowContrast => SweepState::LowContrast,
        SweepReject::PoorFit => SweepState::PoorFit,
        SweepReject::HeadingPoor => SweepState::HeadingPoor,
        SweepReject::TooFast => SweepState::TooFast,
    }
}

const fn is_outcome(state: SweepState) -> bool {
    matches!(
        state,
        SweepState::Done
            | SweepState::ShortSpan
            | SweepState::LowContrast
            | SweepState::PoorFit
            | SweepState::HeadingPoor
            | SweepState::TooFast
    )
}

fn quantized(likelihood: &[f32]) -> Vec<u8> {
    let floor = LIKELIHOOD_FLOOR.ln();
    likelihood
        .iter()
        .map(|&value| (255.0 * (value - floor) / -floor).round().clamp(0.0, 255.0) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use sdrmm_dsp::special::wrap_deg;
    use sdrmm_wire::Attitude;

    use super::*;

    const TICK_S: f64 = 0.05;
    const EPOCH_NS: i128 = 1_790_000_000_000_000_000;
    const EPOCH_S: f64 = 1_790_000_000.0;
    const SOURCE_DEG: f64 = 123.0;
    const FREQ_HZ: f64 = 433.92e6;

    struct Rng(u64);

    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn normal(&mut self) -> f64 {
            let u = self.uniform().max(1e-300);
            let v = self.uniform();
            (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
        }
    }

    struct Walk {
        legs: Vec<(f64, f64)>,
        start_deg: f64,
    }

    impl Walk {
        fn new(start_deg: f64) -> Self {
            Self {
                legs: Vec::new(),
                start_deg,
            }
        }

        fn hold(mut self, seconds: f64) -> Self {
            self.legs.push((seconds, 0.0));
            self
        }

        fn turn(mut self, degrees: f64, rate_dps: f64) -> Self {
            self.legs
                .push((degrees.abs() / rate_dps, degrees.signum() * rate_dps));
            self
        }

        fn ticks(&self) -> usize {
            (self.legs.iter().map(|leg| leg.0).sum::<f64>() / TICK_S).ceil() as usize + 1
        }

        fn heading(&self, t: f64) -> f64 {
            let mut heading = self.start_deg;
            let mut left = t.max(0.0);
            for &(seconds, rate) in &self.legs {
                let used = left.min(seconds);
                heading += rate * used;
                left -= used;
            }
            heading
        }
    }

    fn swing() -> Walk {
        Walk::new(10.0).hold(1.0).turn(270.0, 90.0).hold(1.5)
    }

    fn pattern(offset_deg: f64) -> f64 {
        let half = 30f64.to_radians().cos();
        let power = 0.5f64.ln() / ((1.0 + half) / 2.0).ln();
        let cosine = offset_deg.to_radians().cos();
        (10.0 * power * ((1.0 + cosine) / 2.0).log10()).max(-15.0)
    }

    fn fix(t_s: f64, heading_deg: Option<f64>) -> PositionFix {
        let stamp = jiff::Timestamp::from_nanosecond(EPOCH_NS + (t_s * 1e9).round() as i128)
            .expect("timestamp");
        PositionFix {
            latitude: 48.137,
            longitude: 11.575,
            altitude_m: None,
            accuracy_m: Some(4.0),
            speed_mps: Some(0.3),
            track_deg: None,
            time: stamp.to_string(),
            attitude: Attitude {
                heading_deg,
                heading_accuracy_deg: heading_deg.map(|_| 2.0),
                ..Attitude::default()
            },
        }
    }

    fn sweeping(params: &HuntSweepParams) -> SweepDf {
        let mut df = SweepDf::new("hunt-1".to_owned(), "phone-a".to_owned(), params).unwrap();
        df.set_on(true);
        df
    }

    fn run(df: &mut SweepDf, walk: &Walk, level_lag_s: f64, delays: &[f64]) -> Vec<DfBearing> {
        let mut received = Vec::with_capacity(delays.len());
        let mut latest = f64::NEG_INFINITY;
        for (tick, delay) in delays.iter().enumerate() {
            latest = latest.max(tick as f64 * TICK_S + delay);
            received.push(latest);
        }
        let mut rng = Rng(11);
        let mut delivered = 0;
        let mut bearings = Vec::new();
        for tick in 0..delays.len() {
            let heard = tick as f64 * TICK_S + TICK_S / 2.0;
            while delivered < delays.len() && received[delivered] <= heard {
                let t = delivered as f64 * TICK_S;
                let pose = fix(t, Some(norm_deg(walk.heading(t))));
                df.pose(&pose, EPOCH_S + received[delivered]).unwrap();
                delivered += 1;
            }
            let level = -60.0
                + pattern(walk.heading(heard - level_lag_s) - SOURCE_DEG)
                + 0.5 * rng.normal();
            bearings.extend(df.level(level as f32, EPOCH_S + heard, FREQ_HZ));
        }
        bearings
    }

    #[test]
    fn hunt_sweep_turns_levels_and_poses_into_a_bearing() {
        let walk = swing();
        let mut df = sweeping(&HuntSweepParams::default());
        let bearings = run(&mut df, &walk, 0.1, &vec![0.0; walk.ticks()]);
        assert_eq!(bearings.len(), 1, "{bearings:?}");
        let bearing = &bearings[0];
        assert!(
            wrap_deg(f64::from(bearing.bearing_deg) - SOURCE_DEG).abs() < 3.0,
            "{bearing:?}"
        );
        assert_eq!(bearing.source, BearingSource::Sweep);
        assert_eq!(bearing.lat, Some(48.137));
        assert_eq!(bearing.lon, Some(11.575));
        assert_eq!(bearing.node, "hunt-1");
        assert_eq!(bearing.station_id.as_deref(), Some("phone-a"));
        assert_eq!(bearing.freq_hz, Some(FREQ_HZ));
        assert!(!bearing.moving);
        assert_eq!(bearing.likelihood.len(), 360);
        let best = bearing
            .likelihood
            .iter()
            .enumerate()
            .max_by_key(|(_, value)| **value)
            .map(|(degree, _)| degree as f64)
            .unwrap();
        assert!(wrap_deg(best - SOURCE_DEG).abs() < 3.0, "peak at {best}");
        assert!(bearing.confidence > 0.0 && bearing.confidence < 1.0);
        let status = df.status();
        assert_eq!(status.state, SweepState::Done);
        assert_eq!(status.peak_deg, Some(bearing.bearing_deg));
        assert_eq!(status.bins.len(), HUNT_SWEEP_BINS);
        assert_eq!(status.bins.iter().max(), Some(&u8::MAX));
        assert!(status.covered_deg >= 270.0, "{status:?}");
        assert!((status.lag_ms - 100.0).abs() < 1.0);
    }

    #[test]
    fn jittered_pose_delivery_adds_no_heading_noise() {
        let walk = swing();
        let steady = vec![0.05; walk.ticks()];
        let mut rng = Rng(5);
        let jittered: Vec<f64> = (0..walk.ticks())
            .map(|_| 0.05 + 0.25 * rng.uniform())
            .collect();
        let mut clean_df = sweeping(&HuntSweepParams::default());
        let clean = run(&mut clean_df, &walk, 0.15, &steady);
        let mut noisy_df = sweeping(&HuntSweepParams::default());
        let noisy = run(&mut noisy_df, &walk, 0.15, &jittered);
        assert_eq!(clean.len(), 1, "{clean:?}");
        assert_eq!(noisy.len(), 1, "{noisy:?}");
        let clean = f64::from(clean[0].bearing_deg);
        let noisy = f64::from(noisy[0].bearing_deg);
        assert!(wrap_deg(clean - SOURCE_DEG).abs() < 3.0, "clean {clean}");
        assert!(wrap_deg(noisy - clean).abs() < 1.0, "{noisy} vs {clean}");
    }

    #[test]
    fn hunt_sweep_without_heading_says_no_heading() {
        let mut df = sweeping(&HuntSweepParams::default());
        assert_eq!(df.status().state, SweepState::NoHeading);
        for tick in 0..40 {
            let t = f64::from(tick) * TICK_S;
            df.pose(&fix(t, None), EPOCH_S + t).unwrap();
            assert_eq!(df.level(-50.0, EPOCH_S + t, FREQ_HZ), None);
        }
        assert_eq!(df.status().state, SweepState::NoHeading);
        assert_eq!(df.status().heading_deg, None);
        assert_eq!(df.mark(EPOCH_S + 2.0, FREQ_HZ), Err(MarkRefusal::NoHeading));
    }

    #[test]
    fn hunt_mark_uses_the_current_heading_and_mount_offset() {
        let mut df = SweepDf::new(
            "hunt-1".to_owned(),
            "phone-a".to_owned(),
            &HuntSweepParams {
                mount_offset_deg: -90.0,
                ..HuntSweepParams::default()
            },
        )
        .unwrap();
        assert_eq!(df.mark(EPOCH_S, FREQ_HZ), Err(MarkRefusal::NoPosition));
        df.pose(&fix(0.0, Some(100.0)), EPOCH_S + 0.1).unwrap();
        let mark = df.mark(EPOCH_S + 0.2, FREQ_HZ).unwrap();
        assert!((mark.bearing_deg - 10.0).abs() < 1e-4, "{mark:?}");
        assert_eq!(mark.source, BearingSource::Mark);
        assert_eq!(mark.sigma_deg, 15.0);
        assert!((f64::from(mark.confidence) - erf(5.0 / (15.0 * SQRT_2))).abs() < 1e-6);
        assert_eq!(mark.heading_deg, Some(10.0));
        assert_eq!(mark.heading_sigma_deg, Some(2.0));
        assert_eq!(mark.lat, Some(48.137));
        assert_eq!(mark.accuracy_m, Some(4.0));
        assert!(mark.likelihood.is_empty());
        assert_eq!(df.status().state, SweepState::Off, "a mark needs no sweep");
        assert_eq!(
            df.mark(EPOCH_S + 5.0, FREQ_HZ),
            Err(MarkRefusal::NoHeading),
            "a stale heading is no heading"
        );

        df.apply(&HuntSweepParams::default()).unwrap();
        let mut shaky = fix(1.0, Some(100.0));
        shaky.attitude.heading_accuracy_deg = Some(25.0);
        df.pose(&shaky, EPOCH_S + 1.1).unwrap();
        let mark = df.mark(EPOCH_S + 1.2, FREQ_HZ).unwrap();
        assert!((mark.bearing_deg - 100.0).abs() < 1e-4, "{mark:?}");
        assert_eq!(mark.sigma_deg, 25.0, "a poor compass widens the mark");

        df.apply(&HuntSweepParams {
            mount_offset_deg: 90.0,
            ..HuntSweepParams::default()
        })
        .unwrap();
        let mark = df.mark(EPOCH_S + 1.3, FREQ_HZ).unwrap();
        assert!(
            (mark.bearing_deg - 190.0).abs() < 1e-4,
            "a new mount offset turns the held heading: {mark:?}"
        );
    }

    #[test]
    fn a_sweep_that_is_off_ignores_levels_and_on_starts_fresh() {
        let walk = swing();
        let mut df = sweeping(&HuntSweepParams::default());
        assert_eq!(run(&mut df, &walk, 0.1, &vec![0.0; walk.ticks()]).len(), 1);
        df.set_on(false);
        assert_eq!(df.status().state, SweepState::Off);
        assert_eq!(df.level(-40.0, EPOCH_S + 10.0, FREQ_HZ), None);
        df.set_on(true);
        let status = df.status();
        assert_eq!(status.state, SweepState::Idle);
        assert_eq!(status.peak_deg, None);
        assert!(status.bins.iter().all(|&bin| bin == 0));
    }

    #[test]
    fn a_short_swing_says_short_span() {
        let walk = Walk::new(10.0).hold(1.0).turn(90.0, 90.0).hold(1.5);
        let mut df = sweeping(&HuntSweepParams::default());
        assert!(run(&mut df, &walk, 0.1, &vec![0.0; walk.ticks()]).is_empty());
        assert_eq!(df.status().state, SweepState::ShortSpan);
    }

    #[test]
    fn bad_params_ids_and_fixes_are_refused() {
        let wide = HuntSweepParams {
            beamwidth_deg: 5.0,
            ..HuntSweepParams::default()
        };
        assert!(matches!(
            SweepDf::new("h".to_owned(), "h".to_owned(), &wide),
            Err(ChannelError::InvalidSettings(text)) if text == "Beamwidth out of range"
        ));
        for (node, station) in [("", "h"), ("h", ""), ("h", &"s".repeat(65))] {
            assert!(matches!(
                SweepDf::new(
                    node.to_owned(),
                    station.to_owned(),
                    &HuntSweepParams::default()
                ),
                Err(ChannelError::InvalidSettings(_))
            ));
        }
        let mut df = sweeping(&HuntSweepParams::default());
        assert!(df.apply(&wide).is_err());
        let mut pose = fix(0.0, Some(10.0));
        pose.time = "yesterday".to_owned();
        assert!(matches!(
            df.pose(&pose, EPOCH_S),
            Err(ChannelError::InvalidPayload(_))
        ));
        let mut lost = fix(0.0, Some(10.0));
        lost.latitude = f64::NAN;
        assert!(matches!(
            df.pose(&lost, EPOCH_S),
            Err(ChannelError::InvalidPayload(_))
        ));
        assert_eq!(df.status().state, SweepState::NoHeading);
        assert_eq!(df.mark(EPOCH_S, FREQ_HZ), Err(MarkRefusal::NoPosition));
    }
}
