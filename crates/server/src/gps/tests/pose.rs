use super::*;
use crate::phones::tests::pair_one;

fn app() -> AppState {
    AppState::new(
        Engine::new(None),
        Arc::new(crate::Store::open(None).expect("store")),
    )
}

fn phone_node(id: &str, phone: &str) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body: NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Phone {
                phone: phone.to_owned(),
            }),
        }),
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn activate(app: &AppState, nodes: Vec<PatchNode>) {
    let mut snapshot = WorkspaceSnapshot::empty();
    snapshot.graph.nodes = nodes;
    let id = app
        .store
        .create_workspace("field", &snapshot)
        .expect("workspace");
    app.store.activate_workspace(id).expect("activate");
    app.gps.reconcile(app);
}

fn pose(latitude: f64, heading: f64) -> PositionFix {
    PositionFix {
        latitude,
        longitude: 13.405,
        altitude_m: None,
        accuracy_m: Some(4.0),
        speed_mps: None,
        track_deg: None,
        time: "2026-09-28T12:00:00Z".to_owned(),
        attitude: sdrmm_wire::Attitude {
            heading_deg: Some(heading),
            heading_accuracy_deg: Some(6.0),
            heading_source: Some(HeadingSource::Fused),
            ..sdrmm_wire::Attitude::default()
        },
    }
}

fn error_of(app: &AppState, node: &str) -> Option<String> {
    app.gps
        .snapshot()
        .into_iter()
        .find_map(|event| match event {
            ServerEvent::PositionChanged {
                node: shown, error, ..
            } if shown == node => error,
            _ => None,
        })
}

#[test]
fn a_pose_reaches_every_gps_node_of_that_phone() {
    let app = app();
    let first = pair_one(&app.phones).phone.id;
    let second = pair_one(&app.phones).phone.id;
    activate(
        &app,
        vec![
            phone_node("car", &first),
            phone_node("roof", &first),
            phone_node("bike", &second),
        ],
    );
    assert_eq!(app.gps.bound_to(&first), ["car", "roof"]);
    let fix = pose(52.52, 87.5);
    assert_eq!(
        app.gps.publish_pose(&app, &first, Some(fix.clone()), None),
        Ok(2)
    );
    assert_eq!(app.gps.fix("car"), Some(fix.clone()));
    assert_eq!(app.gps.fix("roof"), Some(fix));
    assert_eq!(app.gps.fix("bike"), None);
}

#[test]
fn a_pose_from_an_unbound_phone_changes_nothing() {
    let app = app();
    let lonely = pair_one(&app.phones).phone.id;
    let other = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("bike", &other)]);
    let mut events = app.gps.subscribe();
    assert_eq!(
        app.gps
            .publish_pose(&app, &lonely, Some(pose(52.0, 1.0)), None),
        Ok(0)
    );
    assert!(events.try_recv().is_err(), "an unbound pose made news");
    assert_eq!(app.gps.fix("bike"), None);
}

#[test]
fn poses_faster_than_the_phone_limit_are_refused_but_repeats_are_free() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    let fix = pose(52.52, 87.5);
    assert_eq!(
        app.gps.publish_pose(&app, &phone, Some(fix.clone()), None),
        Ok(1)
    );
    assert_eq!(
        app.gps.publish_pose(&app, &phone, Some(fix), None),
        Ok(1),
        "a keepalive of the same pose was refused"
    );
    assert_eq!(
        app.gps
            .publish_pose(&app, &phone, Some(pose(52.53, 88.0)), None),
        Err("pose updates are limited to 20 Hz per phone".to_owned())
    );
    app.gps.set_pose_interval(Duration::ZERO);
    assert_eq!(
        app.gps
            .publish_pose(&app, &phone, Some(pose(52.53, 88.0)), None),
        Ok(1)
    );
}

#[test]
fn a_pose_needs_a_valid_fix_or_an_error() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    app.gps.set_pose_interval(Duration::ZERO);
    assert!(app.gps.publish_pose(&app, &phone, None, None).is_err());
    assert!(
        app.gps
            .publish_pose(&app, &phone, Some(pose(1.0, 1.0)), Some("x".to_owned()))
            .is_err()
    );
    assert_eq!(
        app.gps
            .publish_pose(&app, &phone, Some(pose(1.0, 360.0)), None),
        Err("heading must be within 0°..360°".to_owned())
    );
    assert!(
        app.gps
            .publish_pose(&app, &phone, None, Some("  ".to_owned()))
            .is_err()
    );
    assert_eq!(
        app.gps
            .publish_pose(&app, &phone, None, Some("No GPS".to_owned())),
        Ok(1)
    );
    assert_eq!(error_of(&app, "car").as_deref(), Some("No GPS"));
}

#[test]
fn an_unknown_phone_is_not_paired() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(
        &app,
        vec![
            phone_node("ghost", "p0123456789abcdef"),
            phone_node("car", &phone),
        ],
    );
    assert_eq!(error_of(&app, "ghost").as_deref(), Some(PHONE_NOT_PAIRED));
    assert_eq!(error_of(&app, "car").as_deref(), Some(PHONE_OFFLINE));
    let (_socket, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    assert_eq!(error_of(&app, "car").as_deref(), Some(WAITING));
    app.gps.reconcile(&app);
    assert_eq!(error_of(&app, "car").as_deref(), Some(WAITING));
}

#[test]
fn going_offline_drops_the_old_coordinates() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    let (socket, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    app.gps
        .publish_pose(&app, &phone, Some(pose(52.52, 87.5)), None)
        .expect("pose");
    app.gps.reconcile(&app);
    assert!(
        app.gps.fix("car").is_some(),
        "a reconcile dropped a live fix"
    );
    app.phones.leave(socket);
    app.gps.phone_offline(&app, &phone);
    assert_eq!(app.gps.fix("car"), None);
    assert_eq!(error_of(&app, "car").as_deref(), Some(PHONE_OFFLINE));
    let (_again, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    assert_eq!(error_of(&app, "car").as_deref(), Some(WAITING));
}

#[test]
fn the_last_socket_closing_marks_the_phone_offline() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    let (first, _) = app.phones.join(&phone);
    let (second, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    app.gps
        .publish_pose(&app, &phone, Some(pose(52.52, 87.5)), None)
        .expect("pose");
    assert!(!app.phones.leave(first));
    app.gps.phone_offline(&app, &phone);
    assert!(app.gps.fix("car").is_some(), "one socket is still open");
    assert!(app.phones.leave(second));
    app.gps.phone_offline(&app, &phone);
    assert_eq!(error_of(&app, "car").as_deref(), Some(PHONE_OFFLINE));
}

#[test]
fn a_late_offline_leaves_a_reconnected_phone_online() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    let (old, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    assert!(app.phones.leave(old));
    let (_new, first) = app.phones.join(&phone);
    assert!(first);
    app.gps.phone_online(&app, &phone);
    app.gps.phone_offline(&app, &phone);
    assert_eq!(error_of(&app, "car").as_deref(), Some(WAITING));
    app.gps
        .publish_pose(&app, &phone, Some(pose(52.52, 87.5)), None)
        .expect("pose");
    app.gps.phone_online(&app, &phone);
    assert!(app.gps.fix("car").is_some());
}

#[test]
fn a_phone_without_a_socket_is_not_marked_online() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    app.gps.phone_online(&app, &phone);
    assert_eq!(error_of(&app, "car").as_deref(), Some(PHONE_OFFLINE));
}

#[tokio::test(start_paused = true)]
async fn a_silent_phone_is_reported() {
    let app = app();
    let phone = pair_one(&app.phones).phone.id;
    activate(&app, vec![phone_node("car", &phone)]);
    let (_socket, _) = app.phones.join(&phone);
    app.gps.phone_online(&app, &phone);
    app.gps.spawn_watchdog(&app);
    app.gps
        .publish_pose(&app, &phone, Some(pose(52.52, 87.5)), None)
        .expect("pose");
    tokio::time::sleep(Duration::from_millis(4_500)).await;
    assert!(app.gps.fix("car").is_some(), "silent too early");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(app.gps.fix("car"), None);
    assert_eq!(error_of(&app, "car").as_deref(), Some(PHONE_SILENT));
    app.gps
        .publish_pose(&app, &phone, Some(pose(52.53, 87.5)), None)
        .expect("pose");
    assert!(
        app.gps.fix("car").is_some(),
        "a new pose did not end the silence"
    );
}
