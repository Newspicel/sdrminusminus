use std::{sync::Arc, time::Duration};

use sdrmm_wire::{
    ChannelParams, ChannelSettings, DecodedRecord, DfBearing, DfFusionState, EventOrigin, GpsNode,
    HuntSettings, NO_CHANNEL, NavTargetKind, PatchEdge, PatchNode, PortRef, Position,
    PositionSource, WorkspaceSnapshot,
    fusion::BearingSource,
    geo::{self, LatLon},
};
use tokio::sync::broadcast;

use super::*;
use crate::{ServerOptions, Store, router_with_state, tests::state_over};

const HOME: LatLon = LatLon {
    lat: 52.52,
    lon: 13.405,
};

fn node(id: &str, kind: &str) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body: NodeBody::default_for(kind).expect("a known kind"),
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn fixed_gps(id: &str, at: LatLon) -> PatchNode {
    PatchNode {
        body: NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Fixed {
                lat: at.lat,
                lon: at.lon,
                altitude_m: None,
            }),
        }),
        ..node(id, "gps")
    }
}

fn edge(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
    PatchEdge {
        from: PortRef {
            node: from.0.to_owned(),
            port: from.1.to_owned(),
        },
        to: PortRef {
            node: to.0.to_owned(),
            port: to.1.to_owned(),
        },
    }
}

fn graph() -> PatchGraph {
    PatchGraph {
        nodes: vec![
            node("df", "df"),
            node("stray", "df"),
            node("hunt", "hunt"),
            node("filter", "event_filter"),
            node("tri", "triangulation"),
            node("blind", "triangulation"),
            fixed_gps("gps", HOME),
        ],
        edges: vec![
            edge(("df", "events"), ("tri", "events")),
            edge(("df", "events"), ("blind", "events")),
            edge(("hunt", "events"), ("filter", "events")),
            edge(("filter", "events"), ("tri", "events")),
            edge(("gps", "position"), ("tri", "position")),
        ],
    }
}

fn bench(graph: PatchGraph) -> AppState {
    let store = Arc::new(Store::open(None).expect("store"));
    let snapshot = WorkspaceSnapshot {
        graph,
        ..WorkspaceSnapshot::empty()
    };
    let id = store.create_workspace("routes", &snapshot).expect("create");
    store.activate_workspace(id).expect("activate");
    let state = state_over(store);
    let (_, background) = router_with_state(state.clone(), &ServerOptions::default());
    background.detach();
    state
}

fn bearing(station: &str, from: LatLon, bearing_deg: f32) -> DfBearing {
    DfBearing {
        bearing_deg,
        confidence: 0.9,
        lat: Some(from.lat),
        lon: Some(from.lon),
        station_id: Some(station.to_owned()),
        node: String::new(),
        sigma_deg: 3.0,
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

fn record(origin: &str, bearing: DfBearing) -> DecodedRecord {
    DecodedRecord {
        origin: Some(EventOrigin {
            node: origin.to_owned(),
            transmission: 0,
        }),
        device_set: 0,
        channel: NO_CHANNEL,
        at: jiff::Timestamp::now().to_string(),
        freq_hz: 433.92e6,
        event: DecoderEvent::Df(bearing),
        sinks: Vec::new(),
    }
}

async fn fused(
    events: &mut broadcast::Receiver<ServerEvent>,
    node: &str,
    wanted: impl Fn(&DfFusionState) -> bool,
    resend: impl Fn(),
) -> DfFusionState {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        resend();
        let next = tokio::time::Instant::now() + Duration::from_millis(200);
        loop {
            match tokio::time::timeout_at(next.min(deadline), events.recv()).await {
                Ok(Ok(ServerEvent::DfFusionUpdate { node: from, state }))
                    if from == node && wanted(&state) =>
                {
                    return *state;
                }
                Ok(Ok(_) | Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) => panic!("events closed"),
                Err(_) => break,
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no update for {node}"
        );
    }
}

fn stations(state: &DfFusionState) -> Vec<&str> {
    state
        .stations
        .iter()
        .map(|station| station.station_id.as_str())
        .collect()
}

#[tokio::test]
async fn a_df_bearing_reaches_the_triangulation_it_is_wired_to() {
    let state = bench(graph());
    let mut events = state.engine.subscribe_events();
    let reached = fused(
        &mut events,
        "tri",
        |fused| fused.samples > 0,
        || {
            state
                .engine
                .publish_decoded(record("df", bearing("roof", HOME, 60.0)));
        },
    )
    .await;
    assert_eq!(stations(&reached), ["roof"]);
    let also = state.fusion.state("blind").expect("second triangulation");
    assert!(
        also.samples > 0,
        "one bearing reaches every wired triangulation"
    );
}

#[tokio::test]
async fn a_bearing_from_an_unwired_node_is_ignored() {
    let state = bench(graph());
    let mut events = state.engine.subscribe_events();
    let warmed = fused(
        &mut events,
        "tri",
        |fused| fused.samples > 0,
        || {
            state
                .engine
                .publish_decoded(record("df", bearing("roof", HOME, 60.0)));
        },
    )
    .await;
    state
        .engine
        .publish_decoded(record("stray", bearing("stray", HOME, 200.0)));
    let after = warmed.samples;
    let next = fused(
        &mut events,
        "tri",
        |fused| fused.samples > after,
        || {
            state
                .engine
                .publish_decoded(record("df", bearing("roof", HOME, 61.0)));
        },
    )
    .await;
    assert_eq!(stations(&next), ["roof"]);
}

#[tokio::test]
async fn a_hunt_bearing_reaches_triangulation_through_an_event_filter() {
    let state = bench(graph());
    let mut events = state.engine.subscribe_events();
    let reached = fused(
        &mut events,
        "tri",
        |fused| stations(fused).contains(&"walker"),
        || {
            let mut walked = bearing("walker", HOME, 90.0);
            walked.source = BearingSource::Sweep;
            state.engine.publish_decoded(record("hunt", walked));
        },
    )
    .await;
    assert_eq!(reached.stations[0].source, BearingSource::Sweep);
    assert_eq!(
        state.fusion.state("blind").expect("blind").samples,
        0,
        "the filter feeds tri only"
    );
}

#[tokio::test]
async fn the_triangulation_position_input_drives_the_nav_target() {
    let state = bench(graph());
    let mut events = state.engine.subscribe_events();
    let guided = fused(
        &mut events,
        "tri",
        |fused| fused.nav.is_some(),
        || {
            state
                .engine
                .publish_decoded(record("df", bearing("roof", HOME, 60.0)));
        },
    )
    .await;
    let nav = guided.nav.expect("nav");
    assert_eq!(nav.kind, NavTargetKind::Probe);
    assert!((nav.bearing_deg - 60.0).abs() < 0.5, "{nav:?}");
    assert!((nav.distance_m - 5_000.0).abs() < 1.0, "{nav:?}");
    let blind = state.fusion.state("blind").expect("blind");
    assert!(blind.nav.is_none());
    assert!(blind.no_guide_position);

    let moved = geo::destination(HOME, 180.0, 120.0);
    let fix = sdrmm_wire::PositionFix {
        latitude: moved.lat,
        longitude: moved.lon,
        altitude_m: None,
        accuracy_m: Some(5.0),
        speed_mps: None,
        track_deg: None,
        time: jiff::Timestamp::now().to_string(),
        attitude: sdrmm_wire::Attitude::default(),
    };
    let revision = nav.revision;
    let followed = fused(
        &mut events,
        "tri",
        |fused| fused.nav.is_some_and(|nav| nav.revision > revision),
        || follow(&state, "gps", Some(&fix)),
    )
    .await;
    let target = followed.nav.expect("nav");
    let from = LatLon {
        lat: target.lat,
        lon: target.lon,
    };
    assert!((geo::distance_m(moved, from) - 5_000.0).abs() < 1.0);
}

fn identifier(letter: char) -> bool {
    letter.is_alphanumeric() || letter == '_'
}

#[test]
fn no_df_update_event_and_no_fix_fallback_remain() {
    let banned = [["Df", "Update"].concat(), ["any", "_fix"].concat()];
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root];
    let mut scanned = 0;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).expect("source file");
                scanned += 1;
                for word in &banned {
                    let hits = text
                        .match_indices(word.as_str())
                        .filter(|(at, _)| {
                            !text[..*at].ends_with(identifier)
                                && !text[at + word.len()..].starts_with(identifier)
                        })
                        .count();
                    assert_eq!(hits, 0, "{} names {word}", path.display());
                }
            }
        }
    }
    assert!(scanned > 50);
}

#[tokio::test]
async fn a_hunt_position_wire_feeds_hunt_poses() {
    let hunted = PatchGraph {
        nodes: vec![node("walk", "hunt"), fixed_gps("phone", HOME)],
        edges: vec![edge(("phone", "position"), ("walk", "position"))],
    };
    let state = bench(hunted.clone());
    let engine = state.engine.clone();
    let set = engine
        .create_device_set("virtual:siggen")
        .expect("virtual radio");
    let channel = engine
        .add_channel(
            set,
            0,
            ChannelSettings {
                frequency_hz: 100_100_000.0,
                squelch: sdrmm_wire::Squelch::Off,
                params: ChannelParams::default_for("nfm").expect("nfm"),
                blanker: Default::default(),
            },
        )
        .expect("decoder");
    let mut settings = HuntSettings::for_channel(channel);
    settings.node = Some("walk".to_owned());
    settings.sweep.mount_offset_deg = -90.0;
    engine
        .sweep_hunt(set, channel, Some(settings))
        .expect("sweeping hunt");
    assert!(engine.hunt_mark(set, channel).is_err(), "no pose yet");

    reconcile(&state, &hunted);
    let pose = sdrmm_wire::PositionFix {
        latitude: HOME.lat,
        longitude: HOME.lon,
        altitude_m: None,
        accuracy_m: Some(5.0),
        speed_mps: None,
        track_deg: None,
        time: jiff::Timestamp::now().to_string(),
        attitude: sdrmm_wire::Attitude {
            heading_deg: Some(100.0),
            ..sdrmm_wire::Attitude::default()
        },
    };
    follow(&state, "phone", Some(&pose));
    let marked = engine.hunt_mark(set, channel).expect("a pose with heading");
    assert!((marked.bearing_deg - 10.0).abs() < 1e-3, "{marked:?}");
    assert_eq!(marked.source, BearingSource::Mark);
    engine.stop_hunt(set, channel).expect("stop");
}

#[tokio::test]
async fn bearings_lost_before_routing_count_as_dropped() {
    let state = bench(graph());
    let mut events = state.engine.subscribe_events();
    state
        .decoded
        .send(Decoded::Lost(3))
        .expect("the router listens");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("an error in time")
            .expect("events");
        if let ServerEvent::Error { message } = event
            && message == "bearings lost: 3"
        {
            break;
        }
    }
    for triangulation in ["tri", "blind"] {
        assert_eq!(
            state
                .fusion
                .state(triangulation)
                .expect("configured")
                .dropped,
            3
        );
    }
}

#[test]
fn the_table_names_triangulations_and_their_guides() {
    let table = Table::from_graph(&graph());
    assert!(table.is_triangulation("tri") && table.is_triangulation("blind"));
    assert!(!table.is_triangulation("df"));
    assert_eq!(
        table.guided_by("gps"),
        [Guided::Triangulation("tri".to_owned())]
    );
    assert!(table.guided_by("nobody").is_empty());
}
