use std::{
    collections::BTreeMap,
    pin::pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::future::BoxFuture;
use sdrmm_wire::{about::API_PROTOCOL, ws::ServerEvent, ws::SurfaceFit};
use tokio::{
    runtime::Handle,
    sync::{Notify, mpsc, oneshot, watch},
    task::JoinHandle,
    time::Instant,
};

use crate::{
    events::{CoreEvent, EventQueue},
    pose::PoseOut,
    records::{LinkState, Notice, Platform, RefusalKind},
    vault::{MAX_SAVED_HOSTS, ServerRecord, Vault},
};

mod backoff;
mod candidates;
pub(crate) mod phase;
pub(crate) mod rest;
pub(crate) mod socket;

pub(crate) use candidates::{STAGGER, is_local, merged, race, worst};
pub(crate) use phase::Activity;
use phase::{KeepAlive, keep_alive};
use rest::{Api, RestError};

pub(crate) const INBOUND_CAPACITY: usize = 1024;
const RESET_AFTER: Duration = Duration::from_secs(60);
const STOP_GRACE: Duration = Duration::from_secs(2);
const CLOSE_GRACE: Duration = Duration::from_secs(1);

pub(crate) type Subscriptions = BTreeMap<String, Option<SurfaceFit>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DialError {
    Revoked,
    Protocol { server: u32 },
    KeyMismatch { seen: String },
    Server { status: u16, message: String },
    Blocked,
    TimedOut,
    Unreachable(String),
    Closed(String),
}

impl DialError {
    pub(crate) fn rank(&self) -> u8 {
        match self {
            Self::Revoked => 7,
            Self::Protocol { .. } => 6,
            Self::KeyMismatch { .. } => 5,
            Self::Server { .. } => 4,
            Self::Blocked => 3,
            Self::TimedOut => 2,
            Self::Unreachable(_) => 1,
            Self::Closed(_) => 0,
        }
    }

    pub(crate) fn from_rest(error: RestError) -> Self {
        match error {
            RestError::Status { status: 401, .. } => Self::Revoked,
            RestError::Status { status, message } => Self::Server { status, message },
            RestError::KeyMismatch { seen } => Self::KeyMismatch { seen },
            RestError::TimedOut => Self::TimedOut,
            RestError::Unreachable(detail) => Self::Unreachable(detail),
            RestError::Decode(message) => Self::Server {
                status: 200,
                message,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Net {
    android: bool,
    local_allowed: Arc<AtomicBool>,
}

impl Net {
    pub(crate) fn new(platform: Platform) -> Self {
        Self {
            android: platform == Platform::Android,
            local_allowed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn allow_local(&self, allowed: bool) {
        self.local_allowed.store(allowed, Ordering::Relaxed);
    }

    pub(crate) fn explain(&self, host: &str, error: DialError) -> DialError {
        let unanswered = matches!(error, DialError::TimedOut | DialError::Unreachable(_));
        let blocked = self.android && !self.local_allowed.load(Ordering::Relaxed) && is_local(host);
        if unanswered && blocked {
            DialError::Blocked
        } else {
            error
        }
    }
}

pub(crate) struct Session {
    pub(crate) server_name: String,
    pub(crate) phone_id: String,
    pub(crate) host: String,
    pub(crate) api: Arc<dyn Api>,
    revoked: Notify,
}

impl Session {
    pub(crate) fn new(
        server_name: String,
        phone_id: String,
        host: String,
        api: Arc<dyn Api>,
    ) -> Self {
        Self {
            server_name,
            phone_id,
            host,
            api,
            revoked: Notify::new(),
        }
    }

    pub(crate) fn check(&self, error: &RestError) {
        if error.status() == Some(401) {
            self.revoked.notify_one();
        }
    }
}

pub(crate) enum Inbound {
    Live(Arc<Session>),
    Down,
    Event(Box<ServerEvent>),
    Frame(Vec<u8>),
}

pub(crate) struct Dialed {
    pub(crate) session: Arc<Session>,
    pub(crate) run: BoxFuture<'static, DialError>,
    pub(crate) stop: oneshot::Sender<()>,
}

pub(crate) trait Dialer: Send + Sync + 'static {
    fn dial(
        &self,
        record: Arc<ServerRecord>,
        host: String,
    ) -> BoxFuture<'static, Result<Dialed, DialError>>;
}

#[derive(Clone)]
pub(crate) struct Wires {
    pub(crate) events: EventQueue,
    pub(crate) inbound: mpsc::Sender<Inbound>,
    pub(crate) sessions: watch::Sender<Option<Arc<Session>>>,
    pub(crate) subs: watch::Receiver<Subscriptions>,
    pub(crate) pose: watch::Receiver<Option<PoseOut>>,
    pub(crate) activity: watch::Receiver<Activity>,
    pub(crate) net: Net,
}

impl Wires {
    pub(crate) fn forward(&self, inbound: Inbound) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.inbound.try_send(inbound) {
            self.events.missed(1);
        }
    }

    async fn deliver(&self, inbound: Inbound) {
        if self.inbound.send(inbound).await.is_err() {
            tracing::debug!("missions already stopped");
        }
    }
}

pub(crate) enum LinkCmd {
    AddHosts(Vec<String>),
    NetworkChanged,
    Stop,
}

#[derive(Clone, Debug, Default)]
struct Retired(Arc<Mutex<bool>>);

impl Retired {
    fn retire(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = true;
    }

    fn unless_retired(&self, write: impl FnOnce()) {
        let retired = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !*retired {
            write();
        }
    }
}

pub(crate) struct LinkHandle {
    server_id: String,
    cmd: mpsc::UnboundedSender<LinkCmd>,
    task: JoinHandle<()>,
    retired: Retired,
}

impl LinkHandle {
    pub(crate) fn server_id(&self) -> &str {
        &self.server_id
    }

    pub(crate) fn forget(self) {
        self.retired.retire();
        self.send(LinkCmd::Stop);
    }

    pub(crate) fn send(&self, cmd: LinkCmd) {
        if self.cmd.send(cmd).is_err() {
            tracing::debug!(server = %self.server_id, "link already ended");
        }
    }

    pub(crate) async fn stop(self) {
        self.send(LinkCmd::Stop);
        let abort = self.task.abort_handle();
        if tokio::time::timeout(STOP_GRACE, self.task).await.is_err() {
            abort.abort();
        }
    }
}

pub(crate) fn start<D: Dialer>(
    runtime: &Handle,
    record: ServerRecord,
    dialer: D,
    wires: Wires,
    vault: Option<Vault>,
) -> LinkHandle {
    let (cmd, cmds) = mpsc::unbounded_channel();
    let server_id = record.server_id.clone();
    let retired = Retired::default();
    let supervisor = Supervisor {
        record,
        dialer: Arc::new(dialer),
        wires,
        vault,
        retired: retired.clone(),
        cmds,
        backoff: backoff::Backoff::new(backoff::seed()),
        last_good: None,
        background_since: None,
        blocked_told: false,
    };
    LinkHandle {
        server_id,
        cmd,
        task: runtime.spawn(supervisor.run()),
        retired,
    }
}

enum Next {
    Attempt,
    Wait,
    Pause,
    Park,
    Stop,
}

enum Ended {
    Dropped(DialError),
    Revoked,
    Pause,
    Stop,
}

struct Supervisor<D> {
    record: ServerRecord,
    dialer: Arc<D>,
    wires: Wires,
    vault: Option<Vault>,
    retired: Retired,
    cmds: mpsc::UnboundedReceiver<LinkCmd>,
    backoff: backoff::Backoff,
    last_good: Option<String>,
    background_since: Option<Instant>,
    blocked_told: bool,
}

impl<D: Dialer> Supervisor<D> {
    async fn run(mut self) {
        self.note_activity();
        let mut next = Next::Attempt;
        loop {
            next = match next {
                Next::Attempt => self.attempt().await,
                Next::Wait => self.wait().await,
                Next::Pause => self.pause().await,
                Next::Park => self.park().await,
                Next::Stop => break,
            };
        }
        self.wires.sessions.send_replace(None);
        self.emit(LinkState::Offline);
    }

    fn emit(&self, state: LinkState) {
        self.wires.events.emit(CoreEvent::Link { state });
    }

    fn refuse(&self, reason: RefusalKind, text: &str) {
        self.emit(LinkState::Refused {
            reason,
            text: text.to_owned(),
        });
    }

    fn note_activity(&mut self) {
        let activity = *self.wires.activity.borrow_and_update();
        self.background_since = match (activity.background, self.background_since) {
            (false, _) => None,
            (true, Some(since)) => Some(since),
            (true, None) => Some(Instant::now()),
        };
    }

    fn keep_alive(&self) -> KeepAlive {
        keep_alive(*self.wires.activity.borrow())
    }

    fn drop_at(&self) -> Option<Instant> {
        match self.keep_alive() {
            KeepAlive::DropAfter(grace) => self.background_since.map(|since| since + grace),
            KeepAlive::Full | KeepAlive::Lean => None,
        }
    }

    fn add_hosts(&mut self, hosts: &[String]) {
        let hosts = merged(&self.record.hosts, hosts, MAX_SAVED_HOSTS);
        if hosts != self.record.hosts {
            self.record.hosts = hosts;
            self.save();
        }
    }

    fn save(&self) {
        let Some(vault) = &self.vault else {
            return;
        };
        self.retired.unless_retired(|| {
            if let Err(error) = vault.store(&self.record) {
                tracing::warn!(%error, "saved server not updated");
                self.wires.events.emit(CoreEvent::Notice {
                    notice: Notice::warn("Saved server not updated"),
                });
            }
        });
    }

    async fn attempt(&mut self) -> Next {
        let hosts = candidates::ordered(&self.record.hosts, self.last_good.as_deref());
        self.emit(LinkState::Connecting {
            attempt: self.backoff.attempt().saturating_add(1),
            host: hosts.first().cloned().unwrap_or_default(),
        });
        let record = Arc::new(self.record.clone());
        let dialer = self.dialer.clone();
        let racing = race(&hosts, candidates::STAGGER, move |host| {
            dialer.dial(record.clone(), host)
        });
        let outcome = {
            let mut racing = pin!(racing);
            loop {
                tokio::select! {
                    result = &mut racing => break Some(result),
                    cmd = self.cmds.recv() => match cmd {
                        None | Some(LinkCmd::Stop) => break None,
                        Some(LinkCmd::AddHosts(fresh)) => self.add_hosts(&fresh),
                        Some(LinkCmd::NetworkChanged) => {}
                    },
                }
            }
        };
        match outcome {
            None => Next::Stop,
            Some(Ok((host, dialed))) => self.live(host, dialed).await,
            Some(Err(failures)) => self.failed(worst(failures)),
        }
    }

    async fn live(&mut self, host: String, dialed: Dialed) -> Next {
        if self.record.hosts.first() != Some(&host) {
            self.record.hosts = candidates::promoted(&self.record.hosts, &host);
            self.save();
        }
        self.last_good = Some(host);
        let Dialed {
            session,
            mut run,
            stop,
        } = dialed;
        self.wires.sessions.send_replace(Some(session.clone()));
        self.wires.deliver(Inbound::Live(session.clone())).await;
        self.emit(LinkState::Online {
            server: session.server_name.clone(),
        });
        let ended = self.hold(&session, &mut run).await;
        self.wires.sessions.send_replace(None);
        self.wires.deliver(Inbound::Down).await;
        if !matches!(ended, Ended::Dropped(_)) {
            let _ = stop.send(());
            let _ = tokio::time::timeout(CLOSE_GRACE, run).await;
        }
        match ended {
            Ended::Stop => Next::Stop,
            Ended::Pause => Next::Pause,
            Ended::Revoked => self.failed(DialError::Revoked),
            Ended::Dropped(error) => self.failed(error),
        }
    }

    async fn hold(&mut self, session: &Session, run: &mut BoxFuture<'static, DialError>) -> Ended {
        let mut reset = pin!(tokio::time::sleep(RESET_AFTER));
        let mut reset_done = false;
        loop {
            let drop_at = self.drop_at();
            let dropping = tokio::time::sleep_until(drop_at.unwrap_or_else(Instant::now));
            tokio::select! {
                error = &mut *run => return Ended::Dropped(error),
                () = session.revoked.notified() => return Ended::Revoked,
                cmd = self.cmds.recv() => match cmd {
                    None | Some(LinkCmd::Stop) => return Ended::Stop,
                    Some(LinkCmd::AddHosts(fresh)) => self.add_hosts(&fresh),
                    Some(LinkCmd::NetworkChanged) => {}
                },
                () = &mut reset, if !reset_done => {
                    reset_done = true;
                    self.backoff.reset();
                }
                changed = self.wires.activity.changed() => {
                    if changed.is_err() {
                        return Ended::Stop;
                    }
                    self.note_activity();
                }
                () = dropping, if drop_at.is_some() => return Ended::Pause,
            }
        }
    }

    fn failed(&mut self, error: DialError) -> Next {
        tracing::info!(?error, "link down");
        match error {
            DialError::Revoked => {
                self.refuse(RefusalKind::Revoked, "Removed. Pair again");
                Next::Park
            }
            DialError::Protocol { server } if server < API_PROTOCOL => {
                self.refuse(RefusalKind::ServerTooOld, "Server update needed");
                Next::Park
            }
            DialError::Protocol { .. } => {
                self.refuse(RefusalKind::AppTooOld, "App update needed");
                Next::Park
            }
            DialError::KeyMismatch { .. } => {
                self.refuse(RefusalKind::KeyMismatch, "Key changed");
                Next::Wait
            }
            DialError::Blocked => {
                if !self.blocked_told {
                    self.blocked_told = true;
                    self.wires.events.emit(CoreEvent::Notice {
                        notice: Notice::error("Local network blocked"),
                    });
                }
                Next::Wait
            }
            DialError::Server { .. }
            | DialError::TimedOut
            | DialError::Unreachable(_)
            | DialError::Closed(_) => Next::Wait,
        }
    }

    async fn wait(&mut self) -> Next {
        if self.drop_at().is_some() {
            return Next::Pause;
        }
        let delay = self.backoff.next_delay();
        let mut sleeping = pin!(tokio::time::sleep(delay));
        loop {
            tokio::select! {
                () = &mut sleeping => return Next::Attempt,
                cmd = self.cmds.recv() => match cmd {
                    None | Some(LinkCmd::Stop) => return Next::Stop,
                    Some(LinkCmd::AddHosts(fresh)) => {
                        self.add_hosts(&fresh);
                        return Next::Attempt;
                    }
                    Some(LinkCmd::NetworkChanged) => return Next::Attempt,
                },
                changed = self.wires.activity.changed() => {
                    if changed.is_err() {
                        return Next::Stop;
                    }
                    let was_background = self.background_since.is_some();
                    self.note_activity();
                    if was_background && self.background_since.is_none() {
                        return Next::Attempt;
                    }
                    if self.drop_at().is_some() {
                        return Next::Pause;
                    }
                }
            }
        }
    }

    async fn pause(&mut self) -> Next {
        self.emit(LinkState::Offline);
        loop {
            tokio::select! {
                cmd = self.cmds.recv() => match cmd {
                    None | Some(LinkCmd::Stop) => return Next::Stop,
                    Some(LinkCmd::AddHosts(fresh)) => self.add_hosts(&fresh),
                    Some(LinkCmd::NetworkChanged) => {}
                },
                changed = self.wires.activity.changed() => {
                    if changed.is_err() {
                        return Next::Stop;
                    }
                    self.note_activity();
                    if self.drop_at().is_none() {
                        return Next::Attempt;
                    }
                }
            }
        }
    }

    async fn park(&mut self) -> Next {
        loop {
            match self.cmds.recv().await {
                None | Some(LinkCmd::Stop) => return Next::Stop,
                Some(LinkCmd::AddHosts(fresh)) => self.add_hosts(&fresh),
                Some(LinkCmd::NetworkChanged) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
