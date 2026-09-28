use sdrmm_wire::geo::{self, wrap_360};

use crate::{
    missions::views::{GuidanceView, NavPoint, RetargetNotice},
    pose::PoseSnapshot,
};

pub(crate) mod nav_url;
mod retarget;

use retarget::{Decision, Sent, decide};

pub(crate) fn relative(bearing_true_deg: f64, heading_deg: f64) -> f64 {
    wrap_360(bearing_true_deg - heading_deg)
}

pub(crate) fn view(target: NavPoint, pose: Option<PoseSnapshot>) -> Option<GuidanceView> {
    let pose = pose?;
    let bearing = geo::bearing_deg(pose.at.into(), target.at.into());
    Some(GuidanceView {
        kind: target.kind,
        heading_true_deg: bearing,
        heading_rel_deg: pose.heading_deg.map(|heading| relative(bearing, heading)),
        distance_m: geo::distance_m(pose.at.into(), target.at.into()),
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Retargeter {
    sent: Option<Sent>,
}

impl Retargeter {
    pub(crate) fn offer(
        &mut self,
        mission: &str,
        target: NavPoint,
        now_ms: i64,
    ) -> Option<RetargetNotice> {
        match decide(self.sent, target, now_ms) {
            Decision::Keep => None,
            Decision::Send { reason, moved_m } => {
                self.sent = Some(Sent {
                    target,
                    t_ms: now_ms,
                });
                Some(RetargetNotice {
                    mission: mission.to_owned(),
                    target,
                    moved_m,
                    reason,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        missions::views::{GuidanceKind, RetargetReason},
        records::LatLon,
    };

    const HOME: LatLon = LatLon {
        lat: 52.52,
        lon: 13.405,
    };

    fn pose(heading_deg: Option<f64>) -> PoseSnapshot {
        PoseSnapshot {
            at: HOME,
            heading_deg,
            t_ms: 0,
        }
    }

    fn east(metres: f64) -> NavPoint {
        NavPoint {
            at: geo::offset_m(HOME.into(), metres, 0.0).into(),
            kind: GuidanceKind::Estimate,
        }
    }

    #[test]
    fn relative_bearing_wraps() {
        assert_eq!(relative(10.0, 350.0), 20.0);
        assert_eq!(relative(350.0, 10.0), 340.0);
        let guide = view(east(1_000.0), Some(pose(Some(80.0)))).expect("view");
        assert!((guide.heading_true_deg - 90.0).abs() < 0.1);
        assert!(
            guide
                .heading_rel_deg
                .is_some_and(|rel| (rel - 10.0).abs() < 0.1)
        );
    }

    #[test]
    fn relative_bearing_is_none_without_heading() {
        let guide = view(east(1_000.0), Some(pose(None))).expect("view");
        assert_eq!(guide.heading_rel_deg, None);
        assert_eq!(view(east(1_000.0), None), None);
    }

    #[test]
    fn distance_uses_the_phones_own_fix() {
        let guide = view(east(2_500.0), Some(pose(Some(0.0)))).expect("view");
        assert!((guide.distance_m - 2_500.0).abs() < 1.0);
        assert_eq!(guide.kind, GuidanceKind::Estimate);
    }

    #[test]
    fn retargets_are_announced_once_per_decision() {
        let mut retargeter = Retargeter::default();
        let first = retargeter.offer("df1", east(1_000.0), 0).expect("first");
        assert_eq!(first.reason, RetargetReason::First);
        assert_eq!(first.mission, "df1");
        assert_eq!(retargeter.offer("df1", east(1_100.0), 60_000), None);
        let moved = retargeter
            .offer("df1", east(1_400.0), 60_000)
            .expect("moved");
        assert_eq!(moved.reason, RetargetReason::Moved);
        assert!((moved.moved_m - 400.0).abs() < 1.0);
    }
}
