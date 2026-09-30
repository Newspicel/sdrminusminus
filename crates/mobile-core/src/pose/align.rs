use sdrmm_wire::geo::wrap_180;

use crate::records::{AlignHint, AlignState};

pub(crate) const ALIGN_NEED: usize = 20;
pub(crate) const ALIGN_MIN_SPAN_MS: i64 = 15_000;
pub(crate) const ALIGN_TIMEOUT_MS: i64 = 120_000;
pub(crate) const ALIGN_MIN_SPEED_MPS: f64 = 5.5;
pub(crate) const ALIGN_MAX_TURN_DEG_S: f64 = 3.0;
pub(crate) const ALIGN_MAX_COURSE_SIGMA_DEG: f64 = 5.0;
pub(crate) const ALIGN_MAX_COMPASS_DEG: f64 = 20.0;
pub(crate) const ALIGN_MAX_SKEW_MS: i64 = 500;
pub(crate) const ALIGN_MAX_SPREAD_DEG: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Miss {
    TooSlow,
    Turning,
    NoCompass,
    NoFix,
}

const MISSES: [Miss; 4] = [Miss::TooSlow, Miss::Turning, Miss::NoCompass, Miss::NoFix];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignFailure {
    Miss(Miss),
    NotSteady,
    TimedOut,
}

impl AlignFailure {
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Miss(Miss::TooSlow) => "Too slow",
            Self::Miss(Miss::Turning) => "Turning",
            Self::Miss(Miss::NoCompass) => "No compass",
            Self::Miss(Miss::NoFix) => "No fix",
            Self::NotSteady => "Unsteady",
            Self::TimedOut => "Timed out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AlignOutcome {
    Done { offset_deg: f64, spread_deg: f64 },
    Failed(AlignFailure),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Compass {
    pub(crate) t_ms: i64,
    pub(crate) heading_deg: f64,
    pub(crate) accuracy_deg: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AlignInput {
    pub(crate) t_ms: i64,
    pub(crate) speed_mps: Option<f64>,
    pub(crate) turn_rate_deg_s: f64,
    pub(crate) course_deg: Option<f64>,
    pub(crate) course_sigma_deg: f64,
    pub(crate) compass: Option<Compass>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AlignRoutine {
    started_ms: i64,
    samples: Vec<(i64, f64)>,
    misses: [u32; 4],
    last_miss: Option<Miss>,
}

impl AlignRoutine {
    pub(crate) fn new(now_ms: i64) -> Self {
        Self {
            started_ms: now_ms,
            samples: Vec::with_capacity(ALIGN_NEED),
            misses: [0; 4],
            last_miss: None,
        }
    }

    pub(crate) fn offer(&mut self, input: AlignInput) -> Option<AlignOutcome> {
        match qualify(input) {
            Ok(difference) => {
                self.last_miss = None;
                self.samples.push((input.t_ms, difference));
            }
            Err(miss) => {
                self.last_miss = Some(miss);
                if let Some(slot) = MISSES.iter().position(|known| *known == miss) {
                    self.misses[slot] = self.misses[slot].saturating_add(1);
                }
            }
        }
        self.finished(input.t_ms)
    }

    pub(crate) fn check(&self, now_ms: i64) -> Option<AlignOutcome> {
        (now_ms - self.started_ms >= ALIGN_TIMEOUT_MS)
            .then(|| AlignOutcome::Failed(self.main_miss()))
    }

    pub(crate) fn state(&self) -> AlignState {
        AlignState::Collecting {
            progress: (self.samples.len() as f32 / ALIGN_NEED as f32).min(1.0),
            hint: match self.last_miss {
                Some(Miss::TooSlow) => AlignHint::DriveFaster,
                Some(Miss::Turning) => AlignHint::DriveStraight,
                Some(Miss::NoCompass | Miss::NoFix) | None => AlignHint::Hold,
            },
        }
    }

    fn finished(&self, now_ms: i64) -> Option<AlignOutcome> {
        let span = match (self.samples.first(), self.samples.last()) {
            (Some((first, _)), Some((last, _))) => last - first,
            _ => 0,
        };
        if self.samples.len() >= ALIGN_NEED && span >= ALIGN_MIN_SPAN_MS {
            return Some(solve(&self.samples));
        }
        self.check(now_ms)
    }

    fn main_miss(&self) -> AlignFailure {
        MISSES
            .iter()
            .zip(self.misses)
            .filter(|(_, count)| *count > 0)
            .max_by_key(|(_, count)| *count)
            .map_or(AlignFailure::TimedOut, |(miss, _)| {
                AlignFailure::Miss(*miss)
            })
    }
}

fn qualify(input: AlignInput) -> Result<f64, Miss> {
    if input
        .speed_mps
        .is_none_or(|speed| speed < ALIGN_MIN_SPEED_MPS)
    {
        return Err(Miss::TooSlow);
    }
    if input.turn_rate_deg_s > ALIGN_MAX_TURN_DEG_S {
        return Err(Miss::Turning);
    }
    let course = input
        .course_deg
        .filter(|_| input.course_sigma_deg <= ALIGN_MAX_COURSE_SIGMA_DEG)
        .ok_or(Miss::NoFix)?;
    let compass = input
        .compass
        .filter(|compass| {
            compass.accuracy_deg <= ALIGN_MAX_COMPASS_DEG
                && (compass.t_ms - input.t_ms).abs() <= ALIGN_MAX_SKEW_MS
        })
        .ok_or(Miss::NoCompass)?;
    Ok(wrap_180(compass.heading_deg - course))
}

fn solve(samples: &[(i64, f64)]) -> AlignOutcome {
    let count = samples.len() as f64;
    let (sin, cos) = samples.iter().fold((0.0, 0.0), |(sin, cos), (_, deg)| {
        let rad = deg.to_radians();
        (sin + rad.sin(), cos + rad.cos())
    });
    let (sin, cos) = (sin / count, cos / count);
    let length = sin.hypot(cos).clamp(f64::MIN_POSITIVE, 1.0);
    let spread_deg = (-2.0 * length.ln()).sqrt().to_degrees();
    let offset_deg = (sin.atan2(cos).to_degrees() * 10.0).round() / 10.0 + 0.0;
    if spread_deg <= ALIGN_MAX_SPREAD_DEG {
        AlignOutcome::Done {
            offset_deg,
            spread_deg,
        }
    } else {
        AlignOutcome::Failed(AlignFailure::NotSteady)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(t_ms: i64, course: f64, compass: f64) -> AlignInput {
        AlignInput {
            t_ms,
            speed_mps: Some(12.0),
            turn_rate_deg_s: 0.5,
            course_deg: Some(course),
            course_sigma_deg: 2.0,
            compass: Some(Compass {
                t_ms: t_ms - 100,
                heading_deg: compass,
                accuracy_deg: 10.0,
            }),
        }
    }

    fn run(
        routine: &mut AlignRoutine,
        inputs: impl Iterator<Item = AlignInput>,
    ) -> Option<AlignOutcome> {
        inputs.into_iter().find_map(|input| routine.offer(input))
    }

    #[test]
    fn straight_driving_yields_the_offset() {
        let mut routine = AlignRoutine::new(0);
        let noise = [2.0, -1.5, 0.5, -2.0, 1.0, -0.5, 1.5, -1.0];
        let outcome = run(
            &mut routine,
            (0..40).map(|index| {
                let course = 350.0 + f64::from(index) * 0.2;
                drive(
                    i64::from(index) * 1_000,
                    course,
                    course + 7.0 + noise[index as usize % 8],
                )
            }),
        );
        match outcome {
            Some(AlignOutcome::Done {
                offset_deg,
                spread_deg,
            }) => {
                assert!((offset_deg - 7.0).abs() <= 0.5, "{offset_deg}");
                assert!(spread_deg < 3.0);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            routine.state(),
            AlignState::Collecting { progress, .. } if progress >= 1.0
        ));
    }

    #[test]
    fn turning_or_slow_samples_are_not_collected() {
        let mut routine = AlignRoutine::new(0);
        let mut slow = drive(0, 90.0, 97.0);
        slow.speed_mps = Some(3.0);
        assert_eq!(routine.offer(slow), None);
        assert!(
            matches!(routine.state(), AlignState::Collecting { progress, hint: AlignHint::DriveFaster } if progress == 0.0)
        );
        let mut turning = drive(1_000, 90.0, 97.0);
        turning.turn_rate_deg_s = 8.0;
        assert_eq!(routine.offer(turning), None);
        assert!(matches!(
            routine.state(),
            AlignState::Collecting {
                hint: AlignHint::DriveStraight,
                ..
            }
        ));
        let mut stale = drive(2_000, 90.0, 97.0);
        stale.compass = stale.compass.map(|compass| Compass { t_ms: 0, ..compass });
        assert_eq!(routine.offer(stale), None);
        assert!(matches!(
            routine.state(),
            AlignState::Collecting {
                hint: AlignHint::Hold,
                ..
            }
        ));
        assert_eq!(routine.offer(drive(3_000, 90.0, 97.0)), None);
        assert!(
            matches!(routine.state(), AlignState::Collecting { progress, .. } if (progress - 0.05).abs() < 1e-6)
        );
    }

    #[test]
    fn a_scattered_offset_fails_as_not_steady() {
        let mut routine = AlignRoutine::new(0);
        let outcome = run(
            &mut routine,
            (0..30).map(|index| {
                let scatter = if index % 2 == 0 { 25.0 } else { -25.0 };
                drive(i64::from(index) * 1_000, 180.0, 180.0 + scatter)
            }),
        );
        assert_eq!(outcome, Some(AlignOutcome::Failed(AlignFailure::NotSteady)));
    }

    #[test]
    fn it_times_out_after_120_s_with_the_main_reason() {
        let mut routine = AlignRoutine::new(0);
        let mut outcome = None;
        for index in 0..130 {
            let mut input = drive(i64::from(index) * 1_000, 90.0, 97.0);
            input.speed_mps = Some(if index % 3 == 0 { 20.0 } else { 2.0 });
            input.turn_rate_deg_s = if index % 3 == 0 { 10.0 } else { 0.0 };
            outcome = routine.offer(input);
            if outcome.is_some() {
                break;
            }
        }
        assert_eq!(
            outcome,
            Some(AlignOutcome::Failed(AlignFailure::Miss(Miss::TooSlow)))
        );
        assert_eq!(
            AlignRoutine::new(0).check(ALIGN_TIMEOUT_MS),
            Some(AlignOutcome::Failed(AlignFailure::TimedOut))
        );
        assert_eq!(AlignRoutine::new(0).check(ALIGN_TIMEOUT_MS - 1), None);
        assert_eq!(AlignFailure::NotSteady.text(), "Unsteady");
    }
}
