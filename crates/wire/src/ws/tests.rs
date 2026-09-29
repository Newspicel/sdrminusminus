use serde_json::json;

use super::*;
use crate::{
    about::API_PROTOCOL, array::ArrayStatus, fusion::DfFusionState, processor::ProcessorReading,
    survey::SurveyUpdate,
};

fn round_trip<T>(value: &T) -> serde_json::Value
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_value(value).expect("serialize");
    let back: T = serde_json::from_value(json.clone()).expect("deserialize");
    assert_eq!(&back, value);
    json
}

fn survey_update() -> SurveyUpdate {
    SurveyUpdate {
        level_dbfs: Some(-42.5),
        target_hz: Some(433_920_000.0),
        recording: true,
        cells: 3,
        dropped: 0,
        cell: None,
        stopped: None,
    }
}

#[test]
fn server_events_round_trip() {
    let events = [
        ServerEvent::Hello {
            revision: 9,
            protocol: API_PROTOCOL,
            phone: Some("a1b2".to_owned()),
        },
        ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        },
        ServerEvent::StateChanged {
            scope: StateScope::Phones,
        },
        ServerEvent::StateChanged {
            scope: StateScope::Missions,
        },
        ServerEvent::ArrayUpdate {
            status: Box::new(ArrayStatus::default()),
        },
        ServerEvent::ProcessorUpdate {
            node: "df".to_owned(),
            reading: Box::new(ProcessorReading::empty("df").expect("df reading")),
        },
        ServerEvent::ProcessorUpdate {
            node: "radar".to_owned(),
            reading: Box::new(ProcessorReading::empty("passive_radar").expect("radar reading")),
        },
        ServerEvent::DfFusionUpdate {
            node: "tri".to_owned(),
            state: Box::new(DfFusionState::default()),
        },
        ServerEvent::SurveyUpdate {
            node: "survey".to_owned(),
            update: Box::new(survey_update()),
        },
        ServerEvent::SurfaceStreamStarted {
            stream_id: 7,
            node: "radar".to_owned(),
            kind: StreamKind::RangeDoppler,
        },
        ServerEvent::StreamStopped {
            stream_id: 7,
            kind: StreamKind::FusionGrid,
        },
        ServerEvent::SurfaceRefused {
            node: "radar".to_owned(),
            reason: SurfaceRefusal::NoStreamIds,
        },
    ];
    for event in &events {
        let json = round_trip(event);
        assert!(json["type"].is_string());
    }
}

#[test]
fn hello_names_the_protocol_and_only_a_phone() {
    let desktop = serde_json::to_value(ServerEvent::Hello {
        revision: 3,
        protocol: API_PROTOCOL,
        phone: None,
    })
    .expect("hello");
    assert_eq!(desktop["data"]["protocol"], API_PROTOCOL);
    assert!(desktop["data"].get("phone").is_none());
}

#[test]
fn surface_stream_started_names_its_kind() {
    let event = ServerEvent::SurfaceStreamStarted {
        stream_id: 12,
        node: "tri".to_owned(),
        kind: StreamKind::FusionGrid,
    };
    let json = round_trip(&event);
    assert_eq!(json["type"], "SurfaceStreamStarted");
    assert_eq!(json["data"]["kind"], "fusion_grid");
    assert_eq!(json["data"]["node"], "tri");
    assert!(json["data"].get("device_set").is_none());
    for (kind, name) in [
        (StreamKind::RangeDoppler, "range_doppler"),
        (StreamKind::SpatialSpectrum, "spatial_spectrum"),
        (StreamKind::Visibility, "visibility"),
        (StreamKind::FusionGrid, "fusion_grid"),
    ] {
        assert_eq!(serde_json::to_value(kind).expect("kind"), name);
    }
}

#[test]
fn a_surface_refusal_names_its_node_and_reason() {
    let event = ServerEvent::SurfaceRefused {
        node: "tri".to_owned(),
        reason: SurfaceRefusal::FitNotPositive,
    };
    assert_eq!(
        round_trip(&event),
        json!({"type": "SurfaceRefused", "data": {"node": "tri", "reason": "fit_not_positive"}})
    );
    for (reason, name) in [
        (SurfaceRefusal::NoSurface, "no_surface"),
        (SurfaceRefusal::FitNotPositive, "fit_not_positive"),
        (SurfaceRefusal::NoStreamIds, "no_stream_ids"),
    ] {
        assert_eq!(serde_json::to_value(reason).expect("reason"), name);
        assert!(!reason.label().is_empty());
    }
}

#[test]
fn a_pose_arrives_in_the_documented_shape() {
    let sent = r#"{"type":"PublishPose","data":{"fix":{"latitude":52.52,"longitude":13.405,"accuracy_m":4,"time":"2026-09-28T12:00:00Z","heading_deg":87.5,"heading_accuracy_deg":6,"heading_source":"fused","pitch_deg":1.2,"roll_deg":-0.4}}}"#;
    let command: ClientCommand = serde_json::from_str(sent).expect("pose parses");
    let ClientCommand::PublishPose {
        fix: Some(fix),
        error: None,
    } = &command
    else {
        panic!("not a pose: {command:?}");
    };
    assert_eq!(fix.attitude.heading_deg, Some(87.5));
    round_trip(&command);
    let lost = ClientCommand::PublishPose {
        fix: None,
        error: Some("No GPS".to_owned()),
    };
    let json = round_trip(&lost);
    assert!(json["data"].get("fix").is_none());
}

#[test]
fn a_surface_subscription_may_name_its_fit() {
    let fitted: ClientCommand = serde_json::from_value(json!({
        "type": "SubscribeSurface",
        "data": {"node": "radar", "fit": {"cols": 256, "rows": 128}}
    }))
    .expect("fitted subscription");
    assert_eq!(
        fitted,
        ClientCommand::SubscribeSurface {
            node: "radar".to_owned(),
            fit: Some(SurfaceFit {
                cols: 256,
                rows: 128
            }),
        }
    );
    let bare: ClientCommand = serde_json::from_value(json!({
        "type": "SubscribeSurface",
        "data": {"node": "radar"}
    }))
    .expect("bare subscription");
    assert_eq!(
        bare,
        ClientCommand::SubscribeSurface {
            node: "radar".to_owned(),
            fit: None,
        }
    );
}

#[test]
fn removed_messages_are_refused() {
    for gone in [
        json!({"type": "PublishPosition", "data": {"node": "gps"}}),
        json!({"type": "SubscribeFusionGrid", "data": {"node": "tri"}}),
    ] {
        assert!(serde_json::from_value::<ClientCommand>(gone).is_err());
    }
    for gone in [
        json!({"type": "DfUpdate", "data": {"node": "df"}}),
        json!({"type": "RadarDetections", "data": {"node": "radar"}}),
    ] {
        assert!(serde_json::from_value::<ServerEvent>(gone).is_err());
    }
}

#[test]
fn socket_constants_match_the_contract() {
    assert_eq!(WS_SUBPROTOCOL, "sdrmm");
    assert_eq!(WS_BEARER_PROTOCOL_PREFIX, "sdrmm.bearer.");
    assert_eq!(WS_CLOSE_REVOKED, 4003);
}
