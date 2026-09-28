use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use futures::{SinkExt, StreamExt, future::BoxFuture};
use sdrmm_wire::{
    about::{API_PROTOCOL, AboutResponse},
    frame::{RangeDopplerOwned, SpectrumFrame},
    fusion::DfFusionState,
    mission::{MissionAction, MissionActionResponse, MissionsResponse},
    phone::PhoneSelf,
    radar::RadarUpdate,
    state::StateSnapshot,
    survey::SurveyGrid,
    ws::{ClientCommand, ServerEvent, StateScope, SurfaceFit, WS_CLOSE_REVOKED},
};
use tokio::{
    io::DuplexStream,
    sync::{mpsc, oneshot, watch},
    time::Instant,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Message,
        protocol::{CloseFrame, Role, frame::coding::CloseCode},
    },
};

use super::{
    socket::{Connection, DEAD_AFTER, PING_EVERY, POSE_GAP, TlsDialer, upgrade_request},
    *,
};
use crate::{
    events::{EventQueue, Pop},
    pose::PoseOut,
    records::{LinkState, Platform, RefusalKind},
    stub_server::{StubHandler, StubRequest, StubResponse, StubServer, StubSocket},
    vault::testing::{self, MemoryVault},
};

struct NoApi;

impl Api for NoApi {
    fn missions(&self) -> BoxFuture<'_, Result<MissionsResponse, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn phone_self(&self) -> BoxFuture<'_, Result<PhoneSelf, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn act(
        &self,
        _node: String,
        _action: MissionAction,
    ) -> BoxFuture<'_, Result<MissionActionResponse, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn switch_workspace(&self, _id: i64) -> BoxFuture<'_, Result<MissionsResponse, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn radar(&self, _node: String) -> BoxFuture<'_, Result<RadarUpdate, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn survey(&self, _node: String) -> BoxFuture<'_, Result<SurveyGrid, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn fusion(&self, _node: String) -> BoxFuture<'_, Result<DfFusionState, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }

    fn state(&self) -> BoxFuture<'_, Result<StateSnapshot, RestError>> {
        Box::pin(async { Err(RestError::TimedOut) })
    }
}

type Script = Arc<Mutex<HashMap<String, VecDeque<Result<(), DialError>>>>>;

#[derive(Clone, Default)]
struct FakeDialer {
    script: Script,
    calls: Arc<Mutex<Vec<(String, Instant)>>>,
    ends: Arc<Mutex<Vec<oneshot::Sender<DialError>>>>,
    sessions: Arc<Mutex<Vec<Arc<Session>>>>,
    stops: Arc<Mutex<u32>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl FakeDialer {
    fn plan(&self, host: &str, outcomes: impl IntoIterator<Item = Result<(), DialError>>) {
        lock(&self.script)
            .entry(host.to_owned())
            .or_default()
            .extend(outcomes);
    }

    fn calls(&self) -> Vec<(String, Instant)> {
        lock(&self.calls).clone()
    }

    fn end(&self, error: DialError) {
        if let Some(end) = lock(&self.ends).pop() {
            let _ = end.send(error);
        }
    }

    fn stops(&self) -> u32 {
        *lock(&self.stops)
    }
}

impl Dialer for FakeDialer {
    fn dial(
        &self,
        record: Arc<ServerRecord>,
        host: String,
    ) -> BoxFuture<'static, Result<Dialed, DialError>> {
        lock(&self.calls).push((host.clone(), Instant::now()));
        let outcome = lock(&self.script)
            .get_mut(&host)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Err(DialError::Unreachable("refused".to_owned())));
        let this = self.clone();
        Box::pin(async move {
            outcome?;
            let (end, ended) = oneshot::channel();
            let (stop, stopped) = oneshot::channel();
            lock(&this.ends).push(end);
            let session = Arc::new(Session::new(
                "Shack".to_owned(),
                record.phone_id.clone(),
                host,
                Arc::new(NoApi),
            ));
            lock(&this.sessions).push(session.clone());
            let stops = this.stops.clone();
            let run: BoxFuture<'static, DialError> = Box::pin(async move {
                tokio::select! {
                    error = ended => error.unwrap_or(DialError::Closed("gone".to_owned())),
                    _ = stopped => {
                        *lock(&stops) += 1;
                        DialError::Closed("stopped".to_owned())
                    }
                }
            });
            Ok(Dialed { session, run, stop })
        })
    }
}

struct Rig {
    events: EventQueue,
    wires: Wires,
    inbound: mpsc::Receiver<Inbound>,
    subs: watch::Sender<Subscriptions>,
    pose: watch::Sender<Option<PoseOut>>,
    activity: watch::Sender<Activity>,
    sessions: watch::Receiver<Option<Arc<Session>>>,
    clock: std::time::Instant,
}

impl Rig {
    fn new() -> Self {
        let events = EventQueue::default();
        let (inbound_tx, inbound) = mpsc::channel(INBOUND_CAPACITY);
        let (sessions_tx, sessions) = watch::channel(None);
        let (subs, subs_rx) = watch::channel(Subscriptions::new());
        let (pose, pose_rx) = watch::channel(None);
        let (activity, activity_rx) = watch::channel(Activity::default());
        let wires = Wires {
            events: events.clone(),
            inbound: inbound_tx,
            sessions: sessions_tx,
            subs: subs_rx,
            pose: pose_rx,
            activity: activity_rx,
            net: Net::new(Platform::Ios),
        };
        Self {
            events,
            wires,
            inbound,
            subs,
            pose,
            activity,
            sessions,
            clock: std::time::Instant::now(),
        }
    }

    fn drain(&mut self) -> Vec<CoreEvent> {
        self.clock += Duration::from_secs(3_600);
        let mut out = Vec::new();
        while let Pop::Event(event) = self.events.pop(self.clock) {
            out.push(*event);
        }
        out
    }

    fn states(&mut self) -> Vec<LinkState> {
        self.drain()
            .into_iter()
            .filter_map(|event| match event {
                CoreEvent::Link { state } => Some(state),
                _ => None,
            })
            .collect()
    }

    fn last_state(&mut self) -> Option<LinkState> {
        self.states().pop()
    }

    fn notices(&mut self) -> Vec<String> {
        self.drain()
            .into_iter()
            .filter_map(|event| match event {
                CoreEvent::Notice { notice } => Some(notice.text),
                _ => None,
            })
            .collect()
    }

    fn start(&self, dialer: FakeDialer, vault: Option<Vault>) -> LinkHandle {
        start(
            &Handle::current(),
            testing::record(),
            dialer,
            self.wires.clone(),
            vault,
        )
    }
}

async fn settle(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

fn online() -> LinkState {
    LinkState::Online {
        server: "Shack".to_owned(),
    }
}

const FIRST: &str = "192.168.1.20:8443";
const SECOND: &str = "[fe80::1]:8443";

#[tokio::test(start_paused = true)]
async fn a_401_is_terminal_unauthorized() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Err(DialError::Revoked)]);
    let _link = rig.start(dialer.clone(), None);
    settle(120).await;
    assert_eq!(
        rig.last_state(),
        Some(LinkState::Refused {
            reason: RefusalKind::Revoked,
            text: "Removed. Pair again".to_owned()
        })
    );
    assert_eq!(dialer.calls().len(), testing::record().hosts.len());
}

#[tokio::test(start_paused = true)]
async fn a_protocol_mismatch_is_terminal() {
    let cases = [
        (API_PROTOCOL + 1, RefusalKind::AppTooOld),
        (0, RefusalKind::ServerTooOld),
    ];
    for (server, reason) in cases {
        let mut rig = Rig::new();
        let dialer = FakeDialer::default();
        dialer.plan(FIRST, [Err(DialError::Protocol { server })]);
        let _link = rig.start(dialer.clone(), None);
        settle(120).await;
        assert!(matches!(
            rig.last_state(),
            Some(LinkState::Refused { reason: seen, .. }) if seen == reason
        ));
        assert_eq!(dialer.calls().len(), testing::record().hosts.len());
    }
}

#[tokio::test(start_paused = true)]
async fn a_pin_mismatch_keeps_retrying_and_shows_cert_changed() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    let mismatch = DialError::KeyMismatch {
        seen: "ab".repeat(32),
    };
    dialer.plan(FIRST, std::iter::repeat_n(Err(mismatch), 50));
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    assert!(rig.states().contains(&LinkState::Refused {
        reason: RefusalKind::KeyMismatch,
        text: "Key changed".to_owned()
    }));
    settle(120).await;
    assert!(dialer.calls().len() > 3);
}

#[tokio::test(start_paused = true)]
async fn network_change_retries_at_once() {
    let rig = Rig::new();
    let dialer = FakeDialer::default();
    let link = rig.start(dialer.clone(), None);
    tokio::time::sleep(Duration::from_millis(700)).await;
    let before = dialer.calls().len();
    let asked = Instant::now();
    link.send(LinkCmd::NetworkChanged);
    tokio::time::sleep(Duration::from_millis(1)).await;
    let calls = dialer.calls();
    assert!(calls.len() > before);
    assert!(calls[before].1 - asked < Duration::from_millis(2));
}

#[tokio::test(start_paused = true)]
async fn a_new_winning_host_is_saved_first() {
    let mut rig = Rig::new();
    let memory = Arc::new(MemoryVault::default());
    let vault = Vault::new(memory);
    vault.store(&testing::record()).expect("stored");
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Err(DialError::TimedOut)]);
    dialer.plan(SECOND, [Ok(())]);
    let _link = rig.start(dialer.clone(), Some(vault.clone()));
    settle(1).await;
    assert_eq!(rig.last_state(), Some(online()));
    let saved = vault.load(testing::SERVER_ID).expect("saved");
    assert_eq!(saved.hosts, [SECOND, FIRST]);
    assert_eq!(saved.pin, testing::PIN);
    assert!(rig.sessions.borrow().is_some());
    assert!(matches!(rig.inbound.try_recv(), Ok(Inbound::Live(_))));
}

#[tokio::test(start_paused = true)]
async fn a_forgotten_link_never_saves_the_server_again() {
    let rig = Rig::new();
    let vault = Vault::new(Arc::new(MemoryVault::default()));
    vault.store(&testing::record()).expect("stored");
    let dialer = FakeDialer::default();
    dialer.plan(SECOND, [Ok(())]);
    let link = rig.start(dialer, Some(vault.clone()));
    link.send(LinkCmd::AddHosts(vec!["10.0.0.9:8443".to_owned()]));
    link.forget();
    vault.delete(testing::SERVER_ID).expect("deleted");
    settle(5).await;
    assert!(vault.load(testing::SERVER_ID).is_err());
    assert!(vault.servers().expect("listed").records.is_empty());
}

#[tokio::test(start_paused = true)]
async fn close_4003_refuses_without_retry() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(()), Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    dialer.end(DialError::Revoked);
    settle(120).await;
    assert!(matches!(
        rig.last_state(),
        Some(LinkState::Refused {
            reason: RefusalKind::Revoked,
            ..
        })
    ));
    assert_eq!(dialer.calls().len(), 1);
    assert!(rig.sessions.borrow().is_none());
}

#[tokio::test(start_paused = true)]
async fn a_rest_401_refuses_the_live_link() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    let session = lock(&dialer.sessions)[0].clone();
    session.check(&RestError::Status {
        status: 401,
        message: "Phone not paired".to_owned(),
    });
    settle(60).await;
    assert!(matches!(
        rig.last_state(),
        Some(LinkState::Refused {
            reason: RefusalKind::Revoked,
            ..
        })
    ));
    assert_eq!(dialer.stops(), 1);
    assert_eq!(dialer.calls().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_session_comes_back() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(()), Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    dialer.end(DialError::Closed("reset".to_owned()));
    settle(5).await;
    assert_eq!(dialer.calls().len(), 2);
    assert_eq!(rig.last_state(), Some(online()));
    let mut downs = 0;
    while let Ok(inbound) = rig.inbound.try_recv() {
        if matches!(inbound, Inbound::Down) {
            downs += 1;
        }
    }
    assert_eq!(downs, 1);
}

#[tokio::test(start_paused = true)]
async fn background_without_work_pauses_after_30_s() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(()), Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    rig.activity
        .send_modify(|activity| activity.background = true);
    settle(29).await;
    assert_eq!(dialer.stops(), 0);
    settle(2).await;
    assert_eq!(dialer.stops(), 1);
    assert_eq!(rig.last_state(), Some(LinkState::Offline));
    settle(300).await;
    assert_eq!(dialer.calls().len(), 1);
    rig.activity
        .send_modify(|activity| activity.background = false);
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(dialer.calls().len(), 2);
    assert_eq!(rig.last_state(), Some(online()));
}

#[tokio::test(start_paused = true)]
async fn foreground_reconnects_a_paused_link_at_once() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(()), Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    rig.activity
        .send_modify(|activity| activity.background = true);
    settle(40).await;
    assert_eq!(rig.last_state(), Some(LinkState::Offline));
    let asked = Instant::now();
    rig.activity
        .send_modify(|activity| activity.background = false);
    tokio::time::sleep(Duration::from_millis(1)).await;
    let calls = dialer.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].1 - asked < Duration::from_millis(1));
}

#[tokio::test(start_paused = true)]
async fn background_with_an_open_mission_keeps_the_socket() {
    let rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(())]);
    let _link = rig.start(dialer.clone(), None);
    settle(1).await;
    rig.activity.send_modify(|activity| {
        activity.background = true;
        activity.mission_open = true;
    });
    settle(120).await;
    assert_eq!(dialer.stops(), 0);
    rig.activity
        .send_modify(|activity| activity.mission_open = false);
    settle(31).await;
    assert_eq!(dialer.stops(), 1);
}

#[tokio::test(start_paused = true)]
async fn stop_closes_and_goes_offline() {
    let mut rig = Rig::new();
    let dialer = FakeDialer::default();
    dialer.plan(FIRST, [Ok(())]);
    let link = rig.start(dialer.clone(), None);
    settle(1).await;
    link.stop().await;
    assert_eq!(dialer.stops(), 1);
    assert_eq!(rig.last_state(), Some(LinkState::Offline));
    assert!(rig.sessions.borrow().is_none());
}

#[test]
fn the_upgrade_request_carries_the_secret_only_in_the_header() {
    let request = upgrade_request("[fe80::1]:8443", "s3cret").expect("request");
    assert_eq!(request.uri().to_string(), "wss://[fe80::1]:8443/api/ws");
    assert_eq!(request.uri().query(), None);
    let auth = request.headers().get("authorization").expect("header");
    assert_eq!(auth, "Bearer s3cret");
    assert!(auth.is_sensitive());
    assert_eq!(
        request
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("sdrmm")
    );
}

async fn pair() -> (WebSocketStream<DuplexStream>, WebSocketStream<DuplexStream>) {
    let (client, server) = tokio::io::duplex(1 << 20);
    let client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
    let server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    (client, server)
}

async fn connect(
    rig: &Rig,
) -> (
    WebSocketStream<DuplexStream>,
    oneshot::Sender<()>,
    tokio::task::JoinHandle<DialError>,
) {
    let (client, server) = pair().await;
    let (stop, stopped) = oneshot::channel();
    let run = tokio::spawn(Connection::new(client, stopped, rig.wires.clone()).run());
    (server, stop, run)
}

async fn commands(
    server: &mut WebSocketStream<DuplexStream>,
    wait: Duration,
) -> Vec<ClientCommand> {
    let mut out = Vec::new();
    let deadline = Instant::now() + wait;
    while let Ok(Some(Ok(message))) = tokio::time::timeout_at(deadline, server.next()).await {
        if let Message::Text(text) = message
            && let Ok(command) = serde_json::from_str::<ClientCommand>(&text)
        {
            out.push(command);
        }
    }
    out
}

fn pose(seq: u32) -> Option<PoseOut> {
    Some(PoseOut {
        fix: None,
        error: Some(format!("no GPS fix {seq}")),
    })
}

#[tokio::test(start_paused = true)]
async fn offline_poses_are_not_queued_and_the_latest_is_sent_on_live() {
    let rig = Rig::new();
    for seq in 0..3 {
        rig.pose.send_replace(pose(seq));
    }
    let (mut server, _stop, _run) = connect(&rig).await;
    let sent = commands(&mut server, Duration::from_millis(500)).await;
    assert_eq!(
        sent,
        vec![ClientCommand::PublishPose {
            fix: None,
            error: Some("no GPS fix 2".to_owned())
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn pose_publish_rate_is_bounded() {
    let rig = Rig::new();
    let (mut server, _stop, _run) = connect(&rig).await;
    let pose_tx = rig.pose.clone();
    tokio::spawn(async move {
        for seq in 0..1_000 {
            pose_tx.send_replace(pose(seq));
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    let sent = commands(&mut server, Duration::from_millis(1_000)).await;
    let limit = usize::try_from(1_000 / POSE_GAP.as_millis()).unwrap_or(usize::MAX) + 1;
    assert!(sent.len() <= limit, "{}", sent.len());
    assert!(sent.len() + 2 >= limit, "{}", sent.len());
}

#[tokio::test(start_paused = true)]
async fn subscriptions_are_replayed_after_every_reconnect() {
    let rig = Rig::new();
    let fit = SurfaceFit {
        cols: 256,
        rows: 128,
    };
    rig.subs
        .send_replace(Subscriptions::from([("pr1".to_owned(), Some(fit))]));
    let (mut server, stop, first) = connect(&rig).await;
    let subscribe = ClientCommand::SubscribeSurface {
        node: "pr1".to_owned(),
        fit: Some(fit),
    };
    assert_eq!(
        commands(&mut server, Duration::from_millis(100)).await,
        vec![subscribe]
    );
    rig.subs
        .send_replace(Subscriptions::from([("tri1".to_owned(), None)]));
    assert_eq!(
        commands(&mut server, Duration::from_millis(100)).await,
        vec![
            ClientCommand::UnsubscribeSurface {
                node: "pr1".to_owned()
            },
            ClientCommand::SubscribeSurface {
                node: "tri1".to_owned(),
                fit: None
            }
        ]
    );
    let _ = stop.send(());
    assert_eq!(
        first.await.ok(),
        Some(DialError::Closed("stopped".to_owned()))
    );
    let (mut server, _stop, _run) = connect(&rig).await;
    assert_eq!(
        commands(&mut server, Duration::from_millis(100)).await,
        vec![ClientCommand::SubscribeSurface {
            node: "tri1".to_owned(),
            fit: None
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn a_silent_socket_is_dead_after_45_s() {
    let rig = Rig::new();
    let (client, _server) = pair().await;
    let (_stop, stopped) = oneshot::channel();
    let started = Instant::now();
    let ended = Connection::new(client, stopped, rig.wires.clone())
        .run()
        .await;
    assert_eq!(ended, DialError::TimedOut);
    let elapsed = started.elapsed();
    assert!(
        elapsed > DEAD_AFTER && elapsed <= DEAD_AFTER + PING_EVERY,
        "{elapsed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn pings_go_out_every_15_s() {
    let rig = Rig::new();
    let (mut server, _stop, _run) = connect(&rig).await;
    let started = Instant::now();
    let ping = loop {
        match server.next().await {
            Some(Ok(Message::Ping(payload))) => break payload,
            Some(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(ping.len(), 8);
    assert_eq!(started.elapsed(), PING_EVERY);
}

#[tokio::test(start_paused = true)]
async fn close_4003_on_the_socket_is_revoked() {
    let rig = Rig::new();
    let (mut server, _stop, run) = connect(&rig).await;
    server
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(WS_CLOSE_REVOKED),
            reason: "revoked".into(),
        })))
        .await
        .expect("closed");
    assert_eq!(run.await.ok(), Some(DialError::Revoked));
}

#[tokio::test(start_paused = true)]
async fn an_unknown_event_type_raises_one_notice_per_type() {
    let mut rig = Rig::new();
    let (mut server, _stop, _run) = connect(&rig).await;
    for text in [
        r#"{"type":"Mystery","data":{}}"#,
        r#"{"type":"Mystery","data":{"x":1}}"#,
        r#"{"type":"Other"}"#,
        r#"{"type":"StateChanged","data":{"scope":{"scope":"missions"}}}"#,
    ] {
        server.send(Message::text(text)).await.expect("sent");
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        rig.notices(),
        ["Unknown event Mystery", "Unknown event Other"]
    );
    assert!(matches!(
        rig.inbound.try_recv(),
        Ok(Inbound::Event(event)) if matches!(*event, ServerEvent::StateChanged { .. })
    ));
}

fn radar_frame() -> Vec<u8> {
    RangeDopplerOwned {
        stream_id: 3,
        seq: 1,
        timestamp: 0,
        ranges: 2,
        dopplers: 2,
        range_first_m: 0.0,
        range_step_m: 300.0,
        doppler_first_hz: -2.0,
        doppler_step_hz: 2.0,
        carrier_hz: 100e6,
        db_min: 0.0,
        db_max: 30.0,
        cells: vec![0, 1, 2, 3],
    }
    .frame()
    .encode()
}

#[tokio::test(start_paused = true)]
async fn an_unknown_frame_kind_is_noticed_once() {
    let mut rig = Rig::new();
    let (mut server, _stop, _run) = connect(&rig).await;
    let spectrum = SpectrumFrame {
        stream_id: 1,
        seq: 1,
        timestamp: 0,
        center_hz: 1e6,
        span_hz: 1e3,
        db_min: -100.0,
        db_max: 0.0,
        bins: &[1, 2, 3],
    }
    .encode();
    let radar = radar_frame();
    for bytes in [spectrum.clone(), spectrum, radar.clone(), vec![9, 9]] {
        server.send(Message::binary(bytes)).await.expect("sent");
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(rig.notices(), ["Unexpected frame Spectrum", "Bad frame"]);
    assert!(matches!(rig.inbound.try_recv(), Ok(Inbound::Frame(bytes)) if bytes == radar));
}

struct WsStub {
    hello_protocol: u32,
    refuse_upgrade: Option<u16>,
    close_with: Option<u16>,
}

fn about() -> AboutResponse {
    AboutResponse {
        name: "SDR--".to_owned(),
        version: "0.9.0".to_owned(),
        protocol: API_PROTOCOL,
        server_id: testing::SERVER_ID.to_owned(),
        server_name: "Shack".to_owned(),
        license: String::new(),
        license_text: String::new(),
        repository: String::new(),
        components: Vec::new(),
        reveal: false,
    }
}

async fn send_event(socket: &mut StubSocket, event: &ServerEvent) {
    if let Ok(text) = serde_json::to_string(event) {
        let _ = socket.send(Message::text(text)).await;
    }
}

impl StubHandler for WsStub {
    fn http(&self, request: &StubRequest) -> StubResponse {
        match request.path() {
            "/api/about" => StubResponse::json(200, &about()),
            _ => StubResponse::error(404, "no route"),
        }
    }

    fn upgrade(&self, _request: &StubRequest) -> Result<(), StubResponse> {
        match self.refuse_upgrade {
            Some(status) => Err(StubResponse::error(status, "Phone not paired")),
            None => Ok(()),
        }
    }

    fn socket(&self, _request: StubRequest, mut socket: StubSocket) -> BoxFuture<'static, ()> {
        let protocol = self.hello_protocol;
        let close_with = self.close_with;
        Box::pin(async move {
            let hello = ServerEvent::Hello {
                revision: 1,
                protocol,
                phone: Some(testing::PHONE_ID.to_owned()),
            };
            send_event(&mut socket, &hello).await;
            let changed = ServerEvent::StateChanged {
                scope: StateScope::Missions,
            };
            send_event(&mut socket, &changed).await;
            if let Some(code) = close_with {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: CloseCode::from(code),
                        reason: "revoked".into(),
                    })))
                    .await;
            }
            while let Some(Ok(_)) = socket.next().await {}
        })
    }
}

fn stub_record(stub: &StubServer) -> ServerRecord {
    ServerRecord {
        hosts: vec![stub.host()],
        pin: stub.pin.clone(),
        ..testing::record()
    }
}

async fn dial_stub(stub: &StubServer, rig: &Rig) -> Result<Dialed, DialError> {
    TlsDialer::new(rig.wires.clone())
        .dial(Arc::new(stub_record(stub)), stub.host())
        .await
}

fn ws_stub(hello_protocol: u32, refuse_upgrade: Option<u16>, close_with: Option<u16>) -> WsStub {
    WsStub {
        hello_protocol,
        refuse_upgrade,
        close_with,
    }
}

#[tokio::test]
async fn a_stub_session_goes_live_and_forwards_events() {
    let mut rig = Rig::new();
    let server = StubServer::start(ws_stub(API_PROTOCOL, None, None)).await;
    let dialed = dial_stub(&server, &rig).await.expect("dialed");
    assert_eq!(dialed.session.server_name, "Shack");
    assert_eq!(dialed.session.phone_id, testing::PHONE_ID);
    let Dialed { run, stop, .. } = dialed;
    let run = tokio::spawn(run);
    let event = tokio::time::timeout(Duration::from_secs(5), rig.inbound.recv())
        .await
        .expect("an event");
    assert!(matches!(
        event,
        Some(Inbound::Event(event)) if matches!(*event, ServerEvent::StateChanged { .. })
    ));
    let _ = stop.send(());
    let ended = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("stopped");
    assert_eq!(ended.ok(), Some(DialError::Closed("stopped".to_owned())));
}

#[tokio::test]
async fn the_stub_sees_no_query_and_a_bearer_header() {
    let rig = Rig::new();
    let server = StubServer::start(ws_stub(API_PROTOCOL, None, None)).await;
    let dialed = dial_stub(&server, &rig).await.expect("dialed");
    drop(dialed);
    let requests = server.requests();
    let token = testing::token();
    assert_eq!(
        requests.iter().map(StubRequest::path).collect::<Vec<_>>(),
        ["/api/about", "/api/ws"]
    );
    let bearer = format!("Bearer {token}");
    for request in &requests {
        assert!(!request.target.contains('?'), "{}", request.target);
        assert!(!request.target.contains(&token));
        assert_eq!(request.header("authorization"), Some(bearer.as_str()));
    }
    assert_eq!(requests[1].header("sec-websocket-protocol"), Some("sdrmm"));
}

#[tokio::test]
async fn hello_protocol_mismatch_refuses() {
    let rig = Rig::new();
    let server = StubServer::start(ws_stub(API_PROTOCOL + 1, None, None)).await;
    assert_eq!(
        dial_stub(&server, &rig).await.err(),
        Some(DialError::Protocol {
            server: API_PROTOCOL + 1
        })
    );
}

#[tokio::test]
async fn an_upgrade_401_is_revoked_and_a_wrong_pin_sends_nothing() {
    let rig = Rig::new();
    let server = StubServer::start(ws_stub(API_PROTOCOL, Some(401), None)).await;
    assert_eq!(
        dial_stub(&server, &rig).await.err(),
        Some(DialError::Revoked)
    );
    let other = StubServer::start(ws_stub(API_PROTOCOL, None, None)).await;
    let wrong = ServerRecord {
        pin: server.pin.clone(),
        ..stub_record(&other)
    };
    let refused = TlsDialer::new(rig.wires.clone())
        .dial(Arc::new(wrong), other.host())
        .await
        .err();
    assert_eq!(
        refused,
        Some(DialError::KeyMismatch {
            seen: other.pin.clone()
        })
    );
    assert!(other.requests().is_empty());
}

#[tokio::test]
async fn a_revoking_close_from_the_server_parks_the_link() {
    let mut rig = Rig::new();
    let server = StubServer::start(ws_stub(API_PROTOCOL, None, Some(WS_CLOSE_REVOKED))).await;
    let _link = start(
        &Handle::current(),
        stub_record(&server),
        TlsDialer::new(rig.wires.clone()),
        rig.wires.clone(),
        None,
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut refused = false;
    while Instant::now() < deadline && !refused {
        tokio::time::sleep(Duration::from_millis(50)).await;
        refused = rig.states().iter().any(|state| {
            matches!(
                state,
                LinkState::Refused {
                    reason: RefusalKind::Revoked,
                    ..
                }
            )
        });
    }
    assert!(refused);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let upgrades = server
        .requests()
        .iter()
        .filter(|request| request.path() == "/api/ws")
        .count();
    assert_eq!(upgrades, 1);
}
