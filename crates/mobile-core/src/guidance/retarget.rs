use sdrmm_wire::geo;

use crate::missions::views::{NavPoint, RetargetReason};

pub(crate) const RETARGET_MOVE_M: f64 = 250.0;
pub(crate) const RETARGET_MIN_INTERVAL_MS: i64 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sent {
    pub(crate) target: NavPoint,
    pub(crate) t_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Decision {
    Keep,
    Send {
        reason: RetargetReason,
        moved_m: f64,
    },
}

pub(crate) fn decide(sent: Option<Sent>, next: NavPoint, now_ms: i64) -> Decision {
    let Some(sent) = sent else {
        return Decision::Send {
            reason: RetargetReason::First,
            moved_m: 0.0,
        };
    };
    let moved_m = geo::distance_m(sent.target.at.into(), next.at.into());
    if sent.target.kind != next.kind {
        return Decision::Send {
            reason: RetargetReason::KindChanged,
            moved_m,
        };
    }
    if moved_m > RETARGET_MOVE_M && now_ms - sent.t_ms >= RETARGET_MIN_INTERVAL_MS {
        return Decision::Send {
            reason: RetargetReason::Moved,
            moved_m,
        };
    }
    Decision::Keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{missions::views::GuidanceKind, records::LatLon};

    const HOME: LatLon = LatLon {
        lat: 52.52,
        lon: 13.405,
    };

    fn point(north_m: f64, kind: GuidanceKind) -> NavPoint {
        NavPoint {
            at: geo::offset_m(HOME.into(), 0.0, north_m).into(),
            kind,
        }
    }

    fn sent(north_m: f64, kind: GuidanceKind, t_ms: i64) -> Option<Sent> {
        Some(Sent {
            target: point(north_m, kind),
            t_ms,
        })
    }

    fn moved(decision: Decision) -> f64 {
        match decision {
            Decision::Send { moved_m, .. } => moved_m,
            Decision::Keep => f64::NAN,
        }
    }

    #[test]
    fn retarget_first_moved_and_kind_flip() {
        assert_eq!(
            decide(None, point(0.0, GuidanceKind::Probe), 0),
            Decision::Send {
                reason: RetargetReason::First,
                moved_m: 0.0
            }
        );
        let far = decide(
            sent(0.0, GuidanceKind::Estimate, 0),
            point(400.0, GuidanceKind::Estimate),
            31_000,
        );
        assert!(matches!(
            far,
            Decision::Send {
                reason: RetargetReason::Moved,
                ..
            }
        ));
        assert!((moved(far) - 400.0).abs() < 0.5);
        let flip = decide(
            sent(0.0, GuidanceKind::Probe, 0),
            point(10.0, GuidanceKind::Estimate),
            1_000,
        );
        assert!(matches!(
            flip,
            Decision::Send {
                reason: RetargetReason::KindChanged,
                ..
            }
        ));
        assert!((moved(flip) - 10.0).abs() < 0.5);
    }

    #[test]
    fn retarget_ignores_under_250_m() {
        let previous = sent(0.0, GuidanceKind::Estimate, 0);
        assert_eq!(
            decide(previous, point(240.0, GuidanceKind::Estimate), 600_000),
            Decision::Keep
        );
        assert!(matches!(
            decide(previous, point(260.0, GuidanceKind::Estimate), 600_000),
            Decision::Send {
                reason: RetargetReason::Moved,
                ..
            }
        ));
    }

    #[test]
    fn retarget_waits_30_s_except_on_kind_flip() {
        let previous = sent(0.0, GuidanceKind::Estimate, 100_000);
        let far = point(2_000.0, GuidanceKind::Estimate);
        assert_eq!(decide(previous, far, 129_999), Decision::Keep);
        assert!(matches!(
            decide(previous, far, 130_000),
            Decision::Send {
                reason: RetargetReason::Moved,
                ..
            }
        ));
        assert!(matches!(
            decide(previous, point(2_000.0, GuidanceKind::Probe), 100_001),
            Decision::Send {
                reason: RetargetReason::KindChanged,
                ..
            }
        ));
    }
}
