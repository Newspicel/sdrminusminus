use std::time::Instant;

use sdrmm_wire::{
    DfBearing, DfEstimate, NavMode, NavTargetKind, PositionFix, TriangulationParams,
    fusion::{BearingSource, FusionDecay},
    geo::{self, Enu, LatLon},
};

use super::{
    grid::{CELLS, FRAME_CELLS, LogGrid, look},
    nav::Nav,
    observation::{BANKS, FLOOR, RING, prepare},
    *,
};

const HOME: LatLon = LatLon {
    lat: 51.5,
    lon: 7.0,
};
const AT: &str = "2026-01-01T00:00:00Z";

fn toward(station: &str, from: LatLon, target: LatLon, sigma_deg: f32) -> DfBearing {
    aimed(station, from, geo::bearing_deg(from, target), sigma_deg)
}

fn aimed(station: &str, from: LatLon, bearing_deg: f64, sigma_deg: f32) -> DfBearing {
    DfBearing {
        bearing_deg: geo::wrap_360(bearing_deg) as f32,
        confidence: 0.9,
        lat: Some(from.lat),
        lon: Some(from.lon),
        station_id: Some(station.to_owned()),
        node: "df".to_owned(),
        sigma_deg,
        accuracy_m: Some(5.0),
        heading_deg: None,
        heading_sigma_deg: None,
        relative_deg: None,
        mirror_deg: None,
        freq_hz: Some(433.92e6),
        source: BearingSource::Array,
        moving: false,
        others: Vec::new(),
        likelihood: Vec::new(),
    }
}

fn params(decay: FusionDecay, nav: NavMode) -> TriangulationParams {
    TriangulationParams {
        decay,
        nav,
        ..TriangulationParams::default()
    }
}

fn see(fusion: &mut NodeFusion, bearing: &DfBearing, at_s: f64) -> FusionOutcome {
    fusion
        .observe(bearing, at_s, AT)
        .expect("a bearing with a position")
}

fn estimate(fusion: &NodeFusion) -> DfEstimate {
    fusion.estimate.expect("an estimate")
}

fn place(estimate: &DfEstimate) -> LatLon {
    LatLon {
        lat: estimate.lat,
        lon: estimate.lon,
    }
}

fn miss_m(estimate: &DfEstimate, truth: LatLon) -> f64 {
    geo::distance_m(place(estimate), truth)
}

fn sigmas_away(estimate: &DfEstimate, truth: LatLon) -> f64 {
    let (east, north) = Enu::new(place(estimate)).to_enu(truth);
    let axis = estimate.ellipse_bearing_deg.to_radians();
    let along = east * axis.sin() + north * axis.cos();
    let across = east * axis.cos() - north * axis.sin();
    let major = estimate.ellipse_major_m / 2.0;
    let minor = (estimate.ellipse_minor_m / 2.0).max(1.0);
    (along / major).hypot(across / minor)
}

fn fix_at(at: LatLon) -> PositionFix {
    PositionFix {
        latitude: at.lat,
        longitude: at.lon,
        altitude_m: None,
        accuracy_m: Some(5.0),
        speed_mps: None,
        track_deg: None,
        time: AT.to_owned(),
        attitude: sdrmm_wire::Attitude::default(),
    }
}

fn around(target: LatLon, from_deg: f64, range_m: f64) -> LatLon {
    geo::destination(target, from_deg, range_m)
}

fn key_power(fusion: &NodeFusion, station: &str) -> f32 {
    fusion
        .votes
        .keys
        .iter()
        .filter(|key| key.station == station)
        .filter_map(|key| key.painted.as_ref())
        .map(|painted| painted.power)
        .sum()
}

fn job(now_s: f64) -> Job {
    Job::Bearing(Box::new(Sighting {
        node: "cross".to_owned(),
        device_set: 0,
        bearing: aimed("roof", HOME, 45.0, 3.0),
        now_s,
        at: AT.to_owned(),
    }))
}

#[test]
fn three_stations_cross_at_the_transmitter() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    for round in 0..3u32 {
        for (index, from_deg) in [0.0, 120.0, 240.0].into_iter().enumerate() {
            let station = around(target, from_deg, 3_000.0);
            let at_s = f64::from(round) * 3.0 + index as f64;
            see(
                &mut fusion,
                &toward(&format!("s{index}"), station, target, 3.0),
                at_s,
            );
        }
    }
    let found = estimate(&fusion);
    assert!(miss_m(&found, target) < 300.0, "{found:?}");
    assert_eq!(fusion.state(9.0).stations.len(), 3);
}

#[test]
fn a_ula_mirror_leaves_two_modes_until_a_second_station_resolves_it() {
    let target = geo::destination(HOME, 90.0, 4_000.0);
    let mirror = geo::destination(HOME, 270.0, 4_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    let mut at_s = 0.0;
    for step in -2i32..=2 {
        let car = geo::destination(HOME, 0.0, f64::from(step) * 1_000.0);
        for _ in 0..3 {
            let seen = geo::bearing_deg(car, target);
            let bearing = DfBearing {
                mirror_deg: Some(geo::wrap_360(360.0 - seen) as f32),
                moving: true,
                ..aimed("car", car, seen, 2.0)
            };
            see(&mut fusion, &bearing, at_s);
            at_s += 1.0;
        }
        at_s += 7.0;
    }
    let modes = fusion.state(at_s).emitters;
    assert!(modes.len() >= 2, "{modes:?}");
    let near = |truth: LatLon| {
        modes
            .iter()
            .find(|mode| miss_m(mode, truth) < 500.0)
            .unwrap_or_else(|| panic!("no mode near {truth:?}: {modes:?}"))
    };
    let ratio = near(target).mass / near(mirror).mass;
    assert!((0.75..=1.33).contains(&ratio), "{modes:?}");

    let tower = around(target, 180.0, 4_000.0);
    for _ in 0..3 {
        see(&mut fusion, &toward("tower", tower, target, 2.0), at_s);
        at_s += 1.0;
    }
    let found = estimate(&fusion);
    assert!(miss_m(&found, target) < 300.0, "{found:?}");
    let first = fusion.state(at_s).emitters[0];
    assert!(miss_m(&first, target) < 300.0, "{first:?}");
}

fn crossing_major(heading_sigma_deg: f32) -> f64 {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    for round in 0..10u32 {
        for (index, from_deg) in [180.0, 270.0].into_iter().enumerate() {
            let station = around(target, from_deg, 5_000.0);
            let bearing = DfBearing {
                heading_sigma_deg: Some(heading_sigma_deg),
                ..toward(&format!("s{index}"), station, target, 1.0)
            };
            see(&mut fusion, &bearing, f64::from(round) + index as f64 * 0.5);
        }
    }
    estimate(&fusion).ellipse_major_m
}

#[test]
fn heading_uncertainty_widens_the_ellipse() {
    let tight = crossing_major(1.0);
    let loose = crossing_major(10.0);
    assert!(loose >= 2.0 * tight, "{tight} m then {loose} m");
}

fn wedge_drops(accuracy_m: f32) -> (f32, f32) {
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    let bearing = DfBearing {
        accuracy_m: Some(accuracy_m),
        ..aimed("roof", HOME, 0.0, 10.0)
    };
    see(&mut fusion, &bearing, 0.0);
    let cell_m = fusion.grid.as_ref().expect("grid").cell_m();
    let wedge = &fusion.votes.keys[0]
        .painted
        .as_ref()
        .expect("painted")
        .wedge;
    let sample = |range_m: f64, off_deg: f64| {
        let angle = off_deg.to_radians();
        look(
            range_m * angle.sin(),
            range_m * angle.cos(),
            wedge.accuracy_m,
            cell_m,
        )
        .sample(&wedge.ring, wedge.centre_value)
    };
    (
        sample(100.0, 0.0) - sample(100.0, 20.0),
        sample(5_000.0, 0.0) - sample(5_000.0, 5.0),
    )
}

#[test]
fn position_uncertainty_widens_only_near_the_station() {
    let (near_sharp, far_sharp) = wedge_drops(10.0);
    let (near_loose, far_loose) = wedge_drops(200.0);
    assert!(
        near_loose < 0.6 * near_sharp,
        "{near_sharp} vs {near_loose}"
    );
    assert!(
        (far_loose - far_sharp).abs() <= 0.12 * far_sharp,
        "{far_sharp} vs {far_loose}"
    );
}

fn contribution_after(decay: FusionDecay, after_s: f64) -> f32 {
    let mut fusion = NodeFusion::new(&params(decay, NavMode::Auto));
    see(&mut fusion, &aimed("old", HOME, 45.0, 3.0), 0.0);
    let before = key_power(&fusion, "old");
    let far = geo::destination(HOME, 90.0, 5_000.0);
    see(&mut fusion, &aimed("new", far, 0.0, 3.0), after_s);
    key_power(&fusion, "old") / before
}

#[test]
fn fixed_decay_forgets_in_minutes_moving_keeps_half_an_hour() {
    let halved_thrice = contribution_after(FusionDecay::Fixed, 3.0 * 60.0);
    assert!(
        (halved_thrice - 0.125).abs() < 0.01,
        "three half-lives leave an eighth: {halved_thrice}"
    );
    let fixed = contribution_after(FusionDecay::Fixed, 5.0 * 60.0);
    assert!(fixed < 0.04, "{fixed}");
    let moving = contribution_after(FusionDecay::Moving, 5.0 * 60.0);
    assert!(moving > 0.85, "{moving}");
    let moving_later = contribution_after(FusionDecay::Moving, 5.0 * 1_800.0);
    assert!(moving_later < 0.04, "{moving_later}");
}

#[test]
fn a_fast_reporting_station_does_not_outweigh_a_slow_one() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let fast = around(target, 0.0, 5_000.0);
    let slow = [
        around(target, 120.0, 5_000.0),
        around(target, 240.0, 5_000.0),
    ];
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    for tick in 0..600u32 {
        let at_s = f64::from(tick) / 10.0;
        let biased = geo::bearing_deg(fast, target) + 5.0;
        see(&mut fusion, &aimed("fast", fast, biased, 2.0), at_s);
        if tick % 20 == 0 {
            for (index, station) in slow.iter().enumerate() {
                see(
                    &mut fusion,
                    &toward(&format!("slow{index}"), *station, target, 2.0),
                    at_s + 0.01,
                );
            }
        }
    }
    let found = estimate(&fusion);
    assert!(miss_m(&found, target) < 300.0, "{found:?}");
}

#[test]
fn biased_fixed_stations_do_not_converge_falsely() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let south = around(target, 180.0, 5_000.0);
    let west = around(target, 270.0, 5_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    for tick in 0..1_200u32 {
        let at_s = f64::from(tick) / 2.0;
        for (station, from) in [("south", south), ("west", west)] {
            let biased = geo::bearing_deg(from, target) + 3.0;
            let outcome = see(&mut fusion, &aimed(station, from, biased, 1.0), at_s);
            assert!(outcome.first_fix.is_none(), "announced at {at_s} s");
            assert!(
                outcome
                    .state
                    .estimate
                    .is_none_or(|estimate| !estimate.converged),
                "converged at {at_s} s"
            );
        }
    }
    let found = estimate(&fusion);
    let away = sigmas_away(&found, target);
    assert!(away <= 2.0, "{away} sigma: {found:?}");
}

#[test]
fn a_new_key_shrinks_the_ellipse_repeats_do_not() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let a = around(target, 180.0, 5_000.0);
    let b = around(target, 220.0, 5_000.0);
    let mut fusion = NodeFusion::new(&params(
        FusionDecay::HalfLife { seconds: 86_400 },
        NavMode::Auto,
    ));
    let mut at_s = 0.0;
    for _ in 0..10 {
        see(&mut fusion, &toward("a", a, target, 2.0), at_s);
        see(&mut fusion, &toward("b", b, target, 2.0), at_s + 0.5);
        at_s += 1.0;
    }
    let settled = estimate(&fusion).ellipse_major_m;
    for _ in 0..600 {
        see(&mut fusion, &toward("a", a, target, 2.0), at_s);
        at_s += 1.0;
    }
    let repeated = estimate(&fusion).ellipse_major_m;
    assert!(
        (repeated - settled).abs() <= 0.1 * settled,
        "{settled} m then {repeated} m"
    );
    let c = around(target, 240.0, 5_000.0);
    see(&mut fusion, &toward("c", c, target, 2.0), at_s);
    let crossed = estimate(&fusion).ellipse_major_m;
    assert!(crossed < 0.9 * repeated, "{repeated} m then {crossed} m");
}

#[test]
fn the_grid_recentres_when_the_mass_nears_the_edge() {
    let north = geo::destination(HOME, 0.0, 10_000.0);
    let target = geo::destination(north, 90.0, 3_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    fusion.recentred_s = Some(0.0);
    let east = geo::destination(north, 90.0, 8_000.0);
    for tick in 0..4u32 {
        let at_s = f64::from(tick);
        see(&mut fusion, &toward("home", HOME, target, 1.0), at_s);
        see(&mut fusion, &toward("east", east, target, 1.0), at_s + 0.5);
    }
    assert_eq!(
        fusion.grid.as_ref().expect("grid").centre(),
        (0.0, 0.0),
        "held for five seconds"
    );
    let before = estimate(&fusion);
    let outcome = fusion.settle(10.0, false);
    let grid = fusion.grid.as_ref().expect("grid");
    assert_ne!(grid.centre(), (0.0, 0.0), "the grid moved");
    let after = outcome.state.estimate.expect("estimate");
    let moved = geo::distance_m(place(&before), place(&after));
    assert!(moved < grid.cell_m(), "{before:?} then {after:?}");
    assert!(miss_m(&after, target) < 300.0, "{after:?}");
}

#[test]
fn two_transmitters_give_two_emitters() {
    let middle = geo::destination(HOME, 45.0, 6_000.0);
    let first = geo::destination(middle, 270.0, 1_500.0);
    let second = geo::destination(middle, 90.0, 1_500.0);
    let stations = [0.0, 120.0, 240.0].map(|from_deg| around(middle, from_deg, 6_000.0));
    let mut fusion = NodeFusion::new(&params(
        FusionDecay::HalfLife { seconds: 86_400 },
        NavMode::Auto,
    ));
    let pattern = [
        true, true, false, true, true, false, true, true, false, true,
    ];
    let mut at_s = 0.0;
    for _ in 0..3 {
        for strong in pattern {
            let target = if strong { first } else { second };
            for (index, station) in stations.iter().enumerate() {
                see(
                    &mut fusion,
                    &toward(&format!("s{index}"), *station, target, 2.0),
                    at_s + index as f64 * 0.1,
                );
            }
            at_s += 1.0;
        }
    }
    let emitters = fusion.state(at_s).emitters;
    for truth in [first, second] {
        assert!(
            emitters
                .iter()
                .any(|emitter| miss_m(emitter, truth) < 400.0),
            "nothing near {truth:?}: {emitters:?}"
        );
    }
}

#[test]
fn the_ellipse_bearing_is_right_for_a_diagonal_spread() {
    let mut grid = LogGrid::new(HOME, 12.8);
    let axis = 45f64.to_radians();
    for row in 0..CELLS {
        for col in 0..CELLS {
            let (east, north) = grid.cell_centre(row, col);
            let along = east * axis.sin() + north * axis.cos();
            let across = east * axis.cos() - north * axis.sin();
            let value = -((along / 900.0).powi(2) + (across / 200.0).powi(2)) / 2.0;
            grid.values_mut()[row * CELLS + col] = value as f32;
        }
    }
    let found = estimate::global(&grid, 10).expect("estimate").estimate;
    assert!((found.ellipse_bearing_deg - 45.0).abs() <= 5.0, "{found:?}");
    assert!(
        found.ellipse_major_m > 3.0 * found.ellipse_minor_m,
        "{found:?}"
    );
}

#[test]
fn a_legacy_bearing_is_synthesised_from_its_sigma() {
    let sigma = 5.0f32;
    let observation = prepare(&aimed("roof", HOME, 100.0, sigma), 0.0, 0.05).expect("accepted");
    let ring = &observation.bank[0];
    let peak = (0..RING)
        .max_by(|a, b| ring[*a].total_cmp(&ring[*b]))
        .expect("a ring");
    assert_eq!(peak, 100);
    let bend = -(ring[peak - 1] - 2.0 * ring[peak] + ring[peak + 1]);
    let expected = (1.0 - FLOOR) / (sigma * sigma);
    assert!(
        (bend - expected).abs() <= 0.05 * expected,
        "{bend} vs {expected}"
    );
    assert!(ring[peak].abs() < 1e-3, "the peak is normalised to one");
    let widest = &observation.bank[BANKS - 1];
    let side = (peak + 90) % RING;
    assert!(widest[peak] < ring[peak] && widest[side] > ring[side]);
}

#[test]
fn a_bearing_without_position_is_refused_and_counted() {
    let hub = FusionHub::default();
    hub.configure("cross", &TriangulationParams::default());
    let blind = DfBearing {
        lat: None,
        ..aimed("roof", HOME, 45.0, 3.0)
    };
    assert!(matches!(
        hub.observe("cross", &blind, 0.0, AT),
        Err(Refusal::NoPosition)
    ));
    let faint = DfBearing {
        confidence: 0.01,
        ..aimed("roof", HOME, 45.0, 3.0)
    };
    assert!(matches!(
        hub.observe("cross", &faint, 1.0, AT),
        Err(Refusal::Weak)
    ));
    let state = hub.state("cross").expect("configured");
    assert_eq!(state.refused, 2);
    assert_eq!(state.samples, 0);
    assert!(state.stations.is_empty());
}

#[test]
fn nav_auto_probes_along_the_averaged_bearing_then_targets_the_estimate() {
    let target = geo::destination(HOME, 60.0, 8_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    fusion.guide(Some(&fix_at(HOME)), 0.0);
    let mut at_s = 0.0;
    let mut first = None;
    for jitter in [-1.0, 1.0, -0.5, 0.5] {
        let outcome = see(&mut fusion, &aimed("car", HOME, 60.0 + jitter, 3.0), at_s);
        first.get_or_insert(outcome.state.nav);
        at_s += 1.0;
    }
    let probe = first.flatten().expect("a probe target");
    assert_eq!(probe.kind, NavTargetKind::Probe);
    assert!((probe.distance_m - 5_000.0).abs() < 1.0, "{probe:?}");
    let averaged = fusion.nav_target.expect("target");
    assert!((averaged.bearing_deg - 60.0).abs() < 0.2, "{averaged:?}");

    let tower = around(target, 150.0, 5_000.0);
    let mut kinds = Vec::new();
    for _ in 0..6 {
        see(&mut fusion, &toward("car", HOME, target, 1.0), at_s);
        let outcome = see(
            &mut fusion,
            &toward("tower", tower, target, 1.0),
            at_s + 0.5,
        );
        kinds.push(outcome.state.nav.map(|nav| nav.kind));
        at_s += 1.0;
    }
    let nav = fusion.nav_target.expect("target");
    assert_eq!(nav.kind, NavTargetKind::Estimate, "{kinds:?}");
    let aimed_at = LatLon {
        lat: nav.lat,
        lon: nav.lon,
    };
    assert!(geo::distance_m(aimed_at, target) < 300.0, "{nav:?}");
    assert!(nav.revision > probe.revision);
}

#[test]
fn nav_direct_targets_the_best_cell() {
    let target = geo::destination(HOME, 30.0, 5_000.0);
    let mut fusion = NodeFusion::new(&params(FusionDecay::Auto, NavMode::Direct));
    fusion.guide(Some(&fix_at(HOME)), 0.0);
    let other = geo::destination(HOME, 90.0, 5_000.0);
    for tick in 0..2u32 {
        let at_s = f64::from(tick);
        see(&mut fusion, &toward("home", HOME, target, 2.0), at_s);
        see(&mut fusion, &toward("east", other, target, 2.0), at_s + 0.5);
    }
    let nav = fusion.nav_target.expect("target");
    assert_eq!(nav.kind, NavTargetKind::Estimate);
    let found = estimate(&fusion);
    assert_eq!((nav.lat, nav.lon), (found.lat, found.lon));
}

#[test]
fn nav_off_publishes_no_target() {
    let target = geo::destination(HOME, 30.0, 5_000.0);
    let mut fusion = NodeFusion::new(&params(FusionDecay::Auto, NavMode::Off));
    fusion.guide(Some(&fix_at(HOME)), 0.0);
    let outcome = see(&mut fusion, &toward("home", HOME, target, 2.0), 1.0);
    assert!(outcome.state.nav.is_none());
    assert!(!outcome.state.no_guide_position);
}

#[test]
fn nav_revision_moves_only_on_real_changes() {
    let mut nav = Nav::new(NavMode::Auto, 5.0);
    let observation = prepare(&aimed("car", HOME, 60.0, 3.0), 0.0, 0.05).expect("accepted");
    nav.remember(&observation);
    let first = nav.update(Some(&fix_at(HOME)), None, 1.0).0.expect("probe");
    for (index, bearing) in [0.0, 90.0, 180.0, 270.0].into_iter().enumerate() {
        let jittered = geo::destination(HOME, bearing, 10.0);
        let next = nav
            .update(Some(&fix_at(jittered)), None, 2.0 + index as f64)
            .0
            .expect("probe");
        assert_eq!(next.revision, first.revision);
    }
    let driven = geo::destination(HOME, 180.0, 120.0);
    let moved = nav
        .update(Some(&fix_at(driven)), None, 10.0)
        .0
        .expect("probe");
    assert_eq!(moved.revision, first.revision + 1);
    let (gone, flags) = nav.update(None, None, 11.0);
    assert!(gone.is_none() && flags.no_guide_position);
    let back = nav
        .update(Some(&fix_at(driven)), None, 12.0)
        .0
        .expect("probe");
    assert_eq!(back.revision, moved.revision + 1);
}

#[test]
fn nav_without_a_guide_position_says_so() {
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    let outcome = see(&mut fusion, &aimed("roof", HOME, 45.0, 3.0), 0.0);
    assert!(outcome.state.no_guide_position);
    assert!(outcome.state.nav.is_none());
    let far = geo::destination(HOME, 180.0, 5_000.0);
    let lonely = fusion.guide(Some(&fix_at(far)), 1.0);
    assert!(!lonely.state.no_guide_position);
    assert!(lonely.state.no_bearings);
    assert!(lonely.state.nav.is_none());
}

#[test]
fn fusion_paint_stays_within_budget() {
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    see(&mut fusion, &aimed("roof", HOME, 45.0, 3.0), 0.0);
    let key = fusion.votes.keys.pop().expect("a key");
    let wedge = &key.painted.as_ref().expect("painted").wedge;
    let grid = fusion.grid.as_mut().expect("grid");
    let started = Instant::now();
    grid.paint(None, Some(wedge));
    let fresh = started.elapsed();
    assert!(fresh.as_millis() < 50, "{fresh:?}");
    let started = Instant::now();
    grid.paint(Some(wedge), Some(wedge));
    let swapped = started.elapsed();
    assert!(swapped.as_millis() < 50, "{swapped:?}");
}

#[test]
fn a_full_fusion_queue_counts_drops() {
    let hub = FusionHub::default();
    hub.configure("cross", &TriangulationParams::default());
    let (sender, held) = std::sync::mpsc::sync_channel(QUEUE_DEPTH);
    assert!(hub.install(sender));
    let accepted = (0..300u32)
        .filter(|tick| hub.enqueue(job(f64::from(*tick))))
        .count();
    assert_eq!(accepted, QUEUE_DEPTH);
    assert_eq!(hub.state("cross").expect("state").dropped, 44);
    drop(held);
    assert!(!hub.enqueue(job(301.0)));
    assert_eq!(hub.state("cross").expect("state").dropped, 45);
}

#[test]
fn a_bearing_without_a_running_thread_is_counted() {
    let hub = FusionHub::default();
    hub.configure("cross", &TriangulationParams::default());
    assert!(!hub.enqueue(job(0.0)));
    assert_eq!(hub.state("cross").expect("state").dropped, 1);
}

#[test]
fn a_fix_that_closes_up_is_announced_once() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let stations = [0.0, 120.0, 240.0].map(|from_deg| around(target, from_deg, 3_000.0));
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    let mut announced = Vec::new();
    for round in 0..10u32 {
        for (index, station) in stations.iter().enumerate() {
            let at_s = f64::from(round) * 2.0 + index as f64 * 0.5;
            let outcome = see(
                &mut fusion,
                &toward(&format!("s{index}"), *station, target, 0.5),
                at_s,
            );
            announced.extend(outcome.first_fix);
        }
    }
    assert_eq!(announced.len(), 1, "{announced:?}");
    assert!(announced[0].converged);
    assert!(miss_m(&announced[0], target) < 400.0);
}

#[test]
fn a_reset_forgets_bearings_but_keeps_the_guide() {
    let hub = FusionHub::default();
    hub.configure("cross", &TriangulationParams::default());
    hub.guide("cross", Some(&fix_at(HOME)), 0.0);
    for tick in 0..3u32 {
        hub.observe(
            "cross",
            &aimed("roof", HOME, 45.0, 3.0),
            f64::from(tick),
            AT,
        )
        .expect("accepted");
    }
    let before = hub.state("cross").expect("state");
    assert_eq!(before.samples, 3);
    assert!(before.nav.is_some());
    let cleared = hub.reset("cross").expect("configured");
    assert_eq!(cleared.state.samples, 0);
    assert!(cleared.state.stations.is_empty());
    assert!(cleared.state.estimate.is_none());
    assert!(!cleared.state.no_guide_position);
    assert!(cleared.state.no_bearings);
    assert!(cleared.first_fix.is_none());
    let blank = cleared.grid_frame.expect("viewers see the grid emptied");
    assert!(blank.cells.iter().all(|cell| *cell == 0));
    assert!(blank.north > blank.south);
    assert!(hub.due_frames(1e9).is_empty(), "nothing left to paint");
    assert!(hub.reset("nowhere").is_none());
    assert!(matches!(
        hub.observe("nowhere", &aimed("roof", HOME, 45.0, 3.0), 4.0, AT),
        Err(Refusal::Unknown)
    ));
    assert!(hub.state("nowhere").is_none());
}

#[test]
fn a_frame_is_sent_at_most_once_a_second_with_the_north_row_first() {
    let target = geo::destination(HOME, 0.0, 4_000.0);
    let east = geo::destination(HOME, 90.0, 4_000.0);
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    let mut frames = Vec::new();
    for tick in 0..8u32 {
        let at_s = f64::from(tick) * 0.25;
        let (name, station) = if tick % 2 == 0 {
            ("home", HOME)
        } else {
            ("east", east)
        };
        frames.extend(see(&mut fusion, &toward(name, station, target, 2.0), at_s).grid_frame);
    }
    assert_eq!(frames.len(), 2, "one frame per second");
    let frame = frames.last().expect("frame");
    assert_eq!(usize::from(frame.cols), FRAME_CELLS);
    assert_eq!(frame.cells.len(), FRAME_CELLS * FRAME_CELLS);
    assert!(frame.north > frame.south && frame.east > frame.west);
    let brightest = frame
        .cells
        .iter()
        .enumerate()
        .max_by_key(|(_, value)| **value)
        .map(|(index, _)| index)
        .expect("cells");
    assert!(
        brightest / FRAME_CELLS < FRAME_CELLS / 2,
        "north lies on top"
    );
    assert_eq!(frame.cells[brightest], 255);
}

#[test]
fn a_held_back_frame_is_sent_once_the_second_is_up() {
    let hub = FusionHub::default();
    hub.configure("cross", &TriangulationParams::default());
    let target = geo::destination(HOME, 0.0, 4_000.0);
    let east = geo::destination(HOME, 90.0, 4_000.0);
    let first = hub
        .observe("cross", &toward("home", HOME, target, 2.0), 10.0, AT)
        .expect("accepted");
    assert!(first.grid_frame.is_some());
    let second = hub
        .observe("cross", &toward("east", east, target, 2.0), 10.4, AT)
        .expect("accepted");
    assert!(second.grid_frame.is_none(), "held back");
    assert!(hub.due_frames(10.8).is_empty());
    let flushed = hub.due_frames(11.0);
    assert_eq!(flushed.len(), 1);
    assert_eq!(flushed[0].0, "cross");
    assert_eq!(flushed[0].1.seq, 1);
    assert!(hub.due_frames(15.9).is_empty(), "sent once");
    let refreshed = hub.due_frames(16.0);
    assert_eq!(refreshed.len(), 1, "late viewers get the grid again");
    assert_eq!(refreshed[0].1.cells, flushed[0].1.cells);
}

#[test]
fn a_grid_without_evidence_is_sent_dark() {
    let grid = LogGrid::new(HOME, 12.8);
    let frame = grid.frame(0, 0);
    assert_eq!(frame.cells.len(), FRAME_CELLS * FRAME_CELLS);
    assert!(frame.cells.iter().all(|cell| *cell == 0));
}

#[test]
fn a_guide_never_announces_a_fix() {
    let target = geo::destination(HOME, 45.0, 6_000.0);
    let stations = [0.0, 120.0, 240.0].map(|from_deg| around(target, from_deg, 3_000.0));
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    for round in 0..10u32 {
        for (index, station) in stations.iter().enumerate() {
            let at_s = f64::from(round) * 2.0 + index as f64 * 0.5;
            fusion
                .observe(
                    &toward(&format!("s{index}"), *station, target, 0.5),
                    at_s,
                    AT,
                )
                .expect("accepted");
        }
    }
    fusion.announced = false;
    assert!(fusion.guide(Some(&fix_at(HOME)), 30.0).first_fix.is_none());
    let next = see(&mut fusion, &toward("s0", stations[0], target, 0.5), 31.0);
    assert!(next.first_fix.is_some(), "the next bearing announces it");
}

fn triangulation_graph(extent_km: f64) -> PatchGraph {
    let settings = TriangulationParams {
        extent_km,
        ..TriangulationParams::default()
    };
    PatchGraph {
        nodes: vec![sdrmm_wire::PatchNode {
            id: "cross".to_owned(),
            body: NodeBody::Triangulation(sdrmm_wire::TriangulationNode { settings }),
            position: sdrmm_wire::Position { x: 0.0, y: 0.0 },
            size: None,
            label: None,
        }],
        edges: Vec::new(),
    }
}

#[tokio::test]
async fn a_triangulation_with_bad_settings_stops_fusing() {
    let store = Arc::new(crate::Store::open(None).expect("store"));
    let state = crate::tests::state_over(store);
    assert!(reconcile(&state, &triangulation_graph(12.8)).is_empty());
    assert!(state.fusion.state("cross").is_some());
    assert!(state.surfaces.subscribe("cross").is_some());
    let refused = reconcile(&state, &triangulation_graph(500.0));
    assert_eq!(
        refused,
        [("cross".to_owned(), "Extent out of range".to_owned())]
    );
    assert!(state.fusion.state("cross").is_none());
    assert!(state.surfaces.subscribe("cross").is_none());
    assert!(matches!(
        state
            .fusion
            .observe("cross", &aimed("roof", HOME, 45.0, 3.0), 0.0, AT),
        Err(Refusal::Unknown)
    ));
}

#[test]
fn a_guide_walking_off_the_grid_moves_it_even_with_one_station() {
    let mut fusion = NodeFusion::new(&TriangulationParams::default());
    see(&mut fusion, &aimed("roof", HOME, 45.0, 3.0), 0.0);
    let walked = geo::destination(HOME, 90.0, 11_000.0);
    fusion.guide(Some(&fix_at(walked)), 10.0);
    let (east, north) = fusion.grid.as_ref().expect("grid").centre();
    assert!((east - 11_000.0).abs() < 300.0, "{east} m east");
    assert!(north.abs() < 300.0, "{north} m north");
}
