#![cfg(not(any(target_os = "ios", target_os = "android")))]
#![allow(clippy::expect_used)]

mod support;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sdrmm_mobile_core::{
    CoreError, CoreEvent, HeadingMode, HeadingSample, LinkState, LocationSample, MagAccuracy,
    MissionCommand, MissionControl, MissionKind, MissionsView, MotionFrame, MotionSample, Mount,
    PoseSettings, RefusalKind,
};
use sdrmm_wire::{ApiError, HeadingSource, ServerEvent, geo::wrap_180};
use support::{GPS, HUNT, TestPhone, TestServer, gps_graph, hunt_graph, spawn_server};

const HEADING_DEG: f64 = 123.0;
const DECLINATION_DEG: f64 = 3.0;
const MOTION_EVERY: Duration = Duration::from_millis(50);

fn now_ms() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970");
    i64::try_from(since.as_millis()).expect("millis fit")
}

fn missions(event: CoreEvent) -> Option<MissionsView> {
    match event {
        CoreEvent::Missions { view } => Some(view),
        _ => None,
    }
}

fn link(event: CoreEvent) -> Option<LinkState> {
    match event {
        CoreEvent::Link { state } => Some(state),
        _ => None,
    }
}

async fn live_with_hunt(server: &TestServer, phone: &TestPhone) {
    let workspace = server.create_workspace("hunt", hunt_graph(None)).await;
    server.activate(workspace).await;
    phone.go_live(server).await;
    phone
        .wait_for("a ready hunt", |event| {
            missions(event).filter(|view| {
                view.missions
                    .iter()
                    .any(|mission| mission.id == HUNT && mission.ready)
            })
        })
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pairs_from_a_qr_link_and_goes_live() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let offer = server.offer().await;
    let parsed = phone
        .core
        .parse_pair_link(server.local_link(&offer))
        .expect("the link parses");
    assert_eq!(parsed.hosts, [server.phone_host()]);
    assert_eq!(parsed.fingerprint_short, Some(offer.key_check.clone()));
    let saved = phone
        .core
        .pair(parsed, support::PHONE_NAME.to_owned())
        .await
        .expect("paired");
    phone.core.connect(saved.id.clone()).await.expect("connect");
    phone
        .wait_for("online", |event| {
            matches!(link(event), Some(LinkState::Online { .. })).then_some(())
        })
        .await;
    phone.wait_for("missions", missions).await;

    let record = phone.vault.record(&saved.id);
    assert_eq!(record["pin"], offer.endpoint.pin);
    assert_eq!(record["pin"], server.presented_pin().await);
    assert!(!record["token"].as_str().unwrap_or_default().is_empty());
    let listed = server.phones().await;
    let paired = listed
        .phones
        .iter()
        .find(|listed| listed.id == saved.phone_id)
        .expect("the phone is listed");
    assert_eq!(paired.name, support::PHONE_NAME);
    assert!(paired.online);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pose_reaches_the_gps_node_bound_to_the_phone() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let saved = phone.go_live(&server).await;
    phone.core.set_pose_settings(PoseSettings {
        heading_mode: HeadingMode::Auto,
        mount: Mount::Flat,
        mount_offset_deg: 0.0,
        share_pose: true,
    });
    let mut admin = server.admin_ws().await;
    let workspace = server
        .create_workspace("field", gps_graph(&saved.phone_id))
        .await;
    server.activate(workspace).await;

    let core = phone.core.clone();
    let pushing = tokio::spawn(async move {
        let mut tick = tokio::time::interval(MOTION_EVERY);
        let mut step = 0u64;
        loop {
            tick.tick().await;
            step += 1;
            let t = now_ms();
            core.push_motion(level_motion(t));
            if step % 2 == 1 {
                core.push_heading(compass(t));
            }
            if step % 20 == 1 {
                core.push_location(standing(t));
            }
        }
    });
    let fix = admin
        .wait_for("a fused pose on the GPS node", |event| match event {
            ServerEvent::PositionChanged {
                node,
                fix: Some(fix),
                ..
            } if node == GPS && fix.attitude.heading_source == Some(HeadingSource::Fused) => {
                Some(fix)
            }
            _ => None,
        })
        .await;
    pushing.abort();
    assert!((fix.latitude - 48.1).abs() < 1e-9, "{fix:?}");
    assert!((fix.longitude - 11.5).abs() < 1e-9, "{fix:?}");
    let heading = fix.attitude.heading_deg.expect("a heading");
    assert!(wrap_180(heading - HEADING_DEG).abs() <= 3.0, "{heading}");
}

fn level_motion(t: i64) -> MotionSample {
    MotionSample {
        t_unix_ms: t,
        frame: MotionFrame::Arbitrary,
        qw: 1.0,
        qx: 0.0,
        qy: 0.0,
        qz: 0.0,
        rot_x: 0.0,
        rot_y: 0.0,
        rot_z: 0.0,
        grav_x: 0.0,
        grav_y: 0.0,
        grav_z: -1.0,
        heading_deg: None,
        mag_accuracy: MagAccuracy::High,
    }
}

fn compass(t: i64) -> HeadingSample {
    HeadingSample {
        t_unix_ms: t,
        true_deg: Some(HEADING_DEG),
        magnetic_deg: HEADING_DEG - DECLINATION_DEG,
        accuracy_deg: Some(5.0),
    }
}

fn standing(t: i64) -> LocationSample {
    LocationSample {
        t_unix_ms: t,
        lat: 48.1,
        lon: 11.5,
        alt_m: Some(520.0),
        h_acc_m: 5.0,
        v_acc_m: Some(8.0),
        speed_mps: Some(0.0),
        speed_acc_mps: Some(0.5),
        course_deg: None,
        course_acc_deg: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_wrong_pin_never_sends_the_secret() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let saved = phone.pair(&server).await.expect("paired");
    let mut record = phone.vault.record(&saved.id);
    record["pin"] = serde_json::Value::String("ab".repeat(32));
    phone.vault.replace_record(&saved.id, &record);
    phone.core.connect(saved.id.clone()).await.expect("connect");
    let mut refusals = 0;
    phone
        .wait_for("a retry refused again", |event| {
            if matches!(
                link(event),
                Some(LinkState::Refused {
                    reason: RefusalKind::KeyMismatch,
                    ..
                })
            ) {
                refusals += 1;
            }
            (refusals == 2).then_some(())
        })
        .await;
    assert_eq!(phone.vault.record(&saved.id)["pin"], "ab".repeat(32));
    let listed = server.phones().await;
    let paired = listed
        .phones
        .iter()
        .find(|listed| listed.id == saved.phone_id)
        .expect("the phone is listed");
    assert_eq!(paired.last_seen, None);
    assert!(!paired.online);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_phone_is_told_to_pair_again() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let saved = phone.go_live(&server).await;
    server.revoke(&saved.phone_id).await;
    let mut redialed = false;
    let revoked = phone
        .wait_within(Duration::from_secs(5), "revoked", |event| {
            match link(event) {
                Some(LinkState::Connecting { .. }) => {
                    redialed = true;
                    None
                }
                Some(LinkState::Refused {
                    reason: RefusalKind::Revoked,
                    ..
                }) => Some(()),
                _ => None,
            }
        })
        .await;
    assert!(revoked.is_ok(), "{revoked:?}");
    assert!(!redialed, "the live socket was not closed as revoked");
    let retried = phone
        .wait_within(Duration::from_secs(10), "a retry", |event| {
            matches!(
                link(event),
                Some(LinkState::Connecting { .. } | LinkState::Online { .. })
            )
            .then_some(())
        })
        .await;
    assert!(retried.is_err(), "a revoked phone tried again");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_code_is_rejected() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let mut offer = phone.offer(&server).await;
    offer.code = if offer.code == "11111111" {
        "22222222".to_owned()
    } else {
        "11111111".to_owned()
    };
    let refused = phone.core.pair(offer, support::PHONE_NAME.to_owned()).await;
    assert_eq!(refused, Err(CoreError::WrongCode));
    assert!(server.phones().await.phones.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_mission_command_is_refused_with_the_server_reason() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    let saved = phone.go_live(&server).await;
    let workspace = server
        .create_workspace("hunt", hunt_graph(Some(&saved.phone_id)))
        .await;
    server.activate(workspace).await;
    phone
        .wait_for("a hunt that starts", |event| {
            missions(event).filter(|view| {
                view.missions.iter().any(|mission| {
                    mission.id == HUNT && mission.controls.contains(&MissionControl::HuntRun)
                })
            })
        })
        .await;
    phone.core.refresh_missions().await.expect("refreshed");
    assert_eq!(
        phone.core.open_mission("nope".to_owned()),
        Err(CoreError::NoMission)
    );
    phone.core.open_mission(HUNT.to_owned()).expect("opened");
    phone
        .core
        .send(MissionCommand::StartHunt)
        .await
        .expect("the hunt starts");
    let refused = phone.core.send(MissionCommand::Mark).await;
    let (status, body) = server
        .act(HUNT, serde_json::json!({ "action": "mark" }))
        .await;
    assert!(status.is_client_error(), "{status} {body}");
    let expected: ApiError = serde_json::from_str(&body).expect("an api error");
    assert_eq!(
        refused,
        Err(CoreError::Refused {
            message: expected.error
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_workspace_updates_missions() {
    let server = spawn_server().await;
    let phone = TestPhone::new();
    live_with_hunt(&server, &phone).await;
    let second = server
        .create_workspace("second", sdrmm_wire::PatchGraph::default())
        .await;
    phone
        .core
        .switch_workspace(second.to_string())
        .await
        .expect("switched");
    let view = phone
        .wait_for("the second workspace", |event| {
            missions(event).filter(|view| view.workspace.id == second.to_string())
        })
        .await;
    assert_eq!(view.workspace.name, "second");
    assert!(
        view.missions
            .iter()
            .all(|mission| mission.kind != MissionKind::Hunt),
        "{view:?}"
    );
    assert!(
        view.workspaces
            .iter()
            .any(|workspace| workspace.name == "hunt")
    );
}
