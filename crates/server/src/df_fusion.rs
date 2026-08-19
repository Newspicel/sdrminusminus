use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use sdrmm_wire::{
    DfEstimate, DfFusionState, DfGuidance, DfReading, DfStation, GuidanceMode, NavTarget,
    NavTargetKind, PositionFix,
};

pub(crate) const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// How wide the grid reaches from its anchor, and how finely it is cut. A hundred metres a cell
/// over sixty kilometres is fine enough for a fix an operator can drive to and coarse enough to
/// paint thousands of bearings into without noticing.
const EXTENT_M: f64 = 30_000.0;
const CELLS: usize = 301;
/// How sharp a wedge one bearing paints. A confident reading is worth a few degrees; a poor one
/// spreads over most of a quadrant and settles nothing on its own.
const MIN_SPREAD_DEG: f64 = 2.0;
const MAX_SPREAD_DEG: f64 = 40.0;
/// What a bearing is still worth after the next one arrives. Evidence has to fade, or a fix from
/// a mile back keeps outvoting the one from here.
const DECAY: f32 = 0.995;
/// Where the ellipse has closed up enough to drive at rather than across.
const CONVERGED_M: f64 = 400.0;
const MIN_SAMPLES: u32 = 6;
/// How far ahead the crossing waypoint is placed. Far enough that the bearing genuinely changes,
/// near enough to be one leg of a drive.
const CROSSING_M: f64 = 1_500.0;

#[must_use]
pub fn destination(lat: f64, lon: f64, bearing_deg: f64, distance_m: f64) -> (f64, f64) {
    let bearing = bearing_deg.to_radians();
    let angular = distance_m / EARTH_RADIUS_M;
    let phi = lat.to_radians();
    let lambda = lon.to_radians();
    let sin_phi = phi
        .sin()
        .mul_add(angular.cos(), phi.cos() * angular.sin() * bearing.cos());
    let phi2 = sin_phi.clamp(-1.0, 1.0).asin();
    let lambda2 = lambda
        + (bearing.sin() * angular.sin() * phi.cos()).atan2(angular.cos() - phi.sin() * sin_phi);
    (
        phi2.to_degrees(),
        (lambda2.to_degrees() + 540.0).rem_euclid(360.0) - 180.0,
    )
}

#[must_use]
pub fn bearing_between(from: (f64, f64), to: (f64, f64)) -> f64 {
    let phi1 = from.0.to_radians();
    let phi2 = to.0.to_radians();
    let delta = (to.1 - from.1).to_radians();
    let y = delta.sin() * phi2.cos();
    let x = phi1
        .cos()
        .mul_add(phi2.sin(), -(phi1.sin() * phi2.cos() * delta.cos()));
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

#[must_use]
pub fn distance_m(from: (f64, f64), to: (f64, f64)) -> f64 {
    let phi1 = from.0.to_radians();
    let phi2 = to.0.to_radians();
    let dphi = phi2 - phi1;
    let dlambda = (to.1 - from.1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().clamp(0.0, 1.0).asin()
}

fn wrap_deg(angle: f64) -> f64 {
    let wrapped = angle.rem_euclid(360.0);
    if wrapped > 180.0 {
        wrapped - 360.0
    } else {
        wrapped
    }
}

/// A geographic log-likelihood grid: every bearing anyone reports paints a wedge across it, and
/// where the wedges pile up is where the transmitter is.
pub(crate) struct FusionGrid {
    anchor: Option<(f64, f64)>,
    weight: Vec<f32>,
    samples: u32,
}

impl Default for FusionGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl FusionGrid {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            anchor: None,
            weight: vec![0.0; CELLS * CELLS],
            samples: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.anchor = None;
        self.weight.fill(0.0);
        self.samples = 0;
    }

    pub(crate) const fn samples(&self) -> u32 {
        self.samples
    }

    fn cell_size_m() -> f64 {
        2.0 * EXTENT_M / (CELLS - 1) as f64
    }

    /// Metres east and north of the anchor for a cell, and the other way round.
    fn cell_offset(index: usize) -> f64 {
        (index as f64 - (CELLS - 1) as f64 / 2.0) * Self::cell_size_m()
    }

    fn ensure_anchor(&mut self, lat: f64, lon: f64) {
        match self.anchor {
            Some(anchor) if distance_m(anchor, (lat, lon)) < EXTENT_M * 0.5 => {}
            Some(_) => {
                self.weight.fill(0.0);
                self.samples = 0;
                self.anchor = Some((lat, lon));
            }
            None => self.anchor = Some((lat, lon)),
        }
    }

    /// Adds one bearing seen from one place.
    pub(crate) fn paint(&mut self, lat: f64, lon: f64, bearing_deg: f64, confidence: f32) {
        self.ensure_anchor(lat, lon);
        let Some(anchor) = self.anchor else { return };
        let spread = MAX_SPREAD_DEG
            - (MAX_SPREAD_DEG - MIN_SPREAD_DEG) * f64::from(confidence.clamp(0.0, 1.0));
        let kappa = 1.0 / (2.0 * spread.to_radians().powi(2));
        let metres_per_lon = EARTH_RADIUS_M * anchor.0.to_radians().cos().max(1e-6);
        let east0 = (lon - anchor.1).to_radians() * metres_per_lon;
        let north0 = (lat - anchor.0).to_radians() * EARTH_RADIUS_M;
        for row in 0..CELLS {
            let north = Self::cell_offset(row) - north0;
            for col in 0..CELLS {
                let east = Self::cell_offset(col) - east0;
                if east == 0.0 && north == 0.0 {
                    continue;
                }
                let to_cell = east.atan2(north).to_degrees();
                let error = wrap_deg(to_cell - bearing_deg).to_radians();
                let weight = (-kappa * error * error) as f32;
                self.weight[row * CELLS + col] += weight;
            }
        }
        self.samples = self.samples.saturating_add(1);
    }

    pub(crate) fn decay(&mut self) {
        for value in &mut self.weight {
            *value *= DECAY;
        }
    }

    /// Where the wedges agree, and how tightly.
    #[must_use]
    pub(crate) fn estimate(&self) -> Option<DfEstimate> {
        let anchor = self.anchor?;
        if self.samples < 2 {
            return None;
        }
        let peak = self
            .weight
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        if !peak.is_finite() {
            return None;
        }
        let mut total = 0.0f64;
        let mut mean_east = 0.0f64;
        let mut mean_north = 0.0f64;
        for row in 0..CELLS {
            for col in 0..CELLS {
                let value = f64::from(self.weight[row * CELLS + col] - peak).exp();
                total += value;
                mean_east += value * Self::cell_offset(col);
                mean_north += value * Self::cell_offset(row);
            }
        }
        if total <= 0.0 {
            return None;
        }
        mean_east /= total;
        mean_north /= total;
        let (mut vee, mut vnn, mut ven) = (0.0f64, 0.0f64, 0.0f64);
        for row in 0..CELLS {
            for col in 0..CELLS {
                let value = f64::from(self.weight[row * CELLS + col] - peak).exp();
                let de = Self::cell_offset(col) - mean_east;
                let dn = Self::cell_offset(row) - mean_north;
                vee += value * de * de;
                vnn += value * dn * dn;
                ven += value * de * dn;
            }
        }
        vee /= total;
        vnn /= total;
        ven /= total;
        let trace = vee + vnn;
        let root = (((vee - vnn) * (vee - vnn)).mul_add(0.25, ven * ven)).sqrt();
        let major = (trace / 2.0 + root).max(0.0).sqrt();
        let minor = (trace / 2.0 - root).max(0.0).sqrt();
        let ellipse_bearing = (2.0 * ven)
            .atan2(vee - vnn)
            .mul_add(0.5, std::f64::consts::FRAC_PI_2)
            .to_degrees()
            .rem_euclid(180.0);
        let metres_per_lon = EARTH_RADIUS_M * anchor.0.to_radians().cos().max(1e-6);
        Some(DfEstimate {
            lat: anchor.0 + (mean_north / EARTH_RADIUS_M).to_degrees(),
            lon: anchor.1 + (mean_east / metres_per_lon).to_degrees(),
            ellipse_major_m: major * 2.0,
            ellipse_minor_m: minor * 2.0,
            ellipse_bearing_deg: ellipse_bearing,
            converged: major * 2.0 <= CONVERGED_M && self.samples >= MIN_SAMPLES,
            samples: self.samples,
        })
    }
}

/// What to do next, given where the vehicle is, where it last heard the signal, and how sure the
/// grid is.
///
/// While the ellipse is long there is nothing to drive at: two bearings taken from the same road
/// say the same thing. Driving across the bearing is what makes the next one different, and that
/// is what the guidance asks for until the ellipse closes.
#[must_use]
pub(crate) fn guidance(
    fix: &PositionFix,
    bearing_deg: f64,
    estimate: Option<DfEstimate>,
) -> DfGuidance {
    let here = (fix.latitude, fix.longitude);
    if let Some(estimate) = estimate.filter(|estimate| estimate.converged) {
        let target = (estimate.lat, estimate.lon);
        return DfGuidance {
            heading_deg: bearing_between(here, target),
            mode: GuidanceMode::Approach,
            distance_m: distance_m(here, target),
            nav_target: NavTarget {
                lat: estimate.lat,
                lon: estimate.lon,
                kind: NavTargetKind::Target,
            },
        };
    }
    let track = fix.track_deg.unwrap_or(bearing_deg);
    let left = (bearing_deg - 90.0).rem_euclid(360.0);
    let right = (bearing_deg + 90.0).rem_euclid(360.0);
    let heading = if wrap_deg(left - track).abs() <= wrap_deg(right - track).abs() {
        left
    } else {
        right
    };
    let (lat, lon) = destination(here.0, here.1, heading, CROSSING_M);
    DfGuidance {
        heading_deg: heading,
        mode: GuidanceMode::Cross,
        distance_m: CROSSING_M,
        nav_target: NavTarget {
            lat,
            lon,
            kind: NavTargetKind::Cross,
        },
    }
}

#[derive(Default)]
struct NodeFusion {
    grid: FusionGrid,
    stations: HashMap<String, DfStation>,
    guidance: Option<DfGuidance>,
    announced: bool,
}

impl NodeFusion {
    fn see(&mut self, station: &str, lat: f64, lon: f64, at: &str) {
        let entry = self
            .stations
            .entry(station.to_owned())
            .or_insert_with(|| DfStation {
                station_id: station.to_owned(),
                lat,
                lon,
                bearings: 0,
                last_seen: at.to_owned(),
            });
        entry.lat = lat;
        entry.lon = lon;
        entry.bearings = entry.bearings.saturating_add(1);
        entry.last_seen = at.to_owned();
    }
}

/// One grid per triangulation node, fed by every direction finder wired into it. Bearings from
/// one place say where a signal is *towards*; bearings from two places say where it *is*.
#[derive(Default)]
pub(crate) struct FusionHub {
    nodes: Mutex<HashMap<String, NodeFusion>>,
}

/// What one new bearing changed: the state to publish, and whether this was the moment the fix
/// closed up — the one worth telling everyone about.
pub(crate) struct FusionOutcome {
    pub(crate) state: DfFusionState,
    pub(crate) first_fix: Option<DfEstimate>,
}

impl FusionHub {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, NodeFusion>> {
        self.nodes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn reset(&self, node: &str) {
        if let Some(fusion) = self.lock().get_mut(node) {
            fusion.grid.reset();
            fusion.stations.clear();
            fusion.guidance = None;
            fusion.announced = false;
        }
    }

    pub(crate) fn state(&self, node: &str) -> Option<DfFusionState> {
        let nodes = self.lock();
        let fusion = nodes.get(node)?;
        Some(DfFusionState {
            estimate: fusion.grid.estimate(),
            guidance: fusion.guidance,
            stations: fusion.stations.values().cloned().collect(),
            samples: fusion.grid.samples(),
        })
    }

    /// Folds in a bearing one of the wired finders measured, from wherever it was standing.
    pub(crate) fn observe(
        &self,
        node: &str,
        station: &str,
        reading: &DfReading,
        fix: Option<&PositionFix>,
        at: &str,
    ) -> Option<FusionOutcome> {
        let fix = fix?;
        if reading.confidence <= 0.0 {
            return None;
        }
        let mut nodes = self.lock();
        let fusion = nodes.entry(node.to_owned()).or_default();
        fusion.grid.decay();
        fusion.grid.paint(
            fix.latitude,
            fix.longitude,
            f64::from(reading.bearing_deg),
            reading.confidence,
        );
        fusion.see(station, fix.latitude, fix.longitude, at);
        let estimate = fusion.grid.estimate();
        let guidance = guidance(fix, f64::from(reading.bearing_deg), estimate);
        fusion.guidance = Some(guidance);
        let first_fix = match estimate {
            Some(estimate) if estimate.converged && !fusion.announced => {
                fusion.announced = true;
                Some(estimate)
            }
            Some(estimate) if !estimate.converged => {
                fusion.announced = false;
                None
            }
            _ => None,
        };
        Some(FusionOutcome {
            state: DfFusionState {
                estimate,
                guidance: Some(guidance),
                stations: fusion.stations.values().cloned().collect(),
                samples: fusion.grid.samples(),
            },
            first_fix,
        })
    }
}

pub(crate) type SharedFusion = Arc<FusionHub>;

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: (f64, f64) = (51.5, 7.0);
    const AT: &str = "2026-01-01T00:00:01Z";

    fn fix(lat: f64, lon: f64, track_deg: Option<f64>) -> PositionFix {
        PositionFix {
            latitude: lat,
            longitude: lon,
            altitude_m: None,
            accuracy_m: None,
            speed_mps: None,
            track_deg,
            time: "2026-01-01T00:00:00Z".to_owned(),
        }
    }

    fn reading(bearing_deg: f32) -> DfReading {
        DfReading {
            bearing_deg,
            confidence: 0.9,
            peak_to_floor_db: 20.0,
            pseudospectrum: vec![0; 360],
        }
    }

    #[test]
    fn a_bearing_and_a_distance_round_trip_through_the_globe() {
        for (bearing, distance) in [(0.0, 1_000.0), (90.0, 5_000.0), (215.0, 12_000.0)] {
            let there = destination(HOME.0, HOME.1, bearing, distance);
            assert!(
                (distance_m(HOME, there) - distance).abs() < 1.0,
                "{bearing}"
            );
            let back = bearing_between(HOME, there);
            assert!(wrap_deg(back - bearing).abs() < 0.1, "{bearing} vs {back}");
        }
    }

    #[test]
    fn three_bearings_from_three_places_cross_where_the_transmitter_is() {
        let target = destination(HOME.0, HOME.1, 45.0, 6_000.0);
        let mut grid = FusionGrid::new();
        for offset in [0.0f64, 90.0, 180.0] {
            let observer = destination(HOME.0, HOME.1, offset, 3_000.0);
            let bearing = bearing_between(observer, target);
            grid.paint(observer.0, observer.1, bearing, 0.9);
        }
        let estimate = grid.estimate().expect("three crossings give an estimate");
        let error = distance_m((estimate.lat, estimate.lon), target);
        assert!(
            error < 500.0,
            "{error} m from the transmitter: {estimate:?}"
        );
    }

    #[test]
    fn bearings_taken_from_one_place_leave_a_long_ellipse() {
        let target = destination(HOME.0, HOME.1, 45.0, 6_000.0);
        let mut grid = FusionGrid::new();
        for _ in 0..6 {
            grid.paint(HOME.0, HOME.1, bearing_between(HOME, target), 0.9);
        }
        let estimate = grid.estimate().expect("an estimate");
        assert!(
            estimate.ellipse_major_m > 4.0 * estimate.ellipse_minor_m,
            "{estimate:?}"
        );
        assert!(!estimate.converged, "{estimate:?}");
    }

    #[test]
    fn guidance_crosses_the_bearing_until_the_fix_closes_up() {
        let hub = FusionHub::default();
        let target = destination(HOME.0, HOME.1, 45.0, 6_000.0);
        let bearing = bearing_between(HOME, target);
        let outcome = hub
            .observe(
                "cross",
                "north",
                &reading(bearing as f32),
                Some(&fix(HOME.0, HOME.1, Some(0.0))),
                AT,
            )
            .expect("a fix is enough to guide");
        let guidance = outcome.state.guidance.expect("guidance");
        assert_eq!(guidance.mode, GuidanceMode::Cross);
        let across = wrap_deg(guidance.heading_deg - bearing).abs();
        assert!((across - 90.0).abs() < 1e-6, "{guidance:?}");
        assert_eq!(guidance.nav_target.kind, NavTargetKind::Cross);
        assert!(
            (distance_m(HOME, (guidance.nav_target.lat, guidance.nav_target.lon)) - CROSSING_M)
                .abs()
                < 5.0
        );
    }

    #[test]
    fn a_converged_fix_turns_the_guidance_towards_it() {
        let target = destination(HOME.0, HOME.1, 45.0, 6_000.0);
        let estimate = DfEstimate {
            lat: target.0,
            lon: target.1,
            ellipse_major_m: 100.0,
            ellipse_minor_m: 80.0,
            ellipse_bearing_deg: 0.0,
            converged: true,
            samples: 20,
        };
        let guidance = guidance(&fix(HOME.0, HOME.1, Some(0.0)), 45.0, Some(estimate));
        assert_eq!(guidance.mode, GuidanceMode::Approach);
        assert_eq!(guidance.nav_target.kind, NavTargetKind::Target);
        assert!(wrap_deg(guidance.heading_deg - bearing_between(HOME, target)).abs() < 1e-6);
        assert!((guidance.distance_m - 6_000.0).abs() < 5.0);
    }

    #[test]
    fn a_second_station_crosses_what_one_alone_could_not() {
        let hub = FusionHub::default();
        let target = destination(HOME.0, HOME.1, 45.0, 6_000.0);
        for _ in 0..4 {
            hub.observe(
                "cross",
                "north",
                &reading(bearing_between(HOME, target) as f32),
                Some(&fix(HOME.0, HOME.1, None)),
                AT,
            );
        }
        let alone = hub
            .state("cross")
            .expect("state")
            .estimate
            .expect("estimate");
        let away = destination(HOME.0, HOME.1, 135.0, 6_000.0);
        for _ in 0..4 {
            hub.observe(
                "cross",
                "east",
                &reading(bearing_between(away, target) as f32),
                Some(&fix(away.0, away.1, None)),
                AT,
            );
        }
        let together = hub.state("cross").expect("state");
        let estimate = together.estimate.expect("estimate");
        assert!(
            estimate.ellipse_major_m < alone.ellipse_major_m / 2.0,
            "{alone:?} then {estimate:?}"
        );
        let error = distance_m((estimate.lat, estimate.lon), target);
        assert!(error < 700.0, "{error} m away: {estimate:?}");
        assert_eq!(together.stations.len(), 2, "both finders are named");
        assert!(
            together
                .stations
                .iter()
                .all(|station| station.bearings == 4)
        );
    }

    #[test]
    fn a_reset_clears_everything_the_grid_had_learned() {
        let hub = FusionHub::default();
        hub.observe(
            "cross",
            "north",
            &reading(45.0),
            Some(&fix(HOME.0, HOME.1, None)),
            AT,
        );
        hub.observe(
            "cross",
            "north",
            &reading(50.0),
            Some(&fix(HOME.0, HOME.1, None)),
            AT,
        );
        assert!(hub.state("cross").expect("state").samples >= 2);
        hub.reset("cross");
        assert_eq!(hub.state("cross").expect("state").samples, 0);
        assert!(hub.state("cross").expect("state").estimate.is_none());
    }

    #[test]
    fn a_bearing_with_no_position_changes_nothing() {
        let hub = FusionHub::default();
        assert!(
            hub.observe("cross", "north", &reading(45.0), None, AT)
                .is_none()
        );
        assert!(hub.state("cross").is_none());
    }
}
