use sdrmm_wire::mission::{
    ChannelTarget, DfMission, HuntMission, MissionControl as Wire, MissionWorkspace, PositionLink,
    RadarMission, SurveyMission, TriangulationMission,
};

use super::*;

pub(crate) fn listed(node: &str, controls: &[Wire], body: MissionBody) -> wire::Mission {
    wire::Mission {
        node: node.to_owned(),
        label: format!("{node} label"),
        ready: true,
        problems: Vec::new(),
        controls: controls.to_vec(),
        body,
    }
}

pub(crate) fn hunt(node: &str, controls: &[Wire], triangulations: &[&str]) -> wire::Mission {
    listed(
        node,
        controls,
        MissionBody::Hunt(HuntMission {
            target: Some(ChannelTarget {
                device_set: 1,
                channel: 3,
                channel_node: "ch1".to_owned(),
                channel_type: "nfm".to_owned(),
                frequency_hz: 145.5e6,
                bandwidth_hz: 12_500.0,
            }),
            status: None,
            clicks: true,
            position: Some(PositionLink {
                node: "gps1".to_owned(),
                phone: Some("p0123456789abcdef".to_owned()),
            }),
            triangulations: triangulations
                .iter()
                .map(|node| (*node).to_owned())
                .collect(),
        }),
    )
}

pub(crate) fn df(node: &str, triangulations: &[&str]) -> wire::Mission {
    listed(
        node,
        &[Wire::Tune, Wire::Calibrate],
        MissionBody::Df(DfMission {
            array: Some("arr".to_owned()),
            device_sets: vec![1],
            center_hz: Some(433.92e6),
            position: None,
            triangulations: triangulations
                .iter()
                .map(|node| (*node).to_owned())
                .collect(),
        }),
    )
}

pub(crate) fn triangulation(node: &str, sources: &[&str], clear: bool) -> wire::Mission {
    let controls: &[Wire] = if clear { &[Wire::ClearFusion] } else { &[] };
    listed(
        node,
        controls,
        MissionBody::Triangulation(TriangulationMission {
            sources: sources.iter().map(|node| (*node).to_owned()).collect(),
            position: None,
            state: None,
        }),
    )
}

pub(crate) fn radar(node: &str) -> wire::Mission {
    listed(
        node,
        &[Wire::Tune, Wire::Calibrate],
        MissionBody::Radar(RadarMission {
            array: Some("arr".to_owned()),
            device_sets: vec![1],
            center_hz: Some(98.1e6),
            position: None,
            transmitter: None,
            surface: true,
        }),
    )
}

pub(crate) fn survey(node: &str) -> wire::Mission {
    listed(
        node,
        &[Wire::StartSurvey],
        MissionBody::Survey(SurveyMission {
            device_set: Some(1),
            stream: 0,
            frequency_hz: Some(145.5e6),
            offset_hz: 0,
            bandwidth_hz: 12_500,
            position: None,
            recording: false,
            cells: 0,
        }),
    )
}

pub(crate) fn response(missions: Vec<wire::Mission>) -> MissionsResponse {
    MissionsResponse {
        revision: 1,
        workspace: Some(MissionWorkspace {
            id: 7,
            name: "Field".to_owned(),
        }),
        workspaces: vec![
            MissionWorkspace {
                id: 7,
                name: "Field".to_owned(),
            },
            MissionWorkspace {
                id: 8,
                name: "Lab".to_owned(),
            },
        ],
        missions,
        truncated: 0,
    }
}

fn server(node: &str, action: MissionAction) -> Route {
    Route::Server {
        node: node.to_owned(),
        action,
    }
}

#[test]
fn hunt_sweep_controls_map_to_server_actions() {
    let listing = Listing::new(response(vec![hunt(
        "hunt1",
        &[Wire::Tune, Wire::StopHunt, Wire::StartSweep, Wire::Mark],
        &[],
    )]));
    let entry = listing.find("hunt1").expect("listed");
    assert_eq!(entry.mission.kind, MissionKind::Hunt);
    assert_eq!(
        entry.mission.controls,
        [
            MissionControl::Tune,
            MissionControl::HuntRun,
            MissionControl::Sweep,
            MissionControl::Mark
        ]
    );
    assert_eq!(entry.mission.detail, "145.500 MHz");
    assert_eq!(
        route(entry, MissionCommand::Sweep { on: true }),
        Ok(server("hunt1", MissionAction::StartSweep))
    );
    assert_eq!(
        route(entry, MissionCommand::StopHunt),
        Ok(server("hunt1", MissionAction::StopHunt))
    );
    assert_eq!(
        route(entry, MissionCommand::StartHunt),
        Ok(server("hunt1", MissionAction::StartHunt))
    );
    assert_eq!(
        route(entry, MissionCommand::Mark),
        Ok(server("hunt1", MissionAction::Mark))
    );
    assert_eq!(
        route(entry, MissionCommand::Tune { hz: 145.6e6 }),
        Ok(server(
            "hunt1",
            MissionAction::Tune {
                frequency_hz: 145.6e6
            }
        ))
    );
    assert!(matches!(
        entry.target,
        Target::Hunt {
            device_set: Some(1),
            channel: Some(3),
            running: true,
            ..
        }
    ));
}

#[test]
fn sweep_off_sends_stop_sweep() {
    let listing = Listing::new(response(vec![hunt(
        "hunt1",
        &[Wire::StopHunt, Wire::StopSweep],
        &[],
    )]));
    let entry = listing.find("hunt1").expect("listed");
    assert_eq!(
        route(entry, MissionCommand::Sweep { on: false }),
        Ok(server("hunt1", MissionAction::StopSweep))
    );
}

#[test]
fn a_triangulation_without_df_becomes_a_df_drive() {
    let listing = Listing::new(response(vec![triangulation("tri1", &["a", "b"], true)]));
    let entry = listing.find("tri1").expect("listed");
    assert_eq!(entry.mission.kind, MissionKind::DfDrive);
    assert_eq!(entry.mission.detail, "2 sources");
    assert_eq!(
        entry.mission.controls,
        [MissionControl::ClearFusion, MissionControl::TargetMode]
    );
    assert_eq!(entry.fusion_node(), Some("tri1"));
    assert_eq!(
        route(entry, MissionCommand::ClearFusion),
        Ok(server("tri1", MissionAction::ClearFusion))
    );
    assert_eq!(
        route(
            entry,
            MissionCommand::SetTargetMode {
                mode: TargetMode::Direct
            }
        ),
        Ok(Route::TargetMode(TargetMode::Direct))
    );
}

#[test]
fn a_hunt_with_a_triangulation_still_yields_a_guidance_mission() {
    let listing = Listing::new(response(vec![
        hunt("hunt1", &[Wire::StartHunt], &["tri1"]),
        triangulation("tri1", &["hunt1"], true),
    ]));
    let kinds: Vec<(String, MissionKind)> = listing
        .entries
        .iter()
        .map(|entry| (entry.id().to_owned(), entry.mission.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("hunt1".to_owned(), MissionKind::Hunt),
            ("tri1".to_owned(), MissionKind::DfDrive)
        ]
    );
}

#[test]
fn clear_fusion_on_a_df_drive_targets_its_triangulation() {
    let listing = Listing::new(response(vec![
        df("df1", &["tri1", "tri2"]),
        triangulation("tri1", &["df1"], true),
        triangulation("tri2", &["df1"], true),
    ]));
    assert_eq!(listing.entries.len(), 1);
    let entry = listing.find("df1").expect("listed");
    assert_eq!(
        entry.mission.controls,
        [
            MissionControl::Tune,
            MissionControl::Calibrate,
            MissionControl::ClearFusion,
            MissionControl::TargetMode
        ]
    );
    assert_eq!(entry.fusion_node(), Some("tri1"));
    assert_eq!(
        route(entry, MissionCommand::ClearFusion),
        Ok(server("tri1", MissionAction::ClearFusion))
    );
    assert_eq!(
        route(entry, MissionCommand::Calibrate),
        Ok(server("df1", MissionAction::Calibrate))
    );
    let unclearable = Listing::new(response(vec![
        df("df1", &["tri1"]),
        triangulation("tri1", &["df1"], false),
    ]));
    let entry = unclearable.find("df1").expect("listed");
    assert!(
        !entry
            .mission
            .controls
            .contains(&MissionControl::ClearFusion)
    );
    assert_eq!(
        route(entry, MissionCommand::ClearFusion),
        Err(CoreError::Refused {
            message: NOT_A_CONTROL.to_owned()
        })
    );
    let lone = Listing::new(response(vec![df("df1", &[])]));
    assert_eq!(
        lone.find("df1").expect("listed").mission.controls,
        [MissionControl::Tune, MissionControl::Calibrate]
    );
}

#[test]
fn commands_outside_the_controls_are_refused() {
    let listing = Listing::new(response(vec![
        hunt("hunt1", &[Wire::Tune], &[]),
        survey("map1"),
        radar("pr1"),
    ]));
    let hunt = listing.find("hunt1").expect("listed");
    assert_eq!(
        route(hunt, MissionCommand::Calibrate),
        Err(CoreError::Refused {
            message: NOT_A_CONTROL.to_owned()
        })
    );
    assert_eq!(
        route(hunt, MissionCommand::Tune { hz: f64::NAN }),
        Err(CoreError::Refused {
            message: "Bad frequency".to_owned()
        })
    );
    let map = listing.find("map1").expect("listed");
    assert_eq!(map.mission.kind, MissionKind::Survey);
    assert_eq!(
        route(map, MissionCommand::StartSurvey),
        Ok(server("map1", MissionAction::StartSurvey))
    );
    assert!(route(map, MissionCommand::ClearSurvey).is_err());
    let pr = listing.find("pr1").expect("listed");
    assert_eq!(pr.mission.kind, MissionKind::RadarWatch);
    assert_eq!(pr.mission.detail, "98.100 MHz");
}

#[test]
fn blockers_are_the_first_problem_label_and_workspaces_carry_ids() {
    let mut blocked = hunt("hunt1", &[], &[]);
    blocked.ready = false;
    blocked.problems = vec![
        MissionProblem::PhoneOffline {
            phone: "p1".to_owned(),
        },
        MissionProblem::NoPosition,
    ];
    let listing = Listing::new(response(vec![blocked]));
    let view = listing.view();
    assert_eq!(view.missions[0].blocker.as_deref(), Some("Phone offline"));
    assert!(!view.missions[0].ready);
    assert_eq!(view.workspace.id, "7");
    assert_eq!(
        view.workspaces
            .iter()
            .map(|workspace| workspace.name.as_str())
            .collect::<Vec<_>>(),
        ["Field", "Lab"]
    );
    assert_eq!(
        problem_label(&MissionProblem::Unwired {
            port: "array".to_owned()
        }),
        "Wire array"
    );
    assert_eq!(Listing::default().view().workspace.id, "");
}

#[test]
fn a_mission_naming_this_phone_wants_its_pose() {
    let listing = Listing::new(response(vec![hunt("hunt1", &[], &[]), df("df1", &[])]));
    assert!(listing.wants_phone("p0123456789abcdef"));
    assert!(!listing.wants_phone("pffffffffffffffff"));
}

#[test]
fn an_acted_mission_replaces_its_listing_entry() {
    let mut listing = Listing::new(response(vec![hunt("hunt1", &[Wire::StartHunt], &[])]));
    listing.replace(hunt("hunt1", &[Wire::StopHunt], &[]));
    assert!(matches!(
        listing.find("hunt1").map(|entry| &entry.target),
        Some(Target::Hunt { running: true, .. })
    ));
    assert_eq!(mhz(None), "");
}
