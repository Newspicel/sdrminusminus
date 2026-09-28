use serde_json::json;

use super::*;

fn hunt() -> Mission {
    Mission {
        node: "hunt1".to_owned(),
        label: "Signal hunt".to_owned(),
        ready: true,
        problems: Vec::new(),
        controls: vec![MissionControl::Tune, MissionControl::StartHunt],
        body: MissionBody::Hunt(HuntMission {
            target: Some(ChannelTarget {
                device_set: 1,
                channel: 3,
                channel_node: "ch1".to_owned(),
                channel_type: "nfm".to_owned(),
                frequency_hz: 145_500_000.0,
                bandwidth_hz: 12_500.0,
            }),
            status: None,
            clicks: true,
            position: None,
            triangulations: Vec::new(),
        }),
    }
}

fn round_trips(mission: &Mission) -> serde_json::Value {
    let value = serde_json::to_value(mission).unwrap();
    assert_eq!(
        &serde_json::from_value::<Mission>(value.clone()).unwrap(),
        mission
    );
    value
}

#[test]
fn mission_listing_json_shape() {
    let entry = json!({
        "node": "hunt1",
        "label": "Signal hunt",
        "ready": true,
        "controls": ["tune", "start_hunt"],
        "kind": "hunt",
        "data": {
            "target": {
                "device_set": 1,
                "channel": 3,
                "channel_node": "ch1",
                "channel_type": "nfm",
                "frequency_hz": 145_500_000.0,
                "bandwidth_hz": 12_500.0
            },
            "clicks": true
        }
    });
    assert_eq!(
        serde_json::from_value::<Mission>(entry.clone()).unwrap(),
        hunt()
    );
    assert_eq!(round_trips(&hunt()), entry);

    let listing = MissionsResponse {
        revision: 7,
        workspace: Some(MissionWorkspace {
            id: 2,
            name: "Roof".to_owned(),
        }),
        workspaces: vec![MissionWorkspace {
            id: 2,
            name: "Roof".to_owned(),
        }],
        missions: vec![hunt()],
        truncated: 0,
    };
    let value = serde_json::to_value(&listing).unwrap();
    assert_eq!(value["missions"][0], entry);
    assert_eq!(value["workspace"], json!({"id": 2, "name": "Roof"}));
    let bare: MissionsResponse =
        serde_json::from_value(json!({"revision": 1, "workspaces": [], "missions": []})).unwrap();
    assert_eq!(bare.truncated, 0);
    assert!(bare.workspace.is_none());
}

#[test]
fn mission_body_flattens_kind_and_data() {
    let phone = PositionLink {
        node: "gps1".to_owned(),
        phone: Some("p0123456789abcdef".to_owned()),
    };
    let bodies = [
        MissionBody::Df(DfMission {
            array: Some("array1".to_owned()),
            device_sets: vec![0, 1],
            center_hz: Some(433_920_000.0),
            position: Some(phone.clone()),
            triangulations: vec!["tri1".to_owned()],
        }),
        MissionBody::Radar(RadarMission {
            array: None,
            device_sets: vec![2],
            center_hz: None,
            position: None,
            transmitter: Some(PositionLink {
                node: "tx".to_owned(),
                phone: None,
            }),
            surface: true,
        }),
        MissionBody::Survey(SurveyMission {
            device_set: Some(0),
            stream: 1,
            frequency_hz: Some(100_000_000.0),
            offset_hz: -25_000,
            bandwidth_hz: 12_500,
            position: Some(phone),
            recording: false,
            cells: 12,
        }),
        MissionBody::Triangulation(TriangulationMission {
            sources: vec!["df1".to_owned(), "hunt1".to_owned()],
            position: None,
            state: None,
        }),
    ];
    for (body, kind) in bodies
        .into_iter()
        .zip(["df", "radar", "survey", "triangulation"])
    {
        let mission = Mission {
            node: "n".to_owned(),
            label: "N".to_owned(),
            ready: false,
            problems: vec![MissionProblem::NotRunning],
            controls: Vec::new(),
            body,
        };
        let value = round_trips(&mission);
        assert_eq!(value["kind"], kind);
        assert!(value["data"].is_object());
        assert!(value.get("body").is_none() && value.get("controls").is_none());
    }
}

#[test]
fn mission_problems_are_tagged_by_problem() {
    let cases = [
        (
            MissionProblem::Unwired {
                port: "position".to_owned(),
            },
            json!({"problem": "unwired", "port": "position"}),
        ),
        (
            MissionProblem::PhoneOffline {
                phone: "p0123456789abcdef".to_owned(),
            },
            json!({"problem": "phone_offline", "phone": "p0123456789abcdef"}),
        ),
        (MissionProblem::OutOfBand, json!({"problem": "out_of_band"})),
    ];
    for (problem, value) in cases {
        assert_eq!(serde_json::to_value(&problem).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<MissionProblem>(value).unwrap(),
            problem
        );
    }
}

#[test]
fn every_action_names_its_control() {
    let actions = [
        MissionAction::Tune {
            frequency_hz: 145_500_000.0,
        },
        MissionAction::Calibrate,
        MissionAction::StartHunt,
        MissionAction::StopHunt,
        MissionAction::StartSweep,
        MissionAction::StopSweep,
        MissionAction::Mark,
        MissionAction::ClearFusion,
        MissionAction::StartSurvey,
        MissionAction::StopSurvey,
        MissionAction::ClearSurvey,
    ];
    let mut controls = Vec::new();
    for action in &actions {
        let tag = serde_json::to_value(action).unwrap()["action"].clone();
        let control = action.control();
        assert_eq!(serde_json::to_value(control).unwrap(), tag, "{action:?}");
        assert!(!controls.contains(&control), "{control:?} named twice");
        controls.push(control);
    }
    assert_eq!(
        serde_json::from_value::<MissionAction>(json!({"action": "tune", "frequency_hz": 7.1e6}))
            .unwrap(),
        MissionAction::Tune {
            frequency_hz: 7.1e6
        }
    );
    assert_eq!(
        serde_json::to_value(SwitchWorkspaceRequest { workspace: 3 }).unwrap(),
        json!({"workspace": 3})
    );
}
