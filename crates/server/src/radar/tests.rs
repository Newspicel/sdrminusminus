use std::{sync::Arc, time::Duration};

use axum::{Router, http::StatusCode};
use sdrmm_dsp::radar::bistatic::{self, Geodetic};
use sdrmm_engine::ArrayEvent;
use sdrmm_wire::{
    AdsbTruth, ArrayNode, ArrayOrientation, DeviceNode, DeviceRef, GpsNode, PassiveRadarNode,
    PatchEdge, PatchNode, PortRef, Position, PositionSource, RadarAoa, RadarAxes, RadarDetection,
    RadarHealth, ReferenceHealth, TrackChange, TrackState, WorkspaceSnapshot, patch::stream_port,
    radar::LIGHT_SPEED_M_S,
};
use tokio::sync::broadcast::error::RecvError;

use super::{
    truth::{AircraftTable, Baseline, MAX_AIRCRAFT, associate, solver},
    *,
};
use crate::{
    ServerOptions, Store,
    events::Routed,
    router_with_state,
    tests::{request, state_over},
};

const WAIT: Duration = Duration::from_secs(10);
const QUIET: Duration = Duration::from_millis(300);
const ARRAY: &str = "arr";
const RADAR: &str = "radar";
const HEADING_DEG: f64 = 30.0;
const RX: Geodetic = Geodetic {
    lat_deg: 52.0,
    lon_deg: 13.0,
    alt_m: 0.0,
};
const TX: Geodetic = Geodetic {
    lat_deg: 52.27,
    lon_deg: 13.0,
    alt_m: 0.0,
};
const CARRIER_HZ: f64 = 94_500_000.0;
const CRUISE_FT: i32 = 32_000;

fn axes() -> RadarAxes {
    RadarAxes {
        sample_rate_hz: 200_000.0,
        carrier_hz: CARRIER_HZ,
        range_step_m: 750.0,
        gates: 200,
        doppler_step_hz: 1.0,
        doppler_rows: 401,
        batches: 512,
        cpi_ms: 500.0,
        hop_ms: 500.0,
        lanes: 4,
    }
}

fn wavelength_m() -> f64 {
    LIGHT_SPEED_M_S / CARRIER_HZ
}

fn stamp(at: Timestamp) -> String {
    format!("{at:.3}")
}

fn epoch() -> Timestamp {
    Timestamp::from_second(1_790_000_000).expect("a valid instant")
}

fn later(at: Timestamp, seconds: f64) -> Timestamp {
    let millis = (seconds * 1_000.0).round() as i64;
    Timestamp::from_millisecond(at.as_millisecond() + millis).expect("a valid instant")
}

fn cruise_alt_m() -> f64 {
    f64::from(CRUISE_FT) * 0.3048
}

fn position(icao: &str, at: Geodetic) -> AdsbMessage {
    AdsbMessage {
        icao: icao.to_owned(),
        df: 17,
        callsign: Some("DLH4AB ".to_owned()),
        altitude_ft: Some(CRUISE_FT),
        lat: Some(at.lat_deg),
        lon: Some(at.lon_deg),
        ..AdsbMessage::default()
    }
}

fn velocity(icao: &str, speed_kt: f64, track_deg: f64, climb_fpm: i32) -> AdsbMessage {
    AdsbMessage {
        icao: icao.to_owned(),
        df: 17,
        ground_speed_kt: Some(speed_kt),
        track_deg: Some(track_deg),
        vertical_rate_fpm: Some(climb_fpm),
        ..AdsbMessage::default()
    }
}

fn aircraft_at() -> Geodetic {
    Geodetic {
        lat_deg: 52.1,
        lon_deg: 13.3,
        alt_m: cruise_alt_m(),
    }
}

fn baseline() -> Baseline {
    Baseline { rx: RX, tx: TX }
}

fn range_m(point: Geodetic) -> f64 {
    bistatic::bistatic_range_m(
        bistatic::ecef(TX),
        bistatic::ecef(RX),
        bistatic::ecef(point),
    )
}

fn aoa(azimuth_deg: f32) -> Option<RadarAoa> {
    Some(RadarAoa {
        azimuth_deg,
        bearing_deg: None,
        sigma_deg: 2.0,
        quality: 0.9,
        mirror_deg: None,
    })
}

fn track(id: u32, range_km: f32, doppler_hz: f32, azimuth_deg: f32) -> RadarTrack {
    RadarTrack {
        id,
        state: TrackState::Confirmed,
        range_km,
        doppler_hz,
        range_sigma_m: 150.0,
        snr_db: 18.0,
        looks: 5,
        aoa: aoa(azimuth_deg),
        ..RadarTrack::default()
    }
}

fn event(track_id: u32, change: TrackChange) -> RadarTrackEvent {
    RadarTrackEvent {
        track_id,
        change,
        range_km: 24.0,
        range_rate_mps: -80.0,
        doppler_hz: 25.0,
        snr_db: 18.0,
        bearing_deg: None,
        lat: None,
        lon: None,
        icao: None,
    }
}

fn cpi(seq: u64, tracks: Vec<RadarTrack>, events: Vec<RadarTrackEvent>) -> RadarUpdate {
    RadarUpdate {
        seq,
        at: stamp(Timestamp::now()),
        axes: axes(),
        tracks,
        events,
        ..RadarUpdate::default()
    }
}

#[test]
fn truth_maps_an_aircraft_to_delay_and_doppler() {
    let mut table = AircraftTable::default();
    let now = epoch();
    let at = aircraft_at();
    let (speed_kt, track_deg, climb_fpm) = (450.0, 60.0, 1_200);
    table.observe(now, &position("3c6444", at));
    table.observe(now, &velocity("3c6444", speed_kt, track_deg, climb_fpm));

    let mut truth = Vec::new();
    table.truth(now, baseline(), &axes(), &mut truth);

    let [seen] = truth.as_slice() else {
        panic!("one aircraft in view, got {truth:?}");
    };
    let speed = speed_kt * 0.514_444;
    let (sin, cos) = f64::to_radians(track_deg).sin_cos();
    let enu = [speed * sin, speed * cos, f64::from(climb_fpm) * 0.005_08];
    let moved = |seconds: f64| {
        range_m(bistatic::geodetic_from_enu(
            at,
            [enu[0] * seconds, enu[1] * seconds, enu[2] * seconds],
        ))
    };
    let rate_mps = (moved(0.5) - moved(-0.5)) / 1.0;
    let doppler_hz = -rate_mps / wavelength_m();

    assert!((f64::from(seen.range_km) * 1_000.0 - range_m(at)).abs() < 1.0);
    assert!(
        (f64::from(seen.doppler_hz) - doppler_hz).abs() < 0.05,
        "{} Hz against {doppler_hz} Hz",
        seen.doppler_hz
    );
    assert_eq!(seen.icao, "3c6444");
    assert_eq!(seen.callsign.as_deref(), Some("DLH4AB"));
    assert!(seen.in_view);
    assert!((f64::from(seen.altitude_m) - cruise_alt_m()).abs() < 0.01);
    let bearing = bistatic::bearing_deg(RX, at);
    assert!((f64::from(seen.bearing_deg) - bearing).abs() < 0.01);

    table.truth(later(now, 4.0), baseline(), &axes(), &mut truth);
    assert!(
        (f64::from(truth[0].range_km) * 1_000.0 - moved(4.0)).abs() < 2.0,
        "a fresh position is carried forward by its velocity"
    );
    assert!((truth[0].age_s - 4.0).abs() < 1e-3);

    table.truth(later(now, 31.0), baseline(), &axes(), &mut truth);
    assert!(truth.is_empty(), "a position older than 30 s is no truth");
}

#[test]
fn truth_without_velocity_uses_the_range_history() {
    let mut table = AircraftTable::default();
    let start = aircraft_at();
    let step = |second: u32| {
        let point = bistatic::geodetic_from_enu(start, [220.0 * f64::from(second), 0.0, 0.0]);
        Geodetic {
            alt_m: cruise_alt_m(),
            ..point
        }
    };
    let mut truth = Vec::new();
    table.observe(epoch(), &position("4ca123", step(0)));
    table.truth(epoch(), baseline(), &axes(), &mut truth);
    assert!(truth.is_empty(), "one position gives no Doppler");

    for second in 1..=5 {
        table.observe(
            later(epoch(), f64::from(second)),
            &position("4ca123", step(second)),
        );
    }
    let now = later(epoch(), 5.0);
    table.truth(now, baseline(), &axes(), &mut truth);

    let [seen] = truth.as_slice() else {
        panic!("the aircraft has a history, got {truth:?}");
    };
    let middle_rate = range_m(step(3)) - range_m(step(2));
    assert!(
        (f64::from(seen.doppler_hz) + middle_rate / wavelength_m()).abs() < 1e-3,
        "the median of five range steps"
    );
    assert!((f64::from(seen.range_km) * 1_000.0 - range_m(step(5))).abs() < 1.0);
}

#[test]
fn the_aircraft_table_forgets_silence_and_stays_bounded() {
    let mut table = AircraftTable::default();
    for index in 0..=MAX_AIRCRAFT {
        let at = later(epoch(), index as f64 / 100.0);
        table.observe(at, &position(&format!("{index:06x}"), aircraft_at()));
    }
    assert_eq!(table.len(), MAX_AIRCRAFT);
    assert!(!table.knows("000000"), "the oldest entry made room");
    assert!(table.knows("000001") && table.knows(&format!("{MAX_AIRCRAFT:06x}")));

    table.prune(later(epoch(), 75.0));
    assert_eq!(table.len(), 0);
}

#[test]
fn truth_pairs_with_the_nearest_track() {
    let sighting = |icao: &str, range_km: f32, doppler_hz: f32, in_view: bool| AdsbTruth {
        icao: icao.to_owned(),
        range_km,
        doppler_hz,
        in_view,
        ..AdsbTruth::default()
    };
    let mut truth = vec![
        sighting("near", 20.0, 50.0, true),
        sighting("far", 60.0, -100.0, true),
        sighting("hidden", 20.1, 50.2, false),
    ];
    let mut tracks = vec![track(7, 20.8, 46.0, 0.0), track(9, 20.3, 52.0, 0.0)];
    let axes = RadarAxes {
        range_step_m: 1_000.0,
        ..axes()
    };

    associate(&mut solver(), &mut truth, &mut tracks, &axes);

    assert_eq!(truth[0].track_id, Some(9));
    assert_eq!(
        tracks[1].adsb.as_ref().map(|adsb| adsb.icao.as_str()),
        Some("near")
    );
    assert_eq!(tracks[0].adsb, None, "the farther track stays unpaired");
    assert_eq!(truth[1].track_id, None, "outside the gate");
    assert_eq!(truth[2].track_id, None, "out of view");
}

#[test]
fn problems_follow_the_pod_health() {
    let mut node = RadarNode::new(RadarBinding::default());
    let sites = Sites::default();
    let loaded = |load: f32| RadarUpdate {
        health: RadarHealth {
            load,
            aoa: AoaState::PhaseUnknown,
            reference: ReferenceHealth {
                mode: ReferenceMode::Cma,
                locked: false,
                ..ReferenceHealth::default()
            },
            ..RadarHealth::default()
        },
        ..cpi(1, Vec::new(), Vec::new())
    };
    let mut problems = Vec::new();
    for _ in 0..OVERLOAD_CPIS {
        let mut update = loaded(1.3);
        node.complete(&mut update, &sites);
        problems = update.problems;
    }
    assert_eq!(
        problems,
        [
            RadarProblem::NoTransmitter,
            RadarProblem::NoReceiver,
            RadarProblem::PhaseUnknown,
            RadarProblem::Overloaded,
            RadarProblem::ReferenceLost,
        ]
    );
    let mut calm = loaded(0.4);
    node.complete(&mut calm, &sites);
    assert!(!calm.problems.contains(&RadarProblem::Overloaded));

    node.binding.params.aoa = false;
    let mut blind = loaded(0.4);
    node.complete(&mut blind, &sites);
    assert!(!blind.problems.contains(&RadarProblem::PhaseUnknown));
}

#[test]
fn a_carrier_outside_the_array_table_is_a_problem() {
    let mut node = RadarNode::new(RadarBinding::default());
    let sites = Sites::default();
    let steered = |table_out_of_range| RadarUpdate {
        health: RadarHealth {
            aoa: AoaState::Ready,
            table_out_of_range,
            ..RadarHealth::default()
        },
        ..cpi(1, Vec::new(), Vec::new())
    };
    let mut outside = steered(true);
    node.complete(&mut outside, &sites);
    assert_eq!(
        outside.problems,
        [
            RadarProblem::NoTransmitter,
            RadarProblem::NoReceiver,
            RadarProblem::TableOutOfRange,
        ]
    );
    let mut inside = steered(false);
    node.complete(&mut inside, &sites);
    assert!(!inside.problems.contains(&RadarProblem::TableOutOfRange));
    node.binding.params.aoa = false;
    let mut blind = steered(true);
    node.complete(&mut blind, &sites);
    assert!(!blind.problems.contains(&RadarProblem::TableOutOfRange));
}

struct Wiring {
    array: bool,
    tx: bool,
    orientation: ArrayOrientation,
    params: PassiveRadarParams,
}

impl Default for Wiring {
    fn default() -> Self {
        Self {
            array: true,
            tx: true,
            orientation: ArrayOrientation::Fixed {
                azimuth_deg: HEADING_DEG,
            },
            params: PassiveRadarParams::default(),
        }
    }
}

impl Wiring {
    fn without_engine_reports() -> Self {
        Self {
            params: PassiveRadarParams {
                reference_element: 7,
                ..PassiveRadarParams::default()
            },
            ..Self::default()
        }
    }
}

fn node(id: &str, body: NodeBody) -> PatchNode {
    PatchNode {
        id: id.to_owned(),
        body,
        position: Position { x: 0.0, y: 0.0 },
        size: None,
        label: None,
    }
}

fn wire(from: (&str, &str), to: (&str, &str)) -> PatchEdge {
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

fn fixed_gps(id: &str, at: Geodetic) -> PatchNode {
    node(
        id,
        NodeBody::Gps(GpsNode {
            source: Some(PositionSource::Fixed {
                lat: at.lat_deg,
                lon: at.lon_deg,
                altitude_m: None,
            }),
        }),
    )
}

fn graph(wiring: Wiring) -> PatchGraph {
    let mut graph = PatchGraph {
        nodes: vec![
            node(
                "radio",
                NodeBody::Device(DeviceNode {
                    device: Some(DeviceRef {
                        backend: "virtual".to_owned(),
                        serial: None,
                        key: Some("kraken5".to_owned()),
                    }),
                    locked_streams: Vec::new(),
                }),
            ),
            node(
                ARRAY,
                NodeBody::Array(ArrayNode {
                    orientation: wiring.orientation,
                    ..ArrayNode::default()
                }),
            ),
            fixed_gps("rx", RX),
            fixed_gps("tx", TX),
            node(
                RADAR,
                NodeBody::PassiveRadar(PassiveRadarNode {
                    settings: wiring.params,
                }),
            ),
            node("log", NodeBody::DecoderLog),
        ],
        edges: (0..5)
            .map(|lane| {
                wire(
                    ("radio", &stream_port("iq", lane)),
                    (ARRAY, &stream_port("lane", lane)),
                )
            })
            .collect(),
    };
    graph
        .edges
        .push(wire(("rx", "position"), (ARRAY, "position")));
    graph.edges.push(wire((RADAR, "events"), ("log", "events")));
    if wiring.array {
        graph.edges.push(wire((ARRAY, "array"), (RADAR, "array")));
    }
    if wiring.tx {
        graph.edges.push(wire(("tx", "position"), (RADAR, "tx")));
    }
    graph
}

struct Bench {
    app: Router,
    state: AppState,
    workspace: i64,
}

impl Bench {
    fn new(graph: PatchGraph) -> Self {
        let store = Arc::new(Store::open(None).expect("store"));
        let snapshot = WorkspaceSnapshot {
            graph,
            ..WorkspaceSnapshot::empty()
        };
        let workspace = store.create_workspace("radar", &snapshot).expect("create");
        store.activate_workspace(workspace).expect("activate");
        let state = state_over(store);
        let (app, background) = router_with_state(state.clone(), &ServerOptions::default());
        background.detach();
        Self {
            app,
            state,
            workspace,
        }
    }

    async fn applied(graph: PatchGraph) -> Self {
        let bench = Self::new(graph);
        bench.apply().await;
        bench
    }

    async fn apply(&self) -> sdrmm_wire::PatchApplyReport {
        let (status, body) = request(
            self.app.clone(),
            "POST",
            &format!("/api/workspaces/{}/apply", self.workspace),
            Some("{}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
        serde_json::from_slice(&body).expect("apply report")
    }

    async fn call(&self, method: &str, uri: &str) -> (StatusCode, String) {
        let (status, body) = request(self.app.clone(), method, uri, None).await;
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn placed(&self) {
        wait_for("the array pose", || array::array_pose(&self.state, ARRAY)).await;
    }

    async fn sited(&self) {
        self.placed().await;
        wait_for("the transmitter fix", || self.state.gps.fix("tx")).await;
    }

    fn report(&self, update: RadarUpdate) -> RadarUpdate {
        self.state.radar.on_report(&self.state, RADAR, update)
    }

    fn pump(&self, update: RadarUpdate) {
        let reading = Arc::new(ProcessorReading::PassiveRadar(update));
        let flow = array::handle(
            &self.state,
            Ok(ArrayEvent::Report {
                processor: RADAR.to_owned(),
                reading,
            }),
        );
        assert!(flow.is_continue());
    }
}

async fn wait_for<T>(what: &str, mut found: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if let Some(found) = found() {
            return found;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn radar_update(
    events: &mut broadcast::Receiver<ServerEvent>,
    wanted: impl Fn(&RadarUpdate) -> bool,
) -> RadarUpdate {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(ServerEvent::ProcessorUpdate { node, reading }) if node == RADAR => {
                    if let ProcessorReading::PassiveRadar(update) = *reading
                        && wanted(&update)
                    {
                        return update;
                    }
                }
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => panic!("events closed"),
            }
        }
    })
    .await
    .expect("a radar update")
}

async fn radar_records(
    records: &mut broadcast::Receiver<DecodedRecord>,
    track_id: u32,
    wanted: usize,
) -> Vec<DecodedRecord> {
    let mut found = Vec::new();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let quiet = found.len() >= wanted;
        let until = if quiet {
            tokio::time::Instant::now() + QUIET
        } else {
            deadline
        };
        match tokio::time::timeout_at(until, records.recv()).await {
            Ok(Ok(record)) => {
                if matches!(&record.event, DecoderEvent::Radar(event) if event.track_id == track_id)
                {
                    found.push(record);
                }
            }
            Ok(Err(RecvError::Lagged(_))) => {}
            Ok(Err(RecvError::Closed)) => panic!("decoded feed closed"),
            Err(_) if quiet => return found,
            Err(_) => panic!("only {} of {wanted} radar records arrived", found.len()),
        }
    }
}

fn radar_event(record: &DecodedRecord) -> &RadarTrackEvent {
    match &record.event {
        DecoderEvent::Radar(event) => event,
        other => panic!("not a radar event: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_is_sent_every_cpi_even_when_empty() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    let mut events = bench.state.engine.subscribe_events();

    for seq in [9_001, 9_002] {
        bench.pump(cpi(seq, Vec::new(), Vec::new()));
        let update = radar_update(&mut events, |update| update.seq == seq).await;
        assert!(update.detections.is_empty() && update.tracks.is_empty());
        assert_eq!(update.axes, axes());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn lagged_updates_are_counted() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    let mut events = bench.state.engine.subscribe_events();

    assert!(array::handle(&bench.state, Err(RecvError::Lagged(5))).is_continue());
    bench.pump(cpi(9_101, Vec::new(), Vec::new()));
    let first = radar_update(&mut events, |update| update.seq == 9_101).await;
    assert_eq!(first.health.lagged_updates, 5);

    assert!(array::handle(&bench.state, Err(RecvError::Lagged(2))).is_continue());
    let second = bench.report(cpi(9_102, Vec::new(), Vec::new()));
    assert_eq!(second.health.lagged_updates, 7, "lost updates add up");
}

#[tokio::test(flavor = "multi_thread")]
async fn true_bearing_adds_the_array_heading() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    bench.sited().await;
    let mut update = cpi(9_201, vec![track(3, 24.0, 20.0, 350.0)], Vec::new());
    update.detections.push(RadarDetection {
        range_km: 24.0,
        doppler_hz: 20.0,
        aoa: aoa(350.0),
        ..RadarDetection::default()
    });

    let done = bench.report(update);

    let bearing = |aoa: Option<RadarAoa>| aoa.and_then(|aoa| aoa.bearing_deg);
    assert_eq!(bearing(done.detections[0].aoa), Some(20.0));
    assert_eq!(bearing(done.tracks[0].aoa), Some(20.0));
    assert_eq!(done.tracks[0].aoa.map(|aoa| aoa.azimuth_deg), Some(350.0));
    assert!(done.problems.is_empty(), "{:?}", done.problems);
    let geometry = done.geometry.expect("both sites known");
    assert_eq!(geometry.heading_deg, Some(30.0));
    assert!((geometry.baseline_km - 30.0).abs() < 0.5);

    let fix = done.tracks[0].fix.expect("a fix");
    let assumed = PassiveRadarParams::default().assumed_altitude_m;
    assert!(!fix.alt_from_adsb);
    assert_eq!(fix.alt_m, assumed);
    let at = Geodetic {
        lat_deg: fix.lat,
        lon_deg: fix.lon,
        alt_m: f64::from(assumed),
    };
    assert!((bistatic::bearing_deg(RX, at) - 20.0).abs() < 0.01);
    assert!(
        (range_m(at) - 24_000.0).abs() < 1.0,
        "the fix sits on the bistatic ellipsoid at the assumed altitude"
    );
    assert!(fix.major_m >= fix.minor_m && fix.minor_m > 0.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn no_tx_wire_reports_no_transmitter_and_no_fixes() {
    let bench = Bench::applied(graph(Wiring {
        tx: false,
        ..Wiring::default()
    }))
    .await;
    bench.placed().await;
    bench
        .state
        .radar
        .observe(RADAR, Timestamp::now(), &position("3c6444", aircraft_at()));

    let done = bench.report(cpi(9_301, vec![track(3, 24.0, 20.0, 10.0)], Vec::new()));

    assert_eq!(done.problems, [RadarProblem::NoTransmitter]);
    assert_eq!(done.geometry, None);
    assert!(done.truth.is_empty(), "no truth without a transmitter");
    assert_eq!(done.tracks[0].fix, None);
    assert_eq!(
        done.tracks[0].aoa.and_then(|aoa| aoa.bearing_deg),
        Some(40.0),
        "bearings need only the heading"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_heading_less_array_keeps_relative_azimuths() {
    let bench = Bench::applied(graph(Wiring {
        orientation: ArrayOrientation::Heading {
            mount_offset_deg: 0.0,
        },
        ..Wiring::default()
    }))
    .await;
    bench.sited().await;

    let done = bench.report(cpi(9_401, vec![track(3, 24.0, 20.0, 10.0)], Vec::new()));

    assert_eq!(done.problems, [RadarProblem::NoHeading]);
    assert_eq!(done.tracks[0].aoa.and_then(|aoa| aoa.bearing_deg), None);
    assert_eq!(done.tracks[0].fix, None);
    assert_eq!(
        done.geometry.and_then(|geometry| geometry.heading_deg),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn adsb_truth_pairs_and_lifts_the_fix_to_its_altitude() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    bench.sited().await;
    let now = Timestamp::now();
    for message in [
        position("3c6444", aircraft_at()),
        velocity("3c6444", 400.0, 270.0, 0),
    ] {
        let record = DecodedRecord {
            origin: None,
            device_set: 0,
            channel: 0,
            at: stamp(now),
            freq_hz: 1_090_000_000.0,
            event: DecoderEvent::Adsb(message),
            sinks: vec![RADAR.to_owned()],
        };
        bench
            .state
            .decoded
            .send(Decoded::Record(Box::new(Routed::unattributed(record))))
            .expect("the truth feed listens");
    }
    let seen = wait_for("the aircraft to be seen", || {
        let done = bench.report(cpi(9_501, Vec::new(), Vec::new()));
        done.truth.into_iter().next()
    })
    .await;
    assert!(seen.in_view);
    assert!(seen.doppler_hz.abs() > 1.0, "the velocity gives Doppler");

    let azimuth = (f64::from(seen.bearing_deg) - HEADING_DEG).rem_euclid(360.0) as f32;
    let done = bench.report(cpi(
        9_502,
        vec![track(5, seen.range_km, seen.doppler_hz, azimuth)],
        Vec::new(),
    ));

    let tracked = &done.tracks[0];
    assert_eq!(
        tracked.adsb.as_ref().map(|adsb| adsb.icao.as_str()),
        Some("3c6444")
    );
    assert_eq!(done.truth[0].track_id, Some(5));
    let fix = tracked.fix.expect("a fix");
    assert!(fix.alt_from_adsb);
    assert!((fix.alt_m - done.truth[0].altitude_m).abs() < 1.0);
    let found = Geodetic {
        lat_deg: fix.lat,
        lon_deg: fix.lon,
        alt_m: f64::from(fix.alt_m),
    };
    let target = Geodetic {
        lat_deg: done.truth[0].lat,
        lon_deg: done.truth[0].lon,
        alt_m: f64::from(done.truth[0].altitude_m),
    };
    let miss = bistatic::enu(target, bistatic::ecef(found));
    assert!(
        miss[0].hypot(miss[1]) < 150.0,
        "the 3D solve lands on the aircraft: {miss:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn track_events_fire_on_confirm_repeat_and_loss() {
    let bench = Bench::applied(graph(Wiring::without_engine_reports())).await;
    bench.sited().await;
    let mut records = bench.state.engine.subscribe_decoded();

    bench.report(cpi(
        9_601,
        vec![track(4_242, 24.0, 20.0, 10.0)],
        vec![event(4_242, TrackChange::Confirmed)],
    ));
    bench.report(cpi(
        9_602,
        vec![track(4_242, 24.2, 20.0, 10.0)],
        vec![event(4_242, TrackChange::Update)],
    ));
    bench.report(cpi(
        9_603,
        Vec::new(),
        vec![event(4_242, TrackChange::Lost)],
    ));

    let found = radar_records(&mut records, 4_242, 3).await;
    let changes: Vec<TrackChange> = found
        .iter()
        .map(|record| radar_event(record).change)
        .collect();
    assert_eq!(
        changes,
        [
            TrackChange::Confirmed,
            TrackChange::Update,
            TrackChange::Lost
        ]
    );
    for record in &found {
        let event = radar_event(record);
        assert_eq!(event.bearing_deg, Some(40.0));
        assert!(event.lat.is_some() && event.lon.is_some());
        assert_eq!(record.channel, NO_CHANNEL);
        assert_eq!(record.freq_hz, CARRIER_HZ);
        assert_eq!(
            record.origin.as_ref().map(|origin| origin.node.as_str()),
            Some(RADAR)
        );
    }
    assert_eq!(
        radar_event(&found[2]).lat,
        radar_event(&found[1]).lat,
        "a lost track keeps its last fix"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_track_event_reaches_a_sink_once_with_its_fix() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    bench.sited().await;
    let mut routed = bench.state.decoded.subscribe();
    let deadline = tokio::time::Instant::now() + WAIT;
    'warm: loop {
        bench.report(cpi(9_700, Vec::new(), vec![event(1, TrackChange::Update)]));
        let until = tokio::time::Instant::now() + Duration::from_millis(200);
        while let Ok(Ok(item)) = tokio::time::timeout_at(until, routed.recv()).await {
            if let Decoded::Record(record) = item
                && record.record.sinks == ["log"]
            {
                break 'warm;
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "routes never loaded"
        );
    }
    while routed.try_recv().is_ok() {}

    bench.report(cpi(
        9_701,
        vec![track(77, 24.0, 20.0, 10.0)],
        vec![event(77, TrackChange::Confirmed)],
    ));

    let mut delivered = Vec::new();
    while let Ok(Ok(item)) = tokio::time::timeout(QUIET * 2, routed.recv()).await {
        if let Decoded::Record(record) = item
            && matches!(&record.record.event, DecoderEvent::Radar(event) if event.track_id == 77)
        {
            delivered.push(record);
        }
    }
    let [only] = delivered.as_slice() else {
        panic!("one record per event, got {}", delivered.len());
    };
    assert_eq!(only.record.sinks, ["log"]);
    assert_eq!(only.source.as_deref(), Some(RADAR));
    let event = radar_event(&only.record);
    assert_eq!(event.change, TrackChange::Confirmed);
    assert_eq!(event.bearing_deg, Some(40.0));
    let (lat, lon) = (
        event.lat.expect("a latitude"),
        event.lon.expect("a longitude"),
    );
    assert!((lat - RX.lat_deg).abs() < 0.5 && (lon - RX.lon_deg).abs() < 0.5);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unwired_radar_reports_no_array() {
    let bench = Bench::new(graph(Wiring {
        array: false,
        ..Wiring::default()
    }));
    let mut events = bench.state.engine.subscribe_events();
    bench.apply().await;

    let sent = radar_update(&mut events, |update| !update.problems.is_empty()).await;
    assert_eq!(sent.problems, [RadarProblem::NoArray]);
    assert_eq!(sent.axes, RadarAxes::default());

    let (status, body) = bench.call("GET", "/api/radar/radar").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let served: RadarUpdate = serde_json::from_str(&body).expect("a radar update");
    assert_eq!(served.problems, [RadarProblem::NoArray]);

    bench.apply().await;
    assert!(
        tokio::time::timeout(
            QUIET,
            radar_update(&mut events, |update| !update.problems.is_empty())
        )
        .await
        .is_err(),
        "an unchanged refusal is told once"
    );

    let (status, body) = bench.call("DELETE", "/api/radar/radar/tracks").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("Not running"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_refusal_is_a_problem() {
    let bench = Bench::new(graph(Wiring::without_engine_reports()));
    let mut events = bench.state.engine.subscribe_events();
    let report = bench.apply().await;

    let refused = report
        .refused
        .iter()
        .find(|refusal| refusal.node == RADAR)
        .map(|refusal| refusal.reason.clone())
        .expect("the apply report names the radar");
    let sent = radar_update(&mut events, |update| !update.problems.is_empty()).await;
    assert_eq!(sent.problems, [RadarProblem::Refused(refused.clone())]);
    assert_eq!(
        bench
            .state
            .radar
            .latest(RADAR)
            .map(|update| update.problems),
        Some(vec![RadarProblem::Refused(refused.clone())])
    );

    let (status, body) = bench.call("DELETE", "/api/radar/radar/tracks").await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains(&refused), "the engine text: {body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_tracks_resets_the_tracker() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    bench.sited().await;
    let tracked = bench.report(cpi(9_801, vec![track(12, 24.0, 20.0, 10.0)], Vec::new()));
    assert_eq!(tracked.tracks.len(), 1);

    let (status, body) = bench.call("DELETE", "/api/radar/radar/tracks").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let latest = bench.state.radar.latest(RADAR).expect("a picture");
    assert!(latest.tracks.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_radar_left_behind_is_forgotten() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    let (status, _) = bench.call("GET", "/api/radar/radar").await;
    assert_eq!(status, StatusCode::OK);
    let other = bench
        .state
        .store
        .create_workspace("empty", &WorkspaceSnapshot::empty())
        .expect("create");

    let (status, body) = bench
        .call("POST", &format!("/api/workspaces/{other}/activate"))
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = bench.call("GET", "/api/radar/radar").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a switch forgets the radar");

    let late = bench.report(cpi(
        9_901,
        vec![track(8, 24.0, 20.0, 10.0)],
        vec![event(8, TrackChange::Confirmed)],
    ));
    assert!(late.events.is_empty());
    let (status, _) = bench.call("GET", "/api/radar/radar").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a late report does not revive it"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_radar_is_404() {
    let bench = Bench::applied(graph(Wiring::default())).await;
    let (status, body) = bench.call("GET", "/api/radar/ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no radar ghost"), "{body}");
    let (status, body) = bench.call("DELETE", "/api/radar/ghost/tracks").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("no radar ghost"), "{body}");

    let (status, _) = bench.call("GET", "/api/radar/radar").await;
    assert_eq!(status, StatusCode::OK);
}
