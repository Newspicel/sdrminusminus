use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use futures::future::BoxFuture;
use sdrmm_wire::{
    array::{ArrayStatus, CalPhase},
    frame::{FusionGridOwned, RangeDopplerOwned},
    fusion::{DfEstimate, DfFusionState, NavTarget, NavTargetKind},
    hunt::{HuntSettings, HuntStatus},
    mission::{MissionAction, MissionActionResponse, MissionControl as Wire, MissionsResponse},
    phone::{Phone, PhonePlatform, PhoneSelf},
    processor::{ProcessorReading, df::DfReading},
    radar::RadarUpdate,
    state::StateSnapshot,
    survey::{MAX_SURVEY_CELLS, SurveyCell, SurveyGrid, SurveyUpdate},
    ws::{ServerEvent, StateScope, StreamKind},
};
use tokio::sync::{mpsc, watch};

use super::{
    array::tests as array_tests,
    listing::tests::{df, hunt, radar, response, survey, triangulation},
    reducer::{Effect, Input, RADAR_FIT, Reducer, Seed, SeedRequest},
    views::{DfState, GuidanceKind, RetargetReason, TargetMode},
    *,
};
use crate::{
    link::{Session, rest::Api},
    pose::PoseSnapshot,
    records::LatLon,
};

const PHONE: &str = "p0123456789abcdef";
const T0: i64 = 1_790_000_000_000;

fn emitted(effects: &[Effect]) -> Vec<&CoreEvent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Emit(event) => Some(event.as_ref()),
            _ => None,
        })
        .collect()
}

fn notices(effects: &[Effect]) -> Vec<String> {
    emitted(effects)
        .into_iter()
        .filter_map(|event| match event {
            CoreEvent::Notice { notice } => Some(notice.text.clone()),
            _ => None,
        })
        .collect()
}

fn listed(missions: Vec<sdrmm_wire::mission::Mission>) -> Reducer {
    let mut reducer = Reducer::new();
    reducer.handle(
        Input::Live {
            phone_id: PHONE.to_owned(),
        },
        T0,
    );
    reducer.handle(Input::Listing(Box::new(response(missions))), T0);
    reducer
}

#[test]
fn a_cut_listing_says_how_many_missions_are_missing() {
    let mut reducer = listed(vec![hunt("hunt1", &[Wire::StartHunt], &[])]);
    let mut cut = response(vec![hunt("hunt1", &[Wire::StartHunt], &[])]);
    cut.truncated = 3;
    let effects = reducer.handle(Input::Listing(Box::new(cut.clone())), T0);
    assert_eq!(notices(&effects), ["3 missions not listed"]);
    let again = reducer.handle(Input::Listing(Box::new(cut.clone())), T0);
    assert!(notices(&again).is_empty());
    cut.truncated = 0;
    let whole = reducer.handle(Input::Listing(Box::new(cut)), T0);
    assert!(notices(&whole).is_empty());
}

fn event(event: ServerEvent) -> Input {
    Input::Event(Box::new(event))
}

fn hunt_status(channel: u32) -> HuntStatus {
    HuntStatus {
        settings: HuntSettings::for_channel(channel),
        freq_hz: 145.5e6,
        bw_hz: 12_500.0,
        level_db: Some(-60.0),
        smooth_db: Some(-61.0),
        floor_db: Some(-90.0),
        best_db: Some(-55.0),
        strength: 0.95,
        closing: false,
        readings: 5,
        at_ms: 0,
        pose_drops: 0,
        sweep: None,
        error: None,
    }
}

fn fused(kind: NavTargetKind, north_m: f64) -> DfFusionState {
    let at = sdrmm_wire::geo::offset_m(
        sdrmm_wire::geo::LatLon {
            lat: 52.52,
            lon: 13.405,
        },
        0.0,
        north_m,
    );
    DfFusionState {
        estimate: Some(DfEstimate {
            lat: at.lat,
            lon: at.lon,
            ellipse_major_m: 400.0,
            ellipse_minor_m: 100.0,
            ellipse_bearing_deg: 10.0,
            converged: true,
            samples: 8,
            mass: 0.9,
        }),
        nav: Some(NavTarget {
            lat: at.lat,
            lon: at.lon,
            kind,
            revision: 1,
            distance_m: north_m,
            bearing_deg: 0.0,
        }),
        samples: 8,
        ..DfFusionState::default()
    }
}

fn radar_frame(stream_id: u16) -> Vec<u8> {
    RangeDopplerOwned {
        stream_id,
        seq: 1,
        timestamp: 0,
        ranges: 4,
        dopplers: 2,
        range_first_m: 0.0,
        range_step_m: 300.0,
        doppler_first_hz: -2.0,
        doppler_step_hz: 2.0,
        carrier_hz: 98.1e6,
        db_min: 0.0,
        db_max: 30.0,
        cells: vec![0, 10, 20, 30, 40, 50, 60, 255],
    }
    .frame()
    .encode()
}

fn grid_frame(stream_id: u16) -> Vec<u8> {
    let mut cells = vec![0u8; 16];
    cells[5] = 255;
    cells[6] = 200;
    FusionGridOwned {
        stream_id,
        seq: 1,
        timestamp: 0,
        south: 52.0,
        west: 13.0,
        north: 53.0,
        east: 14.0,
        cols: 4,
        rows: 4,
        cells,
    }
    .frame()
    .encode()
}

#[test]
fn opening_radar_subscribes_the_surface_and_closing_unsubscribes() {
    let mut reducer = listed(vec![radar("pr1")]);
    let effects = reducer.handle(Input::Open("pr1".to_owned()), T0);
    assert!(effects.contains(&Effect::Seed {
        mission: "pr1".to_owned(),
        what: SeedRequest::Radar("pr1".to_owned())
    }));
    assert!(matches!(
        emitted(&effects).as_slice(),
        [CoreEvent::Radar { view }] if view.stale && view.mission == "pr1"
    ));
    assert_eq!(
        reducer.subscriptions(),
        Subscriptions::from([("pr1".to_owned(), Some(RADAR_FIT))])
    );
    reducer.handle(Input::Close, T0);
    assert!(reducer.subscriptions().is_empty());
}

#[test]
fn opening_df_seeds_from_state_and_subscribes_the_grid() {
    let mut reducer = listed(vec![
        df("df1", &["tri1"]),
        triangulation("tri1", &["df1"], true),
    ]);
    let effects = reducer.handle(Input::Open("df1".to_owned()), T0);
    assert!(effects.contains(&Effect::Seed {
        mission: "df1".to_owned(),
        what: SeedRequest::Fusion("tri1".to_owned())
    }));
    assert_eq!(
        reducer.subscriptions(),
        Subscriptions::from([("tri1".to_owned(), None)])
    );
    let seeded = reducer.handle(
        Input::Seeded {
            mission: "df1".to_owned(),
            seed: Seed::Fusion(Box::new(fused(NavTargetKind::Estimate, 1_000.0))),
        },
        T0,
    );
    let view = emitted(&seeded)
        .into_iter()
        .find_map(|event| match event {
            CoreEvent::Df { view } => Some(view.clone()),
            _ => None,
        })
        .expect("df view");
    assert!(view.estimate.is_some_and(|estimate| estimate.converged));
    assert_eq!(view.overlay.ellipse.len(), 49);
    assert_eq!(view.state, DfState::Waiting);
    assert!(matches!(
        emitted(&seeded)[0],
        CoreEvent::Retarget { notice } if notice.reason == RetargetReason::First
    ));
}

fn df_state(effects: &[Effect]) -> Option<DfState> {
    emitted(effects).into_iter().find_map(|event| match event {
        CoreEvent::Df { view } => Some(view.state),
        _ => None,
    })
}

#[test]
fn opening_df_seeds_the_array_gate_until_the_array_speaks() {
    let mut reducer = listed(vec![df("df1", &[])]);
    let effects = reducer.handle(Input::Open("df1".to_owned()), T0);
    assert!(effects.contains(&Effect::Seed {
        mission: "df1".to_owned(),
        what: SeedRequest::Arrays
    }));
    let mut calibrating = array_tests::status("arr");
    calibrating.cal = CalPhase::Measuring;
    let mut unknown = array_tests::status("arr");
    unknown.phase_ready = false;
    let seed = |statuses: Vec<ArrayStatus>| Input::Seeded {
        mission: "df1".to_owned(),
        seed: Seed::Arrays(statuses),
    };
    let other = array_tests::status("other");
    assert_eq!(
        df_state(&reducer.handle(seed(vec![other, calibrating]), T0)),
        Some(DfState::Calibrating)
    );
    let updated = reducer.handle(
        event(ServerEvent::ArrayUpdate {
            status: Box::new(unknown),
        }),
        T0,
    );
    assert_eq!(df_state(&updated), Some(DfState::PhaseUnknown));
    let late = reducer.handle(seed(vec![array_tests::status("arr")]), T0);
    assert_eq!(df_state(&late), None);
}

#[test]
fn a_vanished_mission_is_closed_with_a_notice() {
    let mut reducer = listed(vec![hunt("hunt1", &[Wire::StartHunt], &[]), survey("map1")]);
    reducer.handle(Input::Open("hunt1".to_owned()), T0);
    let effects = reducer.handle(Input::Listing(Box::new(response(vec![survey("map1")]))), T0);
    assert_eq!(notices(&effects), ["Mission gone"]);
    assert_eq!(reducer.open_id(), None);
    assert!(
        matches!(emitted(&effects)[0], CoreEvent::Missions { view } if view.missions.len() == 1)
    );
}

#[test]
fn background_withdraws_heavy_streams_and_foreground_restores_them() {
    let mut reducer = listed(vec![radar("pr1")]);
    reducer.handle(Input::Open("pr1".to_owned()), T0);
    reducer.handle(
        event(ServerEvent::SurfaceStreamStarted {
            stream_id: 5,
            node: "pr1".to_owned(),
            kind: StreamKind::RangeDoppler,
        }),
        T0,
    );
    reducer.handle(Input::Background(true), T0);
    assert!(reducer.subscriptions().is_empty());
    assert!(emitted(&reducer.handle(Input::Frame(radar_frame(5)), T0)).is_empty());
    reducer.handle(Input::Background(false), T0);
    assert_eq!(reducer.subscriptions().len(), 1);
    let effects = reducer.handle(Input::Frame(radar_frame(5)), T0);
    assert!(matches!(
        emitted(&effects).as_slice(),
        [CoreEvent::RadarImage { image }] if image.width == 4 && image.height == 2
    ));
    assert!(emitted(&reducer.handle(Input::Frame(radar_frame(9)), T0)).is_empty());
    reducer.handle(
        event(ServerEvent::StreamStopped {
            stream_id: 5,
            kind: StreamKind::RangeDoppler,
        }),
        T0,
    );
    assert!(emitted(&reducer.handle(Input::Frame(radar_frame(5)), T0)).is_empty());
}

#[test]
fn state_changed_refetches_missions_once_per_300_ms() {
    let mut reducer = listed(vec![]);
    let scopes = [
        StateScope::Missions,
        StateScope::All,
        StateScope::Workspaces,
    ];
    for (step, scope) in scopes.into_iter().enumerate() {
        reducer.handle(
            event(ServerEvent::StateChanged { scope }),
            T0 + step as i64 * 50,
        );
    }
    assert!(
        !reducer
            .handle(Input::Tick, T0 + 299)
            .contains(&Effect::FetchListing)
    );
    assert!(
        reducer
            .handle(Input::Tick, T0 + 300)
            .contains(&Effect::FetchListing)
    );
    assert!(
        !reducer
            .handle(Input::Tick, T0 + 400)
            .contains(&Effect::FetchListing)
    );
    let phones = reducer.handle(
        event(ServerEvent::StateChanged {
            scope: StateScope::Phones,
        }),
        T0 + 500,
    );
    assert!(phones.contains(&Effect::FetchSelf));
    assert!(
        reducer
            .handle(Input::Tick, T0 + 800)
            .contains(&Effect::FetchListing)
    );
    reducer.handle(
        event(ServerEvent::StateChanged {
            scope: StateScope::Devices,
        }),
        T0 + 900,
    );
    assert!(
        !reducer
            .handle(Input::Tick, T0 + 1_300)
            .contains(&Effect::FetchListing)
    );
}

#[test]
fn a_workspace_change_refetches_the_phone_with_the_listing() {
    let mut reducer = listed(vec![]);
    let scope = |scope| event(ServerEvent::StateChanged { scope });
    assert!(
        !reducer
            .handle(scope(StateScope::Missions), T0)
            .contains(&Effect::FetchSelf)
    );
    assert!(
        !reducer
            .handle(Input::Tick, T0 + 300)
            .contains(&Effect::FetchSelf)
    );
    for step in 0..3 {
        let effects = reducer.handle(scope(StateScope::Workspaces), T0 + 400 + step * 10);
        assert!(!effects.contains(&Effect::FetchSelf));
    }
    let fired = reducer.handle(Input::Tick, T0 + 700);
    assert!(fired.contains(&Effect::FetchListing));
    assert_eq!(
        fired
            .iter()
            .filter(|effect| **effect == Effect::FetchSelf)
            .count(),
        1
    );
    reducer.handle(scope(StateScope::Missions), T0 + 800);
    assert!(
        !reducer
            .handle(Input::Tick, T0 + 1_100)
            .contains(&Effect::FetchSelf)
    );
}

#[test]
fn link_down_marks_radar_stale_and_live_seeds_again() {
    let mut reducer = listed(vec![radar("pr1")]);
    reducer.handle(Input::Open("pr1".to_owned()), T0);
    let update = RadarUpdate {
        detections: vec![Default::default(); 2],
        ..RadarUpdate::default()
    };
    reducer.handle(
        Input::Seeded {
            mission: "pr1".to_owned(),
            seed: Seed::Radar(Box::new(update)),
        },
        T0,
    );
    let effects = reducer.handle(Input::Down, T0 + 10);
    assert!(matches!(
        emitted(&effects).as_slice(),
        [CoreEvent::Radar { view }] if view.stale && view.echoes == 2
    ));
    let live = reducer.handle(
        Input::Live {
            phone_id: PHONE.to_owned(),
        },
        T0 + 20,
    );
    assert!(live.contains(&Effect::FetchListing));
    assert!(live.contains(&Effect::Seed {
        mission: "pr1".to_owned(),
        what: SeedRequest::Radar("pr1".to_owned())
    }));
}

#[test]
fn radar_readings_refresh_and_go_stale_after_5_s() {
    let mut reducer = listed(vec![radar("pr1")]);
    reducer.handle(Input::Open("pr1".to_owned()), T0);
    let fresh = reducer.handle(
        event(ServerEvent::ProcessorUpdate {
            node: "pr1".to_owned(),
            reading: Box::new(ProcessorReading::PassiveRadar(RadarUpdate::default())),
        }),
        T0,
    );
    assert!(matches!(emitted(&fresh).as_slice(), [CoreEvent::Radar { view }] if !view.stale));
    assert!(emitted(&reducer.handle(Input::Tick, T0 + 4_000)).is_empty());
    let stale = reducer.handle(Input::Tick, T0 + 5_100);
    assert!(matches!(emitted(&stale).as_slice(), [CoreEvent::Radar { view }] if view.stale));
}

#[test]
fn hunt_updates_for_another_channel_are_ignored() {
    let mut reducer = listed(vec![hunt("hunt1", &[Wire::StopHunt], &[])]);
    let opened = reducer.handle(Input::Open("hunt1".to_owned()), T0);
    assert!(matches!(
        emitted(&opened).as_slice(),
        [CoreEvent::Hunt { view }] if view.running && view.readings == 0
    ));
    let other = reducer.handle(
        event(ServerEvent::HuntUpdate {
            device_set: 1,
            status: Box::new(hunt_status(4)),
        }),
        T0,
    );
    assert!(emitted(&other).is_empty());
    let mine = reducer.handle(
        event(ServerEvent::HuntUpdate {
            device_set: 1,
            status: Box::new(hunt_status(3)),
        }),
        T0,
    );
    assert!(matches!(
        emitted(&mine).as_slice(),
        [CoreEvent::Hunt { view }] if view.readings == 5 && view.trend == super::views::Trend::OnTop
    ));
    let elsewhere = reducer.handle(
        event(ServerEvent::HuntUpdate {
            device_set: 2,
            status: Box::new(hunt_status(3)),
        }),
        T0,
    );
    assert!(emitted(&elsewhere).is_empty());
}

#[test]
fn fusion_grids_become_heat_bands_at_most_once_a_second() {
    let mut reducer = listed(vec![
        df("df1", &["tri1"]),
        triangulation("tri1", &["df1"], true),
    ]);
    reducer.handle(Input::Open("df1".to_owned()), T0);
    reducer.handle(
        event(ServerEvent::SurfaceStreamStarted {
            stream_id: 7,
            node: "tri1".to_owned(),
            kind: StreamKind::FusionGrid,
        }),
        T0,
    );
    let first = reducer.handle(Input::Frame(grid_frame(7)), T0);
    assert!(emitted(&first).iter().any(|event| matches!(
        event,
        CoreEvent::Df { view } if !view.overlay.heat.is_empty()
    )));
    assert!(emitted(&reducer.handle(Input::Frame(grid_frame(7)), T0 + 500)).is_empty());
    assert!(!emitted(&reducer.handle(Input::Frame(grid_frame(7)), T0 + 1_000)).is_empty());
    let mut broken = grid_frame(7);
    broken.pop();
    assert_eq!(
        notices(&reducer.handle(Input::Frame(broken), T0 + 3_000)),
        ["Bad frame from server"]
    );
}

#[test]
fn survey_updates_add_points() {
    let mut reducer = listed(vec![survey("map1")]);
    let opened = reducer.handle(Input::Open("map1".to_owned()), T0);
    assert!(opened.contains(&Effect::Seed {
        mission: "map1".to_owned(),
        what: SeedRequest::Survey("map1".to_owned())
    }));
    let cell = SurveyCell {
        latitude: 52.5,
        longitude: 13.4,
        frequency_hz: 145.5e6,
        level_dbfs: -48.0,
        measured_at: "2026-09-28T12:00:00Z".to_owned(),
        observations: 1,
        accuracy_m: None,
    };
    let seeded = reducer.handle(
        Input::Seeded {
            mission: "map1".to_owned(),
            seed: Seed::Survey(Box::new(SurveyGrid {
                node: "map1".to_owned(),
                frequency_hz: Some(145.5e6),
                offset_hz: 0,
                bandwidth_hz: 12_500,
                recording: true,
                cells: vec![cell.clone()],
                dropped: 0,
            })),
        },
        T0,
    );
    assert!(matches!(
        emitted(&seeded)[0],
        CoreEvent::SurveyPoints { points } if points.len() == 1
    ));
    let update = reducer.handle(
        event(ServerEvent::SurveyUpdate {
            node: "map1".to_owned(),
            update: Box::new(SurveyUpdate {
                level_dbfs: Some(-40.0),
                target_hz: None,
                recording: true,
                cells: 2,
                dropped: 0,
                cell: Some(cell),
                stopped: None,
            }),
        }),
        T0,
    );
    assert!(matches!(
        emitted(&update).as_slice(),
        [CoreEvent::SurveyPoints { points }, CoreEvent::Survey { view }]
            if points.len() == 1 && view.total == 2 && view.level_db == Some(-40.0)
    ));
}

#[test]
fn a_reconnect_seeds_only_unseen_survey_cells() {
    let mut reducer = listed(vec![survey("map1")]);
    reducer.handle(Input::Open("map1".to_owned()), T0);
    let cell = |longitude: f64| SurveyCell {
        latitude: 52.5,
        longitude,
        frequency_hz: 145.5e6,
        level_dbfs: -48.0,
        measured_at: "2026-09-28T12:00:00Z".to_owned(),
        observations: 1,
        accuracy_m: None,
    };
    let seed = |cells: Vec<SurveyCell>| Input::Seeded {
        mission: "map1".to_owned(),
        seed: Seed::Survey(Box::new(SurveyGrid {
            node: "map1".to_owned(),
            frequency_hz: Some(145.5e6),
            offset_hz: 0,
            bandwidth_hz: 12_500,
            recording: true,
            cells,
            dropped: 0,
        })),
    };
    reducer.handle(seed(vec![cell(13.4)]), T0);
    reducer.handle(Input::Down, T0);
    let relive = reducer.handle(
        Input::Live {
            phone_id: PHONE.to_owned(),
        },
        T0,
    );
    assert!(relive.contains(&Effect::Seed {
        mission: "map1".to_owned(),
        what: SeedRequest::Survey("map1".to_owned())
    }));
    let same = reducer.handle(seed(vec![cell(13.4)]), T0);
    assert!(matches!(
        emitted(&same).as_slice(),
        [CoreEvent::Survey { view }] if view.total == 1
    ));
    let grown = reducer.handle(seed(vec![cell(13.4), cell(13.5)]), T0);
    assert!(matches!(
        emitted(&grown).as_slice(),
        [CoreEvent::SurveyPoints { points }, CoreEvent::Survey { view }]
            if points.len() == 1 && points[0].at.lon == 13.5 && view.total == 2
    ));
}

#[test]
fn a_full_survey_grid_reaches_the_app_without_loss() {
    let mut reducer = listed(vec![survey("map1")]);
    reducer.handle(Input::Open("map1".to_owned()), T0);
    let cells: Vec<SurveyCell> = (0..MAX_SURVEY_CELLS)
        .map(|index| SurveyCell {
            latitude: 52.5,
            longitude: 13.0 + index as f64 * 1e-4,
            frequency_hz: 145.5e6,
            level_dbfs: -60.0,
            measured_at: "2026-09-28T12:00:00Z".to_owned(),
            observations: 1,
            accuracy_m: None,
        })
        .collect();
    let seeded = reducer.handle(
        Input::Seeded {
            mission: "map1".to_owned(),
            seed: Seed::Survey(Box::new(SurveyGrid {
                node: "map1".to_owned(),
                frequency_hz: Some(145.5e6),
                offset_hz: 0,
                bandwidth_hz: 12_500,
                recording: false,
                cells,
                dropped: 0,
            })),
        },
        T0,
    );
    let queue = EventQueue::default();
    for event in emitted(&seeded) {
        queue.emit(event.clone());
    }
    let mut delivered = Vec::new();
    let mut clock = std::time::Instant::now();
    while let crate::events::Pop::Event(event) = queue.pop(clock) {
        delivered.push(*event);
        clock += Duration::from_secs(1);
    }
    assert!(
        delivered
            .iter()
            .all(|event| !matches!(event, CoreEvent::Notice { .. })),
        "{delivered:?}"
    );
    assert!(delivered.iter().any(
        |event| matches!(event, CoreEvent::SurveyPoints { points } if points.len() == MAX_SURVEY_CELLS)
    ));
}

#[test]
fn a_df_reading_projects_and_retargets_once() {
    let mut reducer = listed(vec![
        df("df1", &["tri1"]),
        triangulation("tri1", &["df1"], true),
    ]);
    reducer.handle(Input::Open("df1".to_owned()), T0);
    reducer.handle(
        Input::Pose(Some(PoseSnapshot {
            at: LatLon {
                lat: 52.52,
                lon: 13.405,
            },
            heading_deg: Some(90.0),
            t_ms: T0,
        })),
        T0,
    );
    let fusion = |north_m| {
        event(ServerEvent::DfFusionUpdate {
            node: "tri1".to_owned(),
            state: Box::new(fused(NavTargetKind::Probe, north_m)),
        })
    };
    let first = reducer.handle(fusion(2_000.0), T0 + 1_000);
    let retargets = |effects: &[Effect]| {
        emitted(effects)
            .into_iter()
            .filter(|event| matches!(event, CoreEvent::Retarget { .. }))
            .count()
    };
    assert_eq!(retargets(&first), 1);
    assert_eq!(retargets(&reducer.handle(fusion(2_100.0), T0 + 40_000)), 0);
    assert_eq!(retargets(&reducer.handle(fusion(3_000.0), T0 + 40_000)), 1);
    let reading = DfReading {
        peaks: vec![sdrmm_wire::processor::df::DfPeak {
            true_deg: Some(30.0),
            confidence: 0.9,
            sigma_deg: 3.0,
            ..Default::default()
        }],
        azimuth_deg: Some(10.0),
        ..DfReading::default()
    };
    let effects = reducer.handle(
        event(ServerEvent::ProcessorUpdate {
            node: "df1".to_owned(),
            reading: Box::new(ProcessorReading::Df(reading)),
        }),
        T0 + 41_000,
    );
    let view = emitted(&effects)
        .into_iter()
        .find_map(|event| match event {
            CoreEvent::Df { view } => Some(view.clone()),
            _ => None,
        })
        .expect("df view");
    assert_eq!(view.state, DfState::Live);
    assert_eq!(view.bearing_rel_deg, Some(300.0));
    assert!(
        view.guidance
            .is_some_and(|guide| (guide.distance_m - 3_000.0).abs() < 2.0)
    );
    let targeted = reducer.handle(Input::TargetMode(TargetMode::Direct), T0 + 42_000);
    assert!(emitted(&targeted).iter().any(|event| matches!(
        event,
        CoreEvent::Retarget { notice } if notice.reason == RetargetReason::KindChanged
            && notice.target.kind == GuidanceKind::Estimate
    )));
}

#[test]
fn pose_needed_follows_the_listing_and_phone_self() {
    let mut reducer = listed(vec![df("df1", &[])]);
    assert!(!reducer.pose_needed());
    reducer.handle(
        Input::PhoneSelf(Box::new(PhoneSelf {
            phone: Phone {
                id: PHONE.to_owned(),
                name: "Pixel".to_owned(),
                platform: PhonePlatform::Android,
                created_at: "2026-09-28T12:00:00Z".to_owned(),
                last_seen: None,
                online: true,
                gps_nodes: vec!["gps1".to_owned()],
            },
            server_id: "00".to_owned(),
            server_name: "Shack".to_owned(),
        })),
        T0,
    );
    assert!(reducer.pose_needed());
    let wanted = listed(vec![hunt("hunt1", &[], &[])]);
    assert!(wanted.pose_needed());
}

#[test]
fn acted_missions_update_the_listing_and_the_open_view() {
    let mut reducer = listed(vec![hunt("hunt1", &[Wire::StartHunt], &[])]);
    reducer.handle(Input::Open("hunt1".to_owned()), T0);
    let effects = reducer.handle(
        Input::Acted(Box::new(hunt("hunt1", &[Wire::StopHunt], &[]))),
        T0,
    );
    assert!(matches!(emitted(&effects)[0], CoreEvent::Missions { .. }));
    assert!(matches!(
        emitted(&effects)[1],
        CoreEvent::Hunt { view } if view.running
    ));
}

#[test]
fn a_listing_failure_is_noticed_once_and_retried() {
    let mut reducer = listed(vec![]);
    let failed = reducer.handle(Input::ListingFailed("timed out".to_owned()), T0);
    assert_eq!(notices(&failed), ["Missions unavailable"]);
    assert!(notices(&reducer.handle(Input::ListingFailed("again".to_owned()), T0)).is_empty());
    assert!(
        reducer
            .handle(Input::Tick, T0 + 10_000)
            .contains(&Effect::FetchListing)
    );
    let error = reducer.handle(
        event(ServerEvent::Error {
            message: "pose updates are limited to 20 Hz".to_owned(),
        }),
        T0,
    );
    assert_eq!(notices(&error), ["pose updates are limited to 20 Hz"]);
}

#[derive(Default)]
struct FakeApi {
    actions: Mutex<Vec<(String, MissionAction)>>,
}

impl Api for FakeApi {
    fn missions(&self) -> BoxFuture<'_, Result<MissionsResponse, crate::link::rest::RestError>> {
        Box::pin(async { Ok(response(vec![hunt("hunt1", &[Wire::StartHunt], &[])])) })
    }

    fn phone_self(&self) -> BoxFuture<'_, Result<PhoneSelf, crate::link::rest::RestError>> {
        Box::pin(async { Err(crate::link::rest::RestError::TimedOut) })
    }

    fn act(
        &self,
        node: String,
        action: MissionAction,
    ) -> BoxFuture<'_, Result<MissionActionResponse, crate::link::rest::RestError>> {
        self.actions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((node, action));
        Box::pin(async {
            Ok(MissionActionResponse {
                mission: hunt("hunt1", &[Wire::StopHunt], &[]),
            })
        })
    }

    fn switch_workspace(
        &self,
        _id: i64,
    ) -> BoxFuture<'_, Result<MissionsResponse, crate::link::rest::RestError>> {
        Box::pin(async { Ok(response(Vec::new())) })
    }

    fn radar(
        &self,
        _node: String,
    ) -> BoxFuture<'_, Result<RadarUpdate, crate::link::rest::RestError>> {
        Box::pin(async { Ok(RadarUpdate::default()) })
    }

    fn survey(
        &self,
        _node: String,
    ) -> BoxFuture<'_, Result<SurveyGrid, crate::link::rest::RestError>> {
        Box::pin(async {
            Err(crate::link::rest::RestError::Status {
                status: 404,
                message: "No survey map1".to_owned(),
            })
        })
    }

    fn fusion(
        &self,
        _node: String,
    ) -> BoxFuture<'_, Result<DfFusionState, crate::link::rest::RestError>> {
        Box::pin(async { Ok(DfFusionState::default()) })
    }

    fn state(&self) -> BoxFuture<'_, Result<StateSnapshot, crate::link::rest::RestError>> {
        Box::pin(async {
            Ok(StateSnapshot {
                device_sets: Vec::new(),
                trunk_systems: Vec::new(),
                arrays: Vec::new(),
                revision: 1,
            })
        })
    }

    fn unpair(&self) -> BoxFuture<'_, Result<(), crate::link::rest::RestError>> {
        Box::pin(async { Ok(()) })
    }
}

#[test]
fn the_hub_fetches_missions_when_live_and_publishes_its_state() {
    let runtime = crate::runtime::CoreRuntime::start().expect("runtime");
    let events = EventQueue::default();
    let (inbound_tx, inbound) = mpsc::channel(16);
    let (_pose_tx, pose) = watch::channel(None);
    let (subs, _subs_rx) = watch::channel(Subscriptions::new());
    let (needed, needed_rx) = watch::channel(false);
    let (activity, activity_rx) = watch::channel(crate::link::Activity::default());
    let hub = start(
        &runtime,
        MissionWires {
            events: events.clone(),
            inbound,
            pose,
            subs,
            needed,
            activity,
        },
    );
    let api = Arc::new(FakeApi::default());
    let session = Arc::new(Session::new(
        "Shack".to_owned(),
        PHONE.to_owned(),
        "127.0.0.1:8443".to_owned(),
        api.clone(),
    ));
    inbound_tx
        .try_send(Inbound::Live(session))
        .map_err(|_| "full")
        .expect("sent");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while hub.shared().find("hunt1").is_none() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(hub.shared().find("hunt1").is_some());
    assert!(*needed_rx.borrow());
    hub.open(Some("hunt1".to_owned()));
    assert_eq!(hub.shared().open.as_deref(), Some("hunt1"));
    while !activity_rx.borrow().mission_open && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(activity_rx.borrow().mission_open);
    hub.open(Some("ghost".to_owned()));
    assert_eq!(hub.shared().open.as_deref(), Some("ghost"));
    while hub.shared().open.as_deref() == Some("ghost") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(hub.shared().open.as_deref(), Some("hunt1"));
    let mut seen = Vec::new();
    let mut clock = std::time::Instant::now();
    while seen.len() < 2 && std::time::Instant::now() < deadline {
        clock += Duration::from_secs(1);
        if let crate::events::Pop::Event(event) = events.pop(clock) {
            seen.push(*event);
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert!(matches!(seen[0], CoreEvent::Missions { .. }));
    assert!(matches!(seen[1], CoreEvent::Hunt { .. }));
    futures::executor::block_on(hub.apply(Input::Acted(Box::new(hunt(
        "hunt1",
        &[Wire::StopHunt, Wire::Mark],
        &[],
    )))));
    let acted = hub.shared().find("hunt1").cloned().expect("listed");
    assert!(
        acted
            .mission
            .controls
            .contains(&views::MissionControl::Mark)
    );
    hub.open(None);
    assert_eq!(hub.shared().open, None);
    let phone_failure = CoreEvent::Notice {
        notice: Notice::warn("Phone details unavailable"),
    };
    while !seen.contains(&phone_failure) && std::time::Instant::now() < deadline {
        clock += Duration::from_secs(1);
        if let crate::events::Pop::Event(event) = events.pop(clock) {
            seen.push(*event);
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    assert!(seen.contains(&phone_failure), "{seen:?}");
    runtime.shutdown();
}
