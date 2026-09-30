use std::{
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    events::{CoreEvent, EventQueue},
    link::{Activity, Inbound, Session, Subscriptions, rest::RestError},
    pose::{PoseSnapshot, now_ms},
    records::Notice,
    runtime::CoreRuntime,
};

mod array;
mod df;
mod fusion;
mod heat;
mod hunt;
pub(crate) mod listing;
mod radar;
pub(crate) mod reducer;
mod survey;
pub(crate) mod views;

use listing::Entry;
use reducer::{Effect, Input, Reducer, Seed, SeedRequest};

const TICK: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Shared {
    pub(crate) entries: Vec<Entry>,
    pub(crate) open: Option<String>,
    requested: u64,
}

impl Shared {
    pub(crate) fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id() == id)
    }
}

type Posted = (Input, Option<oneshot::Sender<()>>);

#[derive(Clone)]
pub(crate) struct MissionHub {
    input: mpsc::UnboundedSender<Posted>,
    shared: Arc<Mutex<Shared>>,
}

impl MissionHub {
    pub(crate) fn send(&self, input: Input) {
        self.post(input, None);
    }

    pub(crate) async fn apply(&self, input: Input) {
        let (done, applied) = oneshot::channel();
        self.post(input, Some(done));
        if applied.await.is_err() {
            tracing::debug!("missions stopped before applying");
        }
    }

    fn post(&self, input: Input, done: Option<oneshot::Sender<()>>) {
        if self.input.send((input, done)).is_err() {
            tracing::debug!("missions already stopped");
        }
    }

    pub(crate) fn shared(&self) -> Shared {
        self.lock().clone()
    }

    pub(crate) fn open(&self, id: Option<String>) {
        let mut shared = self.lock();
        shared.open.clone_from(&id);
        shared.requested += 1;
        self.send(id.map_or(Input::Close, Input::Open));
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub(crate) struct MissionWires {
    pub(crate) events: EventQueue,
    pub(crate) inbound: mpsc::Receiver<Inbound>,
    pub(crate) pose: watch::Receiver<Option<PoseSnapshot>>,
    pub(crate) subs: watch::Sender<Subscriptions>,
    pub(crate) needed: watch::Sender<bool>,
    pub(crate) activity: watch::Sender<Activity>,
}

pub(crate) fn start(runtime: &CoreRuntime, wires: MissionWires) -> MissionHub {
    let (input, inputs) = mpsc::unbounded_channel();
    let hub = MissionHub {
        input,
        shared: Arc::new(Mutex::new(Shared::default())),
    };
    runtime.spawn(run(Reducer::new(), inputs, hub.clone(), wires));
    hub
}

struct Loop {
    reducer: Reducer,
    hub: MissionHub,
    wires: MissionWires,
    session: Option<Arc<Session>>,
    applied: u64,
}

async fn run(
    reducer: Reducer,
    mut inputs: mpsc::UnboundedReceiver<Posted>,
    hub: MissionHub,
    wires: MissionWires,
) {
    let mut state = Loop {
        reducer,
        hub,
        wires,
        session: None,
        applied: 0,
    };
    let mut activity = state.wires.activity.subscribe();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pose_open = true;
    loop {
        let mut done = None;
        let input = tokio::select! {
            inbound = state.wires.inbound.recv() => match inbound {
                Some(inbound) => state.inbound(inbound),
                None => break,
            },
            posted = inputs.recv() => match posted {
                Some((input, waiting)) => {
                    done = waiting;
                    input
                }
                None => break,
            },
            changed = state.wires.pose.changed(), if pose_open => {
                pose_open = changed.is_ok();
                Input::Pose(*state.wires.pose.borrow_and_update())
            }
            changed = activity.changed() => {
                if changed.is_err() {
                    break;
                }
                Input::Background(activity.borrow_and_update().background)
            }
            _ = tick.tick() => Input::Tick,
        };
        state.step(input);
        if done.is_some_and(|done| done.send(()).is_err()) {
            tracing::debug!("nobody waits for the applied input");
        }
    }
}

impl Loop {
    fn inbound(&mut self, inbound: Inbound) -> Input {
        match inbound {
            Inbound::Live(session) => {
                let phone_id = session.phone_id.clone();
                self.session = Some(session);
                Input::Live { phone_id }
            }
            Inbound::Down => {
                self.session = None;
                Input::Down
            }
            Inbound::Event(event) => Input::Event(event),
            Inbound::Frame(bytes) => Input::Frame(bytes),
        }
    }

    fn step(&mut self, input: Input) {
        if matches!(input, Input::Open(_) | Input::Close) {
            self.applied += 1;
        }
        let effects = self.reducer.handle(input, now_ms());
        for effect in effects {
            self.perform(effect);
        }
        self.publish();
    }

    fn perform(&self, effect: Effect) {
        match effect {
            Effect::Emit(event) => self.wires.events.emit(*event),
            Effect::FetchListing => self.fetch_listing(),
            Effect::FetchSelf => self.fetch_self(),
            Effect::Seed { mission, what } => self.fetch_seed(mission, what),
        }
    }

    fn fetch_listing(&self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let hub = self.hub.clone();
        tokio::spawn(async move {
            match session.api.missions().await {
                Ok(response) => hub.send(Input::Listing(Box::new(response))),
                Err(error) => {
                    session.check(&error);
                    hub.send(Input::ListingFailed(error.to_string()));
                }
            }
        });
    }

    fn fetch_self(&self) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let hub = self.hub.clone();
        let events = self.wires.events.clone();
        tokio::spawn(async move {
            match session.api.phone_self().await {
                Ok(phone) => hub.send(Input::PhoneSelf(Box::new(phone))),
                Err(error) => {
                    session.check(&error);
                    tracing::warn!(%error, "phone self fetch failed");
                    events.emit(CoreEvent::Notice {
                        notice: Notice::warn("Phone details unavailable"),
                    });
                }
            }
        });
    }

    fn fetch_seed(&self, mission: String, what: SeedRequest) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let hub = self.hub.clone();
        let events = self.wires.events.clone();
        tokio::spawn(async move {
            let seed = match what {
                SeedRequest::Radar(node) => session
                    .api
                    .radar(node)
                    .await
                    .map(|update| Seed::Radar(Box::new(update))),
                SeedRequest::Survey(node) => session
                    .api
                    .survey(node)
                    .await
                    .map(|grid| Seed::Survey(Box::new(grid))),
                SeedRequest::Fusion(node) => session
                    .api
                    .fusion(node)
                    .await
                    .map(|state| Seed::Fusion(Box::new(state))),
                SeedRequest::Arrays => session
                    .api
                    .state()
                    .await
                    .map(|state| Seed::Arrays(state.arrays)),
            };
            match seed {
                Ok(seed) => hub.send(Input::Seeded { mission, seed }),
                Err(error) => seed_failed(&session, &events, &error),
            }
        });
    }

    fn publish(&self) {
        let subs = self.reducer.subscriptions();
        self.wires.subs.send_if_modified(|current| {
            let changed = *current != subs;
            *current = subs;
            changed
        });
        let needed = self.reducer.pose_needed();
        self.wires.needed.send_if_modified(|current| {
            let changed = *current != needed;
            *current = needed;
            changed
        });
        let open = self.reducer.open_id().is_some();
        self.wires.activity.send_if_modified(|activity| {
            let changed = activity.mission_open != open;
            activity.mission_open = open;
            changed
        });
        let entries = &self.reducer.listing().entries;
        let mut shared = self.hub.lock();
        if shared.entries != *entries {
            shared.entries.clone_from(entries);
        }
        if shared.requested == self.applied {
            shared.open = self.reducer.open_id().map(str::to_owned);
        }
    }
}

fn seed_failed(session: &Session, events: &EventQueue, error: &RestError) {
    session.check(error);
    tracing::warn!(%error, "mission data fetch failed");
    events.emit(CoreEvent::Notice {
        notice: Notice::warn("Mission data unavailable"),
    });
}

#[cfg(test)]
mod tests;
