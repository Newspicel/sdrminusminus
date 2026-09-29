use sdrmm_channels::{array_processor::GeoFix, pose_clock::PoseClock};
use sdrmm_wire::{HeadingSource, LatLon, PositionFix};

use crate::array::PoseSample;

const MIN_RATE_SPAN_NS: i64 = 20_000_000;
const MAX_RATE_SPAN_NS: i64 = 2_000_000_000;
const HEADING_SIGMA_DEG: f64 = 10.0;
const MOVING_MPS: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PoseView {
    pub(crate) position: LatLon,
    pub(crate) heading_deg: Option<f64>,
    pub(crate) heading_source: Option<HeadingSource>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PoseTrack {
    clock: PoseClock,
    turn_from: Option<(f64, i64)>,
    last: Option<PoseView>,
}

fn fix_time_ns(time: &str) -> Option<i64> {
    let stamp: jiff::Timestamp = time.parse().ok()?;
    i64::try_from(stamp.as_nanosecond()).ok()
}

fn turn_deg(from: f64, to: f64) -> f64 {
    (to - from + 540.0).rem_euclid(360.0) - 180.0
}

fn geo(fix: &PositionFix) -> GeoFix {
    GeoFix {
        lat: fix.latitude,
        lon: fix.longitude,
        altitude_m: fix.altitude_m,
        accuracy_m: fix.accuracy_m.map(|metres| metres as f32),
        speed_mps: fix.speed_mps.map(|speed| speed as f32),
    }
}

impl PoseTrack {
    pub(crate) const fn last(&self) -> Option<PoseView> {
        self.last
    }

    pub(crate) fn sample(&mut self, fix: Option<&PositionFix>, received_ns: i64) -> PoseSample {
        let Some(fix) = fix else {
            self.turn_from = None;
            self.last = None;
            return PoseSample {
                host_ns: received_ns,
                heading_deg: None,
                heading_sigma_deg: 0.0,
                yaw_rate_dps: None,
                fix: None,
                moving: false,
            };
        };
        let host_ns = fix_time_ns(&fix.time)
            .map_or(received_ns, |fix_ns| self.clock.map(fix_ns, received_ns));
        let heading_deg = fix
            .attitude
            .heading_deg
            .filter(|heading| heading.is_finite())
            .map(|heading| heading.rem_euclid(360.0));
        let yaw_rate_dps = fix
            .attitude
            .yaw_rate_dps
            .filter(|rate| rate.is_finite())
            .map(|rate| rate as f32)
            .or_else(|| self.derived_rate(heading_deg, host_ns));
        self.remember(heading_deg, host_ns);
        self.last = Some(PoseView {
            position: fix.at(),
            heading_deg,
            heading_source: fix.attitude.heading_source,
        });
        PoseSample {
            host_ns,
            heading_deg,
            heading_sigma_deg: fix
                .attitude
                .heading_accuracy_deg
                .unwrap_or(HEADING_SIGMA_DEG) as f32,
            yaw_rate_dps,
            fix: Some(geo(fix)),
            moving: fix.speed_mps.is_some_and(|speed| speed > MOVING_MPS),
        }
    }

    fn derived_rate(&self, heading_deg: Option<f64>, host_ns: i64) -> Option<f32> {
        let heading_deg = heading_deg?;
        let (from_deg, from_ns) = self.turn_from?;
        let span_ns = host_ns - from_ns;
        (MIN_RATE_SPAN_NS..=MAX_RATE_SPAN_NS)
            .contains(&span_ns)
            .then(|| (turn_deg(from_deg, heading_deg) / (span_ns as f64 * 1e-9)) as f32)
    }

    fn remember(&mut self, heading_deg: Option<f64>, host_ns: i64) {
        let Some(heading_deg) = heading_deg else {
            self.turn_from = None;
            return;
        };
        let close = self
            .turn_from
            .is_some_and(|(_, from_ns)| (0..MIN_RATE_SPAN_NS).contains(&(host_ns - from_ns)));
        if !close {
            self.turn_from = Some((heading_deg, host_ns));
        }
    }
}
