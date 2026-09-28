use sdrmm_wire::geo::{self, wrap_180, wrap_360};

use super::{
    align::{AlignInput, AlignOutcome, AlignRoutine, Compass},
    axis,
    filter::{HeadingFilter, Update},
    publish::{HeadingOut, PoseOut, PublishPolicy, build_fix},
    vector::{Quat, Vec3, heading_of},
};
use crate::records::{
    AlignState, HeadingMode, HeadingSample, HeadingSourceKind, LatLon, LocationSample, MagAccuracy,
    MotionFrame, MotionSample, Mount, Notice, PoseSettings, PoseView,
};

pub(crate) const COURSE_MIN_SPEED_MPS: f64 = 3.0;
pub(crate) const COURSE_HOLD_MS: i64 = 3_000;
pub(crate) const STATIONARY_MAX_MPS: f64 = 0.5;
pub(crate) const STATIONARY_HOLD_MS: i64 = 2_000;
pub(crate) const FIX_LOST_MS: i64 = 5_000;
pub(crate) const STALE_FIX_MS: i64 = 10_000;
pub(crate) const TURN_TAU_S: f64 = 0.5;
pub(crate) const TURN_RATE_MAX_DEG_S: f64 = 15.0;
pub(crate) const COMPASS_MAX_HANDHELD_DEG: f64 = 25.0;
pub(crate) const COMPASS_MAX_VEHICLE_DEG: f64 = 15.0;
pub(crate) const COMPASS_TAKEOVER_SIGMA_DEG: f64 = 15.0;
pub(crate) const MAX_GYRO_DT_MS: i64 = 500;
pub(crate) const GAP_SIGMA_DEG: f64 = 30.0;
pub(crate) const MAX_PUBLISH_SIGMA_DEG: f64 = 45.0;
pub(crate) const FRESH_MS: i64 = 1_000;
pub(crate) const NOTICE_EVERY_MS: i64 = 60_000;
pub(crate) const ATTITUDE_STREAK: u32 = 10;
pub(crate) const MIN_UP_Z: f64 = 0.95;
pub(crate) const DERIVE_MIN_MS: i64 = 500;
pub(crate) const DERIVE_MAX_MS: i64 = 5_000;
pub(crate) const DERIVE_MAX_ACC_M: f64 = 20.0;
pub(crate) const NO_FIX: &str = "no GPS fix";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Movement {
    Moving,
    Stationary,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct MovementTracker {
    state: Movement,
    fast_since: Option<i64>,
    slow_since: Option<i64>,
    last_fix_ms: Option<i64>,
}

impl MovementTracker {
    fn update(&mut self, t_ms: i64, speed: Option<f64>) {
        self.last_fix_ms = Some(t_ms);
        match speed {
            Some(speed) if speed >= COURSE_MIN_SPEED_MPS => {
                self.slow_since = None;
                let since = *self.fast_since.get_or_insert(t_ms);
                self.state = if t_ms - since >= COURSE_HOLD_MS {
                    Movement::Moving
                } else {
                    Movement::Unknown
                };
            }
            Some(speed) if speed < STATIONARY_MAX_MPS => {
                self.fast_since = None;
                let since = *self.slow_since.get_or_insert(t_ms);
                self.state = if t_ms - since >= STATIONARY_HOLD_MS {
                    Movement::Stationary
                } else {
                    Movement::Unknown
                };
            }
            _ => self.forget(),
        }
    }

    fn expire(&mut self, now_ms: i64) {
        if self.last_fix_ms.is_none_or(|t| now_ms - t > FIX_LOST_MS) {
            self.forget();
        }
    }

    fn forget(&mut self) {
        self.state = Movement::Unknown;
        self.fast_since = None;
        self.slow_since = None;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counters {
    pub(crate) invalid_samples: u64,
    pub(crate) gyro_gaps: u64,
    pub(crate) compass_rejected: u64,
    pub(crate) course_rejected: u64,
    pub(crate) heading_resets: u64,
    pub(crate) attitude_mismatch: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Topic {
    NoDeclination,
    NoAccuracy,
    Reset,
    Invalid,
}

const TOPICS: [Topic; 4] = [
    Topic::NoDeclination,
    Topic::NoAccuracy,
    Topic::Reset,
    Topic::Invalid,
];

impl Topic {
    const fn text(self) -> &'static str {
        match self {
            Self::NoDeclination => "No declination",
            Self::NoAccuracy => "No compass accuracy",
            Self::Reset => "Heading reset",
            Self::Invalid => "Bad sensor data",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseStep {
    pub publish: Option<PoseOut>,
    pub notices: Vec<Notice>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PoseSnapshot {
    pub(crate) at: LatLon,
    pub(crate) heading_deg: Option<f64>,
    pub(crate) t_ms: i64,
}

#[derive(Clone, Debug)]
pub struct PoseEngine {
    settings: PoseSettings,
    filter: HeadingFilter,
    filter_ms: Option<i64>,
    source: HeadingSourceKind,
    compass_floor_deg: Option<f64>,
    movement: MovementTracker,
    fix: Option<LocationSample>,
    clock_ms: i64,
    motion_ms: Option<i64>,
    rate_cw: f64,
    turn_rate: f64,
    up: Option<Vec3>,
    declination: Option<f64>,
    heading_sample: Option<HeadingSample>,
    attitude_ms: Option<i64>,
    compass: Option<Compass>,
    attitude_streak: u32,
    align: Option<AlignRoutine>,
    align_state: AlignState,
    policy: PublishPolicy,
    needed: bool,
    online: bool,
    counters: Counters,
    told: [Option<i64>; 4],
}

impl PoseEngine {
    #[must_use]
    pub fn new(settings: PoseSettings) -> Self {
        Self {
            settings: sanitize(settings),
            filter: HeadingFilter::default(),
            filter_ms: None,
            source: HeadingSourceKind::None,
            compass_floor_deg: None,
            movement: MovementTracker::default(),
            fix: None,
            clock_ms: 0,
            motion_ms: None,
            rate_cw: 0.0,
            turn_rate: 0.0,
            up: None,
            declination: None,
            heading_sample: None,
            attitude_ms: None,
            compass: None,
            attitude_streak: 0,
            align: None,
            align_state: AlignState::Idle,
            policy: PublishPolicy::default(),
            needed: false,
            online: false,
            counters: Counters::default(),
            told: [None; 4],
        }
    }

    pub fn location(&mut self, sample: LocationSample) -> PoseStep {
        let mut step = PoseStep::default();
        let Some(sample) = clean_location(sample) else {
            self.invalid(sample.t_unix_ms, &mut step);
            return step;
        };
        let t = sample.t_unix_ms;
        self.clock(t);
        let speed = sample.speed_mps.or_else(|| self.derived_speed(&sample));
        self.movement.update(t, speed);
        self.fix = Some(sample);
        self.offer_align(&sample, speed, &mut step);
        if self.settings.heading_mode != HeadingMode::Compass {
            self.course(&sample, speed, &mut step);
        }
        self.finish(step)
    }

    pub fn heading(&mut self, sample: HeadingSample) -> PoseStep {
        let mut step = PoseStep::default();
        let Some(sample) = clean_heading(sample) else {
            self.invalid(sample.t_unix_ms, &mut step);
            return step;
        };
        let t = sample.t_unix_ms;
        self.clock(t);
        if let Some(true_deg) = sample.true_deg {
            self.declination = Some(wrap_180(true_deg - sample.magnetic_deg));
        }
        self.heading_sample = Some(sample);
        let attitude_fresh = self
            .attitude_ms
            .is_some_and(|at| (t - at).abs() <= FRESH_MS);
        if !attitude_fresh && self.settings.mount == Mount::Flat {
            let heading = sample.true_deg.or_else(|| {
                self.declination
                    .map(|decl| wrap_360(sample.magnetic_deg + decl))
            });
            match (heading, sample.accuracy_deg) {
                (Some(heading), Some(accuracy)) => {
                    self.compass_measure(t, heading, accuracy, &mut step)
                }
                (None, _) => self.reject_compass(t, Topic::NoDeclination, &mut step),
                (Some(_), None) => self.reject_compass(t, Topic::NoAccuracy, &mut step),
            }
        }
        self.finish(step)
    }

    pub fn motion(&mut self, sample: MotionSample) -> PoseStep {
        let mut step = PoseStep::default();
        let t = sample.t_unix_ms;
        let Some((attitude, gyro, up)) = clean_motion(&sample) else {
            self.invalid(t, &mut step);
            return step;
        };
        if self.motion_ms.is_some_and(|last| t <= last) {
            self.invalid(t, &mut step);
            return step;
        }
        self.clock(t);
        let rate_cw = -gyro.dot(up).to_degrees();
        match self.motion_ms {
            Some(last) if t - last <= MAX_GYRO_DT_MS => {
                let dt = (t - last) as f64 / 1_000.0;
                self.filter.predict_gyro(rate_cw, dt);
                let alpha = 1.0 - (-dt / TURN_TAU_S).exp();
                self.turn_rate = alpha.mul_add(rate_cw.abs() - self.turn_rate, self.turn_rate);
            }
            Some(_) => {
                self.filter.inflate(GAP_SIGMA_DEG);
                self.counters.gyro_gaps += 1;
                self.advance(t);
            }
            None => self.advance(t),
        }
        self.motion_ms = Some(t);
        self.filter_ms = Some(t);
        self.rate_cw = rate_cw;
        self.up = Some(up);
        self.attitude(t, attitude, &sample, up, &mut step);
        self.finish(step)
    }

    pub fn settings(&mut self, settings: PoseSettings) -> PoseStep {
        let settings = sanitize(settings);
        let old = self.settings;
        if settings.mount != old.mount {
            self.filter = HeadingFilter::default();
            self.source = HeadingSourceKind::None;
        } else {
            self.filter
                .shift(settings.mount_offset_deg - old.mount_offset_deg);
        }
        if !settings.share_pose {
            self.policy.forget();
        }
        self.settings = settings;
        self.finish(PoseStep::default())
    }

    pub fn start_align(&mut self, now_ms: i64) -> PoseStep {
        self.clock(now_ms);
        let routine = AlignRoutine::new(self.clock_ms);
        self.align_state = routine.state();
        self.align = Some(routine);
        PoseStep::default()
    }

    pub fn cancel_align(&mut self) -> PoseStep {
        self.align = None;
        self.align_state = AlignState::Idle;
        PoseStep::default()
    }

    pub fn demand(&mut self, needed: bool, online: bool, now_ms: i64) -> PoseStep {
        self.needed = needed;
        self.online = online;
        if !needed {
            self.policy.forget();
        }
        self.clock(now_ms);
        self.finish(PoseStep::default())
    }

    pub fn tick(&mut self, now_ms: i64) -> PoseStep {
        self.clock(now_ms);
        let now = self.clock_ms;
        self.movement.expire(now);
        if !self.gyro_fresh(now) {
            self.advance(now);
        }
        if let Some(AlignOutcome::Failed(failure)) =
            self.align.as_ref().and_then(|routine| routine.check(now))
        {
            self.align = None;
            self.align_state = AlignState::Failed {
                reason: failure.text().to_owned(),
            };
        }
        self.finish(PoseStep::default())
    }

    #[must_use]
    pub fn view(&self, now_ms: i64) -> PoseView {
        let heading = self.heading_out(now_ms);
        PoseView {
            heading_deg: heading.map(|heading| heading.deg),
            accuracy_deg: heading.map(|heading| heading.sigma_deg),
            source: heading.map_or(HeadingSourceKind::None, |heading| heading.source),
            align: self.align_state.clone(),
            sending: self.sharing() && self.online,
            fix_age_ms: self
                .fix
                .map(|fix| u64::try_from((now_ms - fix.t_unix_ms).max(0)).unwrap_or(0)),
        }
    }

    pub(crate) fn snapshot(&self, now_ms: i64) -> Option<PoseSnapshot> {
        let fix = self.fix?;
        (now_ms - fix.t_unix_ms <= STALE_FIX_MS).then(|| PoseSnapshot {
            at: LatLon {
                lat: fix.lat,
                lon: fix.lon,
            },
            heading_deg: self.heading_out(now_ms).map(|heading| heading.deg),
            t_ms: fix.t_unix_ms,
        })
    }

    pub(crate) fn sharing(&self) -> bool {
        self.settings.share_pose && self.needed
    }

    #[cfg(test)]
    pub(crate) fn counters(&self) -> Counters {
        self.counters
    }

    #[cfg(test)]
    pub(crate) fn movement(&self) -> Movement {
        self.movement.state
    }

    fn clock(&mut self, t_ms: i64) {
        self.clock_ms = self.clock_ms.max(t_ms);
    }

    fn gyro_fresh(&self, now_ms: i64) -> bool {
        self.motion_ms.is_some_and(|at| now_ms - at <= FRESH_MS)
    }

    fn advance(&mut self, t_ms: i64) {
        if self.gyro_fresh(t_ms) {
            return;
        }
        if let Some(from) = self.filter_ms
            && t_ms > from
        {
            self.filter.predict_blind((t_ms - from) as f64 / 1_000.0);
        }
        self.filter_ms = Some(self.filter_ms.map_or(t_ms, |from| from.max(t_ms)));
    }

    fn tell(&mut self, t_ms: i64, topic: Topic, step: &mut PoseStep) {
        let Some(slot) = TOPICS.iter().position(|known| *known == topic) else {
            return;
        };
        if self.told[slot].is_none_or(|at| t_ms - at >= NOTICE_EVERY_MS) {
            self.told[slot] = Some(t_ms);
            step.notices.push(Notice::warn(topic.text()));
        }
    }

    fn invalid(&mut self, t_ms: i64, step: &mut PoseStep) {
        self.counters.invalid_samples += 1;
        self.tell(t_ms, Topic::Invalid, step);
    }

    fn reject_compass(&mut self, t_ms: i64, topic: Topic, step: &mut PoseStep) {
        self.counters.compass_rejected += 1;
        self.tell(t_ms, topic, step);
    }

    fn derived_speed(&self, sample: &LocationSample) -> Option<f64> {
        let previous = self.fix?;
        let dt_ms = sample.t_unix_ms - previous.t_unix_ms;
        let accurate = previous.h_acc_m <= DERIVE_MAX_ACC_M && sample.h_acc_m <= DERIVE_MAX_ACC_M;
        ((DERIVE_MIN_MS..=DERIVE_MAX_MS).contains(&dt_ms) && accurate).then(|| {
            let from = geo::LatLon {
                lat: previous.lat,
                lon: previous.lon,
            };
            let to = geo::LatLon {
                lat: sample.lat,
                lon: sample.lon,
            };
            geo::distance_m(from, to) / (dt_ms as f64 / 1_000.0)
        })
    }

    fn course(&mut self, sample: &LocationSample, speed: Option<f64>, step: &mut PoseStep) {
        let Some(course) = sample.course_deg else {
            return;
        };
        if self.movement.state != Movement::Moving || self.turn_rate > TURN_RATE_MAX_DEG_S {
            return;
        }
        let sigma = course_sigma(sample, speed);
        self.advance(sample.t_unix_ms);
        let z = wrap_360(course + self.settings.mount_offset_deg);
        let update = self.filter.update(z, sigma, true);
        self.after_update(sample.t_unix_ms, update, HeadingSourceKind::Course, step);
    }

    fn attitude(
        &mut self,
        t_ms: i64,
        attitude: Quat,
        sample: &MotionSample,
        up: Vec3,
        step: &mut PoseStep,
    ) {
        if sample.frame == MotionFrame::Arbitrary {
            return;
        }
        if attitude.rotate(up).z < MIN_UP_Z {
            self.attitude_streak += 1;
            self.counters.attitude_mismatch += 1;
            if self.attitude_streak == ATTITUDE_STREAK {
                step.notices.push(Notice::error("Phone attitude inverted"));
            }
            return;
        }
        self.attitude_streak = 0;
        let forward = attitude.rotate(axis::forward(self.settings.mount));
        let (east, north) = match sample.frame {
            MotionFrame::TrueNorth => (-forward.y, forward.x),
            MotionFrame::EnuMagnetic | MotionFrame::Arbitrary => (forward.x, forward.y),
        };
        let Some(axis_deg) = heading_of(east, north) else {
            self.counters.compass_rejected += 1;
            return;
        };
        let heading = match sample.frame {
            MotionFrame::EnuMagnetic => match self.declination {
                Some(declination) => wrap_360(axis_deg + declination),
                None => return self.reject_compass(t_ms, Topic::NoDeclination, step),
            },
            MotionFrame::TrueNorth | MotionFrame::Arbitrary => axis_deg,
        };
        let Some(accuracy) = self.compass_accuracy(t_ms, sample.mag_accuracy) else {
            return self.reject_compass(t_ms, Topic::NoAccuracy, step);
        };
        self.attitude_ms = Some(t_ms);
        self.compass_measure(t_ms, heading, accuracy, step);
    }

    fn compass_accuracy(&self, t_ms: i64, magnetic: MagAccuracy) -> Option<f64> {
        self.heading_sample
            .filter(|sample| (t_ms - sample.t_unix_ms).abs() <= FRESH_MS)
            .and_then(|sample| sample.accuracy_deg)
            .or(match magnetic {
                MagAccuracy::High => Some(10.0),
                MagAccuracy::Medium => Some(20.0),
                MagAccuracy::Low => Some(45.0),
                MagAccuracy::Uncalibrated => None,
            })
    }

    fn compass_measure(&mut self, t_ms: i64, heading: f64, accuracy: f64, step: &mut PoseStep) {
        self.compass = Some(Compass {
            t_ms,
            heading_deg: heading,
            accuracy_deg: accuracy,
        });
        let uncertain = self
            .filter
            .heading()
            .is_none_or(|(_, sigma)| sigma > COMPASS_TAKEOVER_SIGMA_DEG);
        let compass_led = self.source == HeadingSourceKind::Compass;
        let accept = match self.settings.heading_mode {
            HeadingMode::Course => false,
            HeadingMode::Compass => accuracy <= COMPASS_MAX_HANDHELD_DEG,
            HeadingMode::Auto => {
                accuracy <= COMPASS_MAX_VEHICLE_DEG
                    && (!self.filter.valid()
                        || (self.movement.state != Movement::Moving && (uncertain || compass_led)))
            }
        };
        if !accept {
            return;
        }
        self.advance(t_ms);
        let update = self.filter.update(heading, accuracy, false);
        if matches!(
            update,
            Update::Accepted | Update::Initialised | Update::Reset
        ) {
            self.compass_floor_deg = Some(accuracy);
        }
        self.after_update(t_ms, update, HeadingSourceKind::Compass, step);
    }

    fn after_update(
        &mut self,
        t_ms: i64,
        update: Update,
        source: HeadingSourceKind,
        step: &mut PoseStep,
    ) {
        match update {
            Update::Accepted | Update::Initialised => self.source = source,
            Update::Rejected if source == HeadingSourceKind::Course => {
                self.counters.course_rejected += 1;
            }
            Update::Rejected => self.counters.compass_rejected += 1,
            Update::Reset => {
                self.source = source;
                self.counters.heading_resets += 1;
                self.tell(t_ms, Topic::Reset, step);
            }
            Update::Ignored => {}
        }
    }

    fn offer_align(&mut self, sample: &LocationSample, speed: Option<f64>, step: &mut PoseStep) {
        let Some(routine) = self.align.as_mut() else {
            return;
        };
        let outcome = routine.offer(AlignInput {
            t_ms: sample.t_unix_ms,
            speed_mps: speed,
            turn_rate_deg_s: self.turn_rate,
            course_deg: sample.course_deg,
            course_sigma_deg: course_sigma(sample, speed),
            compass: self.compass,
        });
        match outcome {
            None => self.align_state = routine.state(),
            Some(AlignOutcome::Done { offset_deg, .. }) => {
                self.align = None;
                self.align_state = AlignState::Done { offset_deg };
                let settings = PoseSettings {
                    mount_offset_deg: offset_deg,
                    ..self.settings
                };
                step.notices.extend(self.settings(settings).notices);
            }
            Some(AlignOutcome::Failed(failure)) => {
                self.align = None;
                self.align_state = AlignState::Failed {
                    reason: failure.text().to_owned(),
                };
            }
        }
    }

    fn heading_out(&self, now_ms: i64) -> Option<HeadingOut> {
        let (psi, filtered) = self.filter.heading()?;
        let sigma = match (self.source, self.compass_floor_deg) {
            (HeadingSourceKind::Compass, Some(floor)) => filtered.max(floor),
            _ => filtered,
        };
        if sigma > MAX_PUBLISH_SIGMA_DEG {
            return None;
        }
        let gyro = self.gyro_fresh(now_ms);
        Some(HeadingOut {
            deg: wrap_360(psi - self.settings.mount_offset_deg),
            sigma_deg: sigma,
            source: if gyro {
                HeadingSourceKind::Fused
            } else {
                self.source
            },
            yaw_rate_dps: gyro.then_some(self.rate_cw),
        })
    }

    fn pitch_roll(&self) -> Option<(f64, f64)> {
        if self.settings.heading_mode != HeadingMode::Compass {
            return None;
        }
        self.up
            .map(|up| axis::pitch_roll(axis::forward(self.settings.mount), up))
    }

    fn finish(&mut self, mut step: PoseStep) -> PoseStep {
        step.publish = self.publish(self.clock_ms);
        step
    }

    fn publish(&mut self, now_ms: i64) -> Option<PoseOut> {
        if !self.sharing() {
            return None;
        }
        match self.fix {
            Some(fix) if now_ms - fix.t_unix_ms <= STALE_FIX_MS => {
                let heading = self.heading_out(now_ms);
                if !self
                    .policy
                    .due(now_ms, fix.t_unix_ms, heading.map(|heading| heading.deg))
                {
                    return None;
                }
                match build_fix(&fix, now_ms, heading, self.pitch_roll()) {
                    Ok(out) => Some(PoseOut {
                        fix: Some(out),
                        error: None,
                    }),
                    Err(problem) => {
                        tracing::error!(problem, "pose fix refused by its own check");
                        self.counters.invalid_samples += 1;
                        None
                    }
                }
            }
            _ => self.policy.error_due(now_ms, NO_FIX).then(|| PoseOut {
                fix: None,
                error: Some(NO_FIX.to_owned()),
            }),
        }
    }
}

fn sanitize(settings: PoseSettings) -> PoseSettings {
    PoseSettings {
        mount_offset_deg: if settings.mount_offset_deg.is_finite() {
            wrap_180(settings.mount_offset_deg)
        } else {
            0.0
        },
        ..settings
    }
}

fn course_sigma(sample: &LocationSample, speed: Option<f64>) -> f64 {
    sample
        .course_acc_deg
        .or_else(|| speed.filter(|speed| *speed > 0.0).map(|speed| 20.0 / speed))
        .unwrap_or(30.0)
        .clamp(1.0, 30.0)
}

fn finite_non_negative(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite() && *value >= 0.0)
}

fn clean_location(sample: LocationSample) -> Option<LocationSample> {
    let valid = sample.lat.is_finite()
        && sample.lon.is_finite()
        && (-90.0..=90.0).contains(&sample.lat)
        && (-180.0..=180.0).contains(&sample.lon)
        && sample.h_acc_m.is_finite()
        && sample.h_acc_m >= 0.0;
    valid.then(|| LocationSample {
        alt_m: sample.alt_m.filter(|alt| alt.is_finite()),
        v_acc_m: finite_non_negative(sample.v_acc_m),
        speed_mps: finite_non_negative(sample.speed_mps),
        speed_acc_mps: finite_non_negative(sample.speed_acc_mps),
        course_deg: finite_non_negative(sample.course_deg)
            .filter(|course| *course <= 360.0)
            .map(wrap_360),
        course_acc_deg: finite_non_negative(sample.course_acc_deg),
        ..sample
    })
}

fn clean_heading(sample: HeadingSample) -> Option<HeadingSample> {
    sample.magnetic_deg.is_finite().then(|| HeadingSample {
        magnetic_deg: wrap_360(sample.magnetic_deg),
        true_deg: sample.true_deg.filter(|deg| deg.is_finite()).map(wrap_360),
        accuracy_deg: finite_non_negative(sample.accuracy_deg),
        ..sample
    })
}

fn clean_motion(sample: &MotionSample) -> Option<(Quat, Vec3, Vec3)> {
    let attitude = Quat {
        w: sample.qw,
        x: sample.qx,
        y: sample.qy,
        z: sample.qz,
    }
    .unit()?;
    let gyro = Vec3::new(sample.rot_x, sample.rot_y, sample.rot_z);
    let up = Vec3::new(-sample.grav_x, -sample.grav_y, -sample.grav_z).unit()?;
    gyro.finite().then_some((attitude, gyro, up))
}

#[cfg(test)]
mod tests;
