use sdrmm_wire::{
    geo::wrap_180,
    phone::POSE_KEEPALIVE_MS,
    position::{Attitude, HeadingSource, MAX_YAW_RATE_DPS, PositionFix},
    ws::ClientCommand,
};

use super::PUBLISH_GAP_MS;
use crate::records::{HeadingSourceKind, LocationSample};

pub(crate) const MIN_INTERVAL_MS: i64 = PUBLISH_GAP_MS as i64;
pub(crate) const KEEPALIVE_MS: i64 = POSE_KEEPALIVE_MS as i64;
pub(crate) const HEADING_STEP_DEG: f64 = 1.0;
pub(crate) const TRACK_MIN_SPEED_MPS: f64 = 0.5;

#[derive(Clone, Debug, PartialEq)]
pub struct PoseOut {
    pub fix: Option<PositionFix>,
    pub error: Option<String>,
}

impl PoseOut {
    pub(crate) fn command(&self) -> ClientCommand {
        ClientCommand::PublishPose {
            fix: self.fix.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PublishPolicy {
    last_ms: Option<i64>,
    last_heading: Option<f64>,
    last_fix_ms: Option<i64>,
    last_error: Option<String>,
}

impl PublishPolicy {
    pub(crate) fn due(&mut self, now_ms: i64, fix_ms: i64, heading: Option<f64>) -> bool {
        if self
            .last_ms
            .is_some_and(|last| now_ms - last < MIN_INTERVAL_MS)
        {
            return false;
        }
        let new_fix = self.last_fix_ms != Some(fix_ms);
        let turned = match (heading, self.last_heading) {
            (Some(now), Some(then)) => wrap_180(now - then).abs() >= HEADING_STEP_DEG,
            (None, None) => false,
            _ => true,
        };
        let heartbeat = self
            .last_ms
            .is_none_or(|last| now_ms - last >= KEEPALIVE_MS);
        let due = new_fix || turned || heartbeat || self.last_error.is_some();
        if due {
            self.last_ms = Some(now_ms);
            self.last_heading = heading;
            self.last_fix_ms = Some(fix_ms);
            self.last_error = None;
        }
        due
    }

    pub(crate) fn error_due(&mut self, now_ms: i64, error: &str) -> bool {
        let changed = self.last_error.as_deref() != Some(error);
        let heartbeat = self
            .last_ms
            .is_none_or(|last| now_ms - last >= KEEPALIVE_MS);
        let spaced = self
            .last_ms
            .is_none_or(|last| now_ms - last >= MIN_INTERVAL_MS);
        let due = spaced && (changed || heartbeat);
        if due {
            self.last_ms = Some(now_ms);
            self.last_error = Some(error.to_owned());
            self.last_fix_ms = None;
            self.last_heading = None;
        }
        due
    }

    pub(crate) fn forget(&mut self) {
        *self = Self::default();
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HeadingOut {
    pub(crate) deg: f64,
    pub(crate) sigma_deg: f64,
    pub(crate) source: HeadingSourceKind,
    pub(crate) yaw_rate_dps: Option<f64>,
}

pub(crate) fn build_fix(
    fix: &LocationSample,
    now_ms: i64,
    heading: Option<HeadingOut>,
    pitch_roll: Option<(f64, f64)>,
) -> Result<PositionFix, &'static str> {
    let time = jiff::Timestamp::from_millisecond(now_ms)
        .map_err(|_| "publish time out of range")?
        .to_string();
    let age_s = ((now_ms - fix.t_unix_ms).max(0) as f64) / 1_000.0;
    let accuracy_m = fix
        .speed_mps
        .map_or(fix.h_acc_m, |speed| speed.mul_add(age_s, fix.h_acc_m));
    let track_deg = fix.course_deg.filter(|_| {
        fix.speed_mps
            .is_some_and(|speed| speed >= TRACK_MIN_SPEED_MPS)
    });
    let attitude = heading.map_or_else(Attitude::default, |heading| Attitude {
        heading_deg: Some(heading.deg),
        heading_accuracy_deg: Some(heading.sigma_deg.min(180.0)),
        heading_source: wire_source(heading.source),
        pitch_deg: pitch_roll.map(|(pitch, _)| pitch),
        roll_deg: pitch_roll.map(|(_, roll)| roll),
        yaw_rate_dps: heading
            .yaw_rate_dps
            .map(|rate| rate.clamp(-MAX_YAW_RATE_DPS, MAX_YAW_RATE_DPS)),
    });
    let out = PositionFix {
        latitude: fix.lat,
        longitude: fix.lon,
        altitude_m: fix.alt_m,
        accuracy_m: Some(accuracy_m),
        speed_mps: fix.speed_mps,
        track_deg,
        time,
        attitude,
    };
    out.validate()?;
    Ok(out)
}

const fn wire_source(source: HeadingSourceKind) -> Option<HeadingSource> {
    match source {
        HeadingSourceKind::Fused => Some(HeadingSource::Fused),
        HeadingSourceKind::Course => Some(HeadingSource::Course),
        HeadingSourceKind::Compass => Some(HeadingSource::Compass),
        HeadingSourceKind::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000_000;

    fn fix(t: i64) -> LocationSample {
        LocationSample {
            t_unix_ms: t,
            lat: 52.52,
            lon: 13.405,
            alt_m: Some(40.0),
            h_acc_m: 5.0,
            v_acc_m: None,
            speed_mps: Some(10.0),
            speed_acc_mps: None,
            course_deg: Some(91.0),
            course_acc_deg: Some(2.0),
        }
    }

    #[test]
    fn fixes_publish_at_most_10_hz_with_a_1_hz_heartbeat() {
        let mut policy = PublishPolicy::default();
        let mut sent = Vec::new();
        for step in 0..400 {
            let now = T0 + step * 10;
            if policy.due(now, now, Some(90.0)) {
                sent.push(now);
            }
        }
        assert!(
            sent.windows(2)
                .all(|pair| pair[1] - pair[0] >= MIN_INTERVAL_MS)
        );
        assert_eq!(MIN_INTERVAL_MS, 100);
        assert_eq!(sent.len(), 40);
        let mut still = PublishPolicy::default();
        let sent: Vec<i64> = (0..400)
            .map(|step| T0 + step * 10)
            .filter(|now| still.due(*now, T0, Some(90.0)))
            .collect();
        assert_eq!(sent, [T0, T0 + 1_000, T0 + 2_000, T0 + 3_000]);
    }

    #[test]
    fn a_heading_change_of_one_degree_publishes_early() {
        let mut policy = PublishPolicy::default();
        assert!(policy.due(T0, T0, Some(90.0)));
        assert!(!policy.due(T0 + 100, T0, Some(90.5)));
        assert!(policy.due(T0 + 200, T0, Some(91.0)));
        assert!(policy.due(T0 + 300, T0, Some(359.0)));
        assert!(!policy.due(T0 + 400, T0, Some(359.5)));
        assert!(policy.due(T0 + 500, T0, None));
    }

    #[test]
    fn errors_repeat_every_second_and_on_change() {
        let mut policy = PublishPolicy::default();
        assert!(policy.error_due(T0, "no GPS fix"));
        assert!(!policy.error_due(T0 + 500, "no GPS fix"));
        assert!(policy.error_due(T0 + 1_000, "no GPS fix"));
        assert!(policy.error_due(T0 + 1_100, "phone offline"));
        assert!(policy.due(T0 + 1_200, T0 - 5_000, None));
        policy.forget();
        assert_eq!(policy, PublishPolicy::default());
    }

    #[test]
    fn the_fix_carries_heading_fields_and_validates() {
        let heading = HeadingOut {
            deg: 92.5,
            sigma_deg: 3.0,
            source: HeadingSourceKind::Fused,
            yaw_rate_dps: Some(-4.0),
        };
        let out = build_fix(&fix(T0), T0, Some(heading), Some((3.0, -1.0))).expect("valid");
        assert_eq!(out.attitude.heading_deg, Some(92.5));
        assert_eq!(out.attitude.heading_accuracy_deg, Some(3.0));
        assert_eq!(out.attitude.heading_source, Some(HeadingSource::Fused));
        assert_eq!(out.attitude.yaw_rate_dps, Some(-4.0));
        assert_eq!(out.attitude.pitch_deg, Some(3.0));
        assert_eq!(out.attitude.roll_deg, Some(-1.0));
        assert_eq!(out.track_deg, Some(91.0));
        assert_eq!(out.time, "2026-09-21T14:13:20Z");
        assert_eq!(out.validate(), Ok(()));
        let bare = build_fix(&fix(T0), T0, None, None).expect("valid");
        assert!(bare.attitude.is_empty());
    }

    #[test]
    fn accuracy_grows_with_fix_age_at_speed() {
        let out = build_fix(&fix(T0), T0 + 2_000, None, None).expect("valid");
        assert_eq!(out.accuracy_m, Some(25.0));
        let mut parked = fix(T0);
        parked.speed_mps = Some(0.2);
        let out = build_fix(&parked, T0 + 2_000, None, None).expect("valid");
        assert!((out.accuracy_m.unwrap_or_default() - 5.4).abs() < 1e-9);
        assert_eq!(out.track_deg, None);
        let mut bad = fix(T0);
        bad.lat = 95.0;
        assert!(build_fix(&bad, T0, None, None).is_err());
    }
}
