use std::{future::IntoFuture, sync::Arc};

use futures::{SinkExt, StreamExt};
use sdrmm_wire::{
    ClientCommand, FusionGridFrame, RangeDopplerFrame, SpatialSpectrumFrame, VisibilityFrame,
};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite;

use super::*;
use crate::{
    AppState,
    surfaces::{SURFACE_BACKLOG, SurfaceHub},
    ws::{MEDIA_ID_BASE, outbox},
};

const WAIT: Duration = Duration::from_secs(5);

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve() -> (Client, AppState) {
    let mut registry = sdrmm_device::DeviceRegistry::new();
    registry.register(1, Box::new(sdrmm_device_virtual::VirtualDriver::new()));
    let engine = sdrmm_engine::Engine::with_registry(registry, None);
    let store = Arc::new(crate::Store::open(None).expect("in-memory store"));
    let state = AppState::new(engine, store);
    let (app, background) =
        crate::router_with_state(state.clone(), &crate::ServerOptions::default());
    background.detach();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(axum::serve(listener, app).into_future());
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws"))
        .await
        .expect("connect");
    assert!(matches!(
        next_event(&mut client).await,
        ServerEvent::Hello { .. }
    ));
    (client, state)
}

async fn send(client: &mut Client, command: &ClientCommand) {
    let json = serde_json::to_string(command).expect("command json");
    client
        .send(tungstenite::Message::text(json))
        .await
        .expect("send");
}

async fn subscribe(client: &mut Client, node: &str, fit: Option<SurfaceFit>) {
    let command = ClientCommand::SubscribeSurface {
        node: node.to_owned(),
        fit,
    };
    send(client, &command).await;
}

async fn next_message(client: &mut Client) -> tungstenite::Message {
    timeout(WAIT, client.next())
        .await
        .expect("timed out waiting for the socket")
        .expect("socket ended")
        .expect("socket error")
}

async fn next_event(client: &mut Client) -> ServerEvent {
    loop {
        if let tungstenite::Message::Text(text) = next_message(client).await {
            return serde_json::from_str(text.as_str()).expect("event json");
        }
    }
}

async fn next_binary(client: &mut Client) -> Vec<u8> {
    loop {
        if let tungstenite::Message::Binary(bytes) = next_message(client).await {
            return bytes.to_vec();
        }
    }
}

async fn started(client: &mut Client) -> (u16, String, StreamKind) {
    match next_event(client).await {
        ServerEvent::SurfaceStreamStarted {
            stream_id,
            node,
            kind,
        } => (stream_id, node, kind),
        other => panic!("expected SurfaceStreamStarted, got {other:?}"),
    }
}

async fn refusal(client: &mut Client) -> (String, SurfaceRefusal) {
    match next_event(client).await {
        ServerEvent::SurfaceRefused { node, reason } => (node, reason),
        other => panic!("expected a surface refusal, got {other:?}"),
    }
}

fn refused(node: &str, reason: SurfaceRefusal) -> (String, SurfaceRefusal) {
    (node.to_owned(), reason)
}

fn fit(cols: u16, rows: u16) -> Option<SurfaceFit> {
    Some(SurfaceFit { cols, rows })
}

fn ramp(len: usize) -> Vec<u8> {
    (0..len)
        .map(|cell| u8::try_from(cell % 251).expect("small"))
        .collect()
}

fn radar(ranges: u16, dopplers: u16) -> RangeDopplerOwned {
    RangeDopplerOwned {
        stream_id: 0,
        seq: 41,
        timestamp: 1_700_000_000_000,
        ranges,
        dopplers,
        range_first_m: 0.0,
        range_step_m: 1_000.0,
        doppler_first_hz: -100.0,
        doppler_step_hz: 2.0,
        carrier_hz: 94_500_000.0,
        db_min: -3.0,
        db_max: 30.0,
        cells: ramp(usize::from(ranges) * usize::from(dopplers)),
    }
}

fn grid(seq: u32, cols: u16, rows: u16) -> FusionGridOwned {
    FusionGridOwned {
        stream_id: 0,
        seq,
        timestamp: 1_700_000_000_000,
        south: 52.0,
        west: 13.0,
        north: 52.4,
        east: 13.6,
        cols,
        rows,
        cells: ramp(usize::from(cols) * usize::from(rows)),
    }
}

fn spatial_frame() -> SpatialSpectrumOwned {
    SpatialSpectrumOwned {
        stream_id: 0,
        seq: 3,
        timestamp: 1_700_000_000_000,
        center_hz: 433_920_000.0,
        span_hz: 1_000.0,
        bearings: 12,
        bins: 10,
        db_min: -80.0,
        db_max: -20.0,
        cells: ramp(120),
    }
}

fn visibility_frame() -> VisibilityOwned {
    VisibilityOwned {
        stream_id: 0,
        seq: 4,
        timestamp: 1_700_000_000_000,
        center_hz: 1_420_405_751.0,
        span_hz: 800.0,
        baselines: 3,
        bins: 8,
        db_min: -60.0,
        db_max: 0.0,
        amplitude: vec![
            1, 9, 2, 3, 7, 7, 0, 5, 4, 4, 4, 4, 8, 1, 1, 8, 0, 0, 0, 0, 2, 6, 6, 2,
        ],
        phase: (0..24).map(|cell| cell * 10).collect(),
    }
}

#[test]
fn surface_fit_max_pools_and_rescales_axes() {
    let frame = RangeDopplerOwned {
        cells: [[1, 7, 3, 0, 2], [4, 0, 9, 1, 1], [0, 5, 0, 6, 0]].concat(),
        ..radar(5, 3)
    };

    let Some(SurfaceFrame::RangeDoppler(pooled)) = fitted(
        &SurfaceFrame::RangeDoppler(frame),
        SurfaceFit { cols: 2, rows: 2 },
    ) else {
        panic!("a bigger frame is pooled");
    };

    assert_eq!((pooled.ranges, pooled.dopplers), (2, 2));
    assert_eq!(pooled.cells, vec![9, 2, 5, 6]);
    assert_eq!(pooled.range_step_m, 3_000.0);
    assert_eq!(
        pooled.range_first_m, 1_000.0,
        "centre of the first three gates"
    );
    assert_eq!(pooled.doppler_step_hz, 4.0);
    assert_eq!(
        pooled.doppler_first_hz, -99.0,
        "centre of the first two rows"
    );
    assert_eq!(
        (pooled.seq, pooled.timestamp, pooled.carrier_hz),
        (41, 1_700_000_000_000, 94_500_000.0)
    );
    assert_eq!((pooled.db_min, pooled.db_max), (-3.0, 30.0));
}

#[test]
fn a_frame_that_fits_is_sent_as_is() {
    let frame = SurfaceFrame::RangeDoppler(radar(4, 2));
    assert!(fitted(&frame, SurfaceFit { cols: 4, rows: 2 }).is_none());
    assert_eq!(
        encode(&frame, 9, Some(SurfaceFit { cols: 64, rows: 64 })),
        frame.encode(9)
    );
}

#[test]
fn a_bearing_axis_pools_by_a_whole_divisor() {
    let bearings = Axis::circular(12, 5);
    assert_eq!(
        bearings.factor, 3,
        "12 bearings into 4 rows, not 3 uneven ones"
    );
    assert_eq!(bearings.len(), 4);
    assert_eq!(Axis::circular(7, 3).factor, 7);
}

#[test]
fn a_pooled_bearing_gathers_the_bearings_around_it() {
    let bearings = Axis::circular(12, 4);
    let slots: Vec<Option<usize>> = (0..12).map(|index| bearings.slot(index)).collect();
    assert_eq!(
        slots,
        [0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 0].map(Some),
        "330 deg wraps to north, 150 deg joins 180 deg"
    );
    assert_eq!(Axis::fitted(12, 4).slot(11), Some(3));
}

#[test]
fn a_grid_that_does_not_divide_keeps_its_cell_size() {
    let Some(SurfaceFrame::FusionGrid(pooled)) = fitted(
        &SurfaceFrame::FusionGrid(grid(1, 5, 4)),
        SurfaceFit { cols: 2, rows: 2 },
    ) else {
        panic!("pooled");
    };
    assert_eq!((pooled.cols, pooled.rows), (2, 2));
    assert_eq!(pooled.north, 52.4);
    assert_eq!(pooled.west, 13.0);
    assert!((pooled.south - 52.0).abs() < 1e-12);
    assert!(
        (pooled.east - (13.0 + 0.6 * 6.0 / 5.0)).abs() < 1e-12,
        "two pooled columns of three cells reach past the old east edge"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn radar_surface_is_max_pooled_to_the_fit() {
    let (mut client, state) = serve().await;
    state.surfaces.open("radar", StreamKind::RangeDoppler);

    subscribe(&mut client, "radar", fit(4, 2)).await;
    let (stream_id, node, kind) = started(&mut client).await;
    assert_eq!((node.as_str(), kind), ("radar", StreamKind::RangeDoppler));
    assert!(stream_id >= MEDIA_ID_BASE);

    let frame = radar(8, 4);
    state.surfaces.publish(
        "radar",
        41,
        Arc::new(SurfaceFrame::RangeDoppler(frame.clone())),
    );
    let bytes = next_binary(&mut client).await;
    let sent = RangeDopplerFrame::decode(&bytes).expect("a range Doppler frame");

    assert_eq!(sent.stream_id, stream_id);
    assert_eq!((sent.ranges, sent.dopplers), (4, 2));
    assert_eq!(sent.range_step_m, 2_000.0);
    assert_eq!(sent.range_first_m, 500.0);
    assert_eq!(sent.doppler_step_hz, 4.0);
    assert_eq!(sent.doppler_first_hz, -99.0);
    let expected: Vec<u8> = (0..2)
        .flat_map(|row| (0..4).map(move |col| (row, col)))
        .map(|(row, col)| {
            let mut peak = 0;
            for doppler in row * 2..row * 2 + 2 {
                for range in col * 2..col * 2 + 2 {
                    peak = peak.max(frame.cells[doppler * 8 + range]);
                }
            }
            peak
        })
        .collect();
    assert_eq!(sent.cells, expected.as_slice());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_zero_fit_is_refused() {
    let (mut client, state) = serve().await;
    state.surfaces.open("radar", StreamKind::RangeDoppler);

    subscribe(&mut client, "radar", fit(0, 128)).await;
    assert_eq!(
        refusal(&mut client).await,
        refused("radar", SurfaceRefusal::FitNotPositive)
    );
    subscribe(&mut client, "radar", fit(256, 0)).await;
    assert_eq!(
        refusal(&mut client).await,
        refused("radar", SurfaceRefusal::FitNotPositive)
    );

    subscribe(&mut client, "radar", fit(256, 128)).await;
    assert_eq!(started(&mut client).await.1, "radar");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_surface_node_is_refused_by_name() {
    let (mut client, state) = serve().await;
    subscribe(&mut client, "ghost", None).await;
    assert_eq!(
        refusal(&mut client).await,
        refused("ghost", SurfaceRefusal::NoSurface)
    );

    state.surfaces.open("ghost", StreamKind::FusionGrid);
    subscribe(&mut client, "ghost", None).await;
    assert_eq!(started(&mut client).await.1, "ghost", "asking again works");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_socket_out_of_stream_ids_refuses_the_surface_by_name() {
    let (_client, state) = serve().await;
    state.surfaces.open("radar", StreamKind::RangeDoppler);
    let (out, mut output) = outbox::channel();
    let mut session = Session::new(
        state.engine.clone(),
        state.clone(),
        out,
        crate::auth::Identity::Open,
    );
    for id in MEDIA_ID_BASE..=u16::MAX {
        let task = tokio::spawn(async {});
        session.video.insert((u32::from(id), 0), (id, task));
    }

    session.subscribe_surface("radar".to_owned(), None).await;
    let message = timeout(WAIT, output.recv())
        .await
        .expect("an answer")
        .expect("the outbox is open");
    let Message::Text(text) = message else {
        panic!("a text event, got {message:?}");
    };
    assert_eq!(
        serde_json::from_str::<ServerEvent>(text.as_str()).expect("event json"),
        ServerEvent::SurfaceRefused {
            node: "radar".to_owned(),
            reason: SurfaceRefusal::NoStreamIds,
        }
    );
    assert!(session.surfaces.streams.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn fusion_grid_frames_reach_a_subscriber() {
    let (mut client, state) = serve().await;
    state.surfaces.open("tri", StreamKind::FusionGrid);

    subscribe(&mut client, "tri", None).await;
    let (stream_id, _, kind) = started(&mut client).await;
    assert_eq!(kind, StreamKind::FusionGrid);

    let frame = grid(12, 128, 128);
    state
        .surfaces
        .publish("tri", 12, Arc::new(SurfaceFrame::FusionGrid(frame.clone())));
    let bytes = next_binary(&mut client).await;
    let sent = FusionGridFrame::decode(&bytes).expect("a fusion grid frame");
    assert_eq!(sent.stream_id, stream_id);
    assert_eq!(sent.seq, 12);
    assert_eq!((sent.cols, sent.rows), (128, 128));
    assert_eq!(sent.cells, frame.cells.as_slice());

    subscribe(&mut client, "tri", None).await;
    match next_event(&mut client).await {
        ServerEvent::StreamStopped {
            stream_id: stopped,
            kind,
        } => assert_eq!((stopped, kind), (stream_id, StreamKind::FusionGrid)),
        other => panic!("a second subscribe stops the first, got {other:?}"),
    }
    let (again, _, _) = started(&mut client).await;
    assert_ne!(again, stream_id);

    send(
        &mut client,
        &ClientCommand::UnsubscribeSurface {
            node: "tri".to_owned(),
        },
    )
    .await;
    assert!(matches!(
        next_event(&mut client).await,
        ServerEvent::StreamStopped { stream_id, .. } if stream_id == again
    ));

    subscribe(&mut client, "tri", None).await;
    let (last, _, _) = started(&mut client).await;
    state.surfaces.forget("tri");
    assert!(matches!(
        next_event(&mut client).await,
        ServerEvent::StreamStopped { stream_id, kind: StreamKind::FusionGrid } if stream_id == last
    ));

    state.surfaces.open("tri", StreamKind::FusionGrid);
    subscribe(&mut client, "tri", None).await;
    assert!(
        matches!(
            next_event(&mut client).await,
            ServerEvent::SurfaceStreamStarted { .. }
        ),
        "an ended stream is not stopped twice"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn spatial_and_visibility_frames_encode() {
    let (mut client, state) = serve().await;
    state.surfaces.open("spatial", StreamKind::SpatialSpectrum);
    state.surfaces.open("corr", StreamKind::Visibility);

    subscribe(&mut client, "spatial", fit(5, 5)).await;
    let (spatial_id, _, spatial_kind) = started(&mut client).await;
    subscribe(&mut client, "corr", fit(4, 1)).await;
    let (corr_id, _, corr_kind) = started(&mut client).await;
    assert_eq!(
        (spatial_kind, corr_kind),
        (StreamKind::SpatialSpectrum, StreamKind::Visibility)
    );

    state.surfaces.publish(
        "spatial",
        3,
        Arc::new(SurfaceFrame::SpatialSpectrum(spatial_frame())),
    );
    let bytes = next_binary(&mut client).await;
    let spatial = SpatialSpectrumFrame::decode(&bytes).expect("a spatial frame");
    assert_eq!(spatial.stream_id, spatial_id);
    assert_eq!((spatial.bearings, spatial.bins), (4, 5));
    assert_eq!(spatial.cells.len(), 20);
    assert_eq!(
        spatial.cells[0], 111,
        "max of bearings 11, 0 and 1, bins 0 and 1"
    );
    assert_eq!(spatial.center_hz, 433_920_000.0);
    assert_eq!(spatial.span_hz, 1_000.0);

    state.surfaces.publish(
        "corr",
        4,
        Arc::new(SurfaceFrame::Visibility(visibility_frame())),
    );
    let bytes = next_binary(&mut client).await;
    let visibility = VisibilityFrame::decode(&bytes).expect("a visibility frame");
    assert_eq!(visibility.stream_id, corr_id);
    assert_eq!(
        (visibility.baselines, visibility.bins),
        (3, 4),
        "baselines stay apart"
    );
    assert_eq!(visibility.amplitude, &[9, 3, 7, 5, 4, 4, 8, 8, 0, 0, 6, 6]);
    assert_eq!(
        visibility.phase,
        &[10, 30, 40, 70, 80, 100, 120, 150, 160, 180, 210, 220],
        "each pooled cell keeps the phase of its strongest bin"
    );
}

async fn collect_until(output: &mut outbox::Output, marker: u32) -> (Vec<String>, Vec<u32>) {
    let mut errors = Vec::new();
    let mut seqs = Vec::new();
    loop {
        let message = timeout(WAIT, output.recv())
            .await
            .expect("the forwarder keeps sending")
            .expect("the outbox is open");
        match message {
            Message::Text(text) => {
                if let Ok(ServerEvent::Error { message }) = serde_json::from_str(text.as_str()) {
                    errors.push(message);
                }
            }
            Message::Binary(bytes) => {
                let seq = FusionGridFrame::decode(&bytes).expect("a grid").seq;
                seqs.push(seq);
                if seq == marker {
                    return (errors, seqs);
                }
            }
            _ => {}
        }
    }
}

fn flood(hub: &SurfaceHub, from: u32, count: u32) -> u32 {
    for seq in from..from + count {
        hub.publish(
            "tri",
            seq,
            Arc::new(SurfaceFrame::FusionGrid(grid(seq, 2, 2))),
        );
    }
    from + count - 1
}

#[tokio::test(start_paused = true)]
async fn a_lagging_surface_subscriber_is_told() {
    let hub = SurfaceHub::default();
    hub.open("tri", StreamKind::FusionGrid);
    let (kind, frames) = hub.subscribe("tri").expect("an open surface");
    let (out, mut output) = outbox::channel();
    let stream = SurfaceStream {
        stream_id: 0x8001,
        kind,
        fit: None,
    };
    let burst = u32::try_from(SURFACE_BACKLOG).expect("small backlog") + 6;

    let last = flood(&hub, 0, burst);
    let task = spawn(stream, frames, out);
    let (errors, seqs) = collect_until(&mut output, last).await;
    assert_eq!(errors, ["surface frames skipped: 6"]);
    assert_eq!(seqs.last(), Some(&last), "the newest frame still goes out");

    tokio::time::advance(Duration::from_secs(1)).await;
    let last = flood(&hub, 100, burst);
    let (errors, _) = collect_until(&mut output, last).await;
    assert!(errors.is_empty(), "told at most once per 5 s: {errors:?}");

    tokio::time::advance(SKIPS_TOLD_EVERY).await;
    let last = flood(&hub, 200, burst);
    let (errors, _) = collect_until(&mut output, last).await;
    assert_eq!(
        errors,
        ["surface frames skipped: 18"],
        "the count sums every skip of the stream"
    );
    task.abort();
}
