use std::{
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    API_PROTOCOL, MdnsState, PhoneAccess, PhoneAccessStatus, PhoneEndpoint, PhoneListenerState,
    StateScope, phone::DEFAULT_PHONE_PORT,
};

use super::mdns::{self, Advert, Advertiser};
use crate::{
    AppState, Store, StoreError,
    auth::ListenerRole,
    net,
    tls::{self, Tls},
};

mod endpoint;
mod listener;

pub(crate) use endpoint::{ListenerRecord, candidate, endpoint};
use listener::RunningListener;

const ACCESS_KEY: &str = "phone_access";
const LISTENERS_KEY: &str = "listeners";
const NO_DATA_DIR: &str = "no data directory";
const LISTENER_ENDED: &str = "listener stopped";

pub(crate) struct MainListener {
    pub(crate) record: ListenerRecord,
    pub(crate) own_key: Option<Arc<rustls::ServerConfig>>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccessError {
    #[error("Pick another port")]
    Port,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("saving phone access stopped: {0}")]
    Stopped(String),
}

pub(crate) struct PhoneGate {
    advertise: bool,
    main: OnceLock<MainListener>,
    applying: tokio::sync::Mutex<()>,
    live: Mutex<Live>,
    settled: Arc<Mutex<Settled>>,
    closed: AtomicBool,
}

#[derive(Default)]
struct Live {
    running: Option<RunningListener>,
    advertiser: Option<(Advert, Advertiser)>,
}

struct Settled {
    access: PhoneAccess,
    listener: PhoneListenerState,
    mdns: MdnsState,
    advert: u64,
}

pub(crate) struct GateGuard(Arc<PhoneGate>);

impl GateGuard {
    pub(crate) fn new(gate: Arc<PhoneGate>) -> Self {
        Self(gate)
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.stop();
    }
}

impl Default for PhoneGate {
    fn default() -> Self {
        Self::new(!cfg!(test))
    }
}

impl PhoneGate {
    pub(crate) fn new(advertise: bool) -> Self {
        Self {
            advertise,
            main: OnceLock::new(),
            applying: tokio::sync::Mutex::new(()),
            live: Mutex::new(Live::default()),
            settled: Arc::new(Mutex::new(Settled {
                access: PhoneAccess::default(),
                listener: PhoneListenerState::Off,
                mdns: MdnsState::Off,
                advert: 0,
            })),
            closed: AtomicBool::new(false),
        }
    }

    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn settled(&self) -> MutexGuard<'_, Settled> {
        self.settled.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_main(&self, main: MainListener) {
        if self.main.set(main).is_err() {
            tracing::warn!("the main listener is already recorded");
        }
    }

    pub(crate) fn main_bound(&self) -> Option<IpAddr> {
        self.main.get().map(|main| main.record.bound)
    }

    pub(crate) fn status(&self) -> PhoneAccessStatus {
        let (access, listener, mdns) = {
            let settled = self.settled();
            (
                settled.access,
                settled.listener.clone(),
                settled.mdns.clone(),
            )
        };
        let listener = match listener {
            PhoneListenerState::On { port } if self.ended() => PhoneListenerState::Failed {
                port,
                reason: LISTENER_ENDED.to_owned(),
            },
            listener => listener,
        };
        PhoneAccessStatus {
            access,
            listener,
            endpoint: self.endpoint(),
            mdns,
            protocol: API_PROTOCOL,
        }
    }

    pub(crate) fn endpoint(&self) -> Option<PhoneEndpoint> {
        endpoint(&self.records(), &net::lan_addresses(), &net::mdns_host())
    }

    pub(crate) async fn apply(
        &self,
        state: &AppState,
        access: PhoneAccess,
    ) -> Result<PhoneAccessStatus, AccessError> {
        if !self.port_allowed(access.port) {
            return Err(AccessError::Port);
        }
        let text = serde_json::to_string(&access).map_err(StoreError::from)?;
        let _one = self.applying.lock().await;
        on_store(state, move |store| store.put_meta(ACCESS_KEY, &text)).await?;
        Ok(self.settle(state, access).await)
    }

    pub(crate) async fn restore(&self, state: &AppState) {
        let _one = self.applying.lock().await;
        match on_store(state, stored_access).await {
            Ok(access) => {
                self.settle(state, access).await;
            }
            Err(error) => {
                tracing::warn!(%error, "phone access not restored");
                self.settle(state, PhoneAccess::default()).await;
                self.settled().listener = PhoneListenerState::Failed {
                    port: DEFAULT_PHONE_PORT,
                    reason: error.to_string(),
                };
                state.engine.emit_scope(StateScope::Phones);
            }
        }
    }

    pub(crate) fn stop(&self) {
        let (running, advertiser) = {
            let mut live = self.live();
            self.closed.store(true, Ordering::Release);
            (live.running.take(), live.advertiser.take())
        };
        if let Some(running) = running {
            running.halt();
        }
        if let Some((_, advertiser)) = advertiser {
            advertiser.shutdown();
        }
    }

    fn ended(&self) -> bool {
        self.live()
            .running
            .as_ref()
            .is_some_and(RunningListener::ended)
    }

    fn port_allowed(&self, port: u16) -> bool {
        port != 0 && self.main.get().is_none_or(|main| main.record.port != port)
    }

    async fn settle(&self, state: &AppState, access: PhoneAccess) -> PhoneAccessStatus {
        if self.closed.load(Ordering::Acquire) {
            return self.status();
        }
        let wanted = access.enabled.then_some(access.port);
        let stale = self
            .live()
            .running
            .take_if(|running| Some(running.port) != wanted || running.ended());
        if let Some(running) = stale {
            running.stop().await;
        }
        let listener = match wanted {
            Some(port) => self.run(state, port).await,
            None => PhoneListenerState::Off,
        };
        {
            let mut settled = self.settled();
            settled.access = access;
            settled.listener = listener;
        }
        self.record_listeners(state).await;
        self.readvertise(state).await;
        state.engine.emit_scope(StateScope::Phones);
        self.status()
    }

    async fn run(&self, state: &AppState, port: u16) -> PhoneListenerState {
        if self.live().running.is_some() {
            return PhoneListenerState::On { port };
        }
        if !self.port_allowed(port) {
            return PhoneListenerState::Failed {
                port,
                reason: AccessError::Port.to_string(),
            };
        }
        let started = match self.phone_tls(state).await {
            Ok((tls, pin)) => RunningListener::start(state, port, tls, pin),
            Err(reason) => Err(reason),
        };
        match started {
            Ok(running) => {
                self.keep(running);
                PhoneListenerState::On { port }
            }
            Err(reason) => {
                tracing::warn!(port, %reason, "phone listener failed");
                PhoneListenerState::Failed { port, reason }
            }
        }
    }

    fn keep(&self, running: RunningListener) {
        let mut live = self.live();
        if self.closed.load(Ordering::Acquire) {
            drop(live);
            running.halt();
        } else {
            live.running = Some(running);
        }
    }

    async fn phone_tls(
        &self,
        state: &AppState,
    ) -> Result<(Arc<rustls::ServerConfig>, String), String> {
        if let Some(main) = self.main.get()
            && let (Some(tls), Some(pin)) = (&main.own_key, &main.record.pin)
        {
            return Ok((tls.clone(), pin.clone()));
        }
        let dir = state
            .data_dir
            .clone()
            .ok_or_else(|| NO_DATA_DIR.to_owned())?;
        let loaded = tokio::task::spawn_blocking(move || {
            tls::load(&Tls::SelfSigned {
                dir,
                names: Vec::new(),
            })
        })
        .await;
        match loaded {
            Ok(Ok(served)) => Ok((served.config, served.pin)),
            Ok(Err(error)) => Err(error.to_string()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn records(&self) -> Vec<ListenerRecord> {
        let mut records: Vec<ListenerRecord> = self
            .main
            .get()
            .map(|main| main.record.clone())
            .into_iter()
            .collect();
        if let Some(running) = &self.live().running
            && !running.ended()
        {
            records.push(ListenerRecord {
                role: ListenerRole::Phones,
                port: running.port,
                bound: Ipv4Addr::UNSPECIFIED.into(),
                pin: Some(running.pin.clone()),
                stable_key: true,
                names: Vec::new(),
            });
        }
        records
    }

    async fn record_listeners(&self, state: &AppState) {
        let records = self.records();
        if let Err(error) = on_store(state, move |store| save_listeners(store, &records)).await {
            tracing::warn!(%error, "listeners not saved: sdrmm pair cannot find them");
        }
    }

    async fn readvertise(&self, state: &AppState) {
        let wanted = self
            .advertise
            .then(|| self.endpoint())
            .flatten()
            .map(|endpoint| mdns::advert(&state.server_id, &state.server_name, &endpoint));
        let replaced = {
            let mut live = self.live();
            let current = live.advertiser.as_ref().map(|(advert, _)| advert);
            if wanted.is_some() && current == wanted.as_ref() {
                return;
            }
            live.advertiser.take()
        };
        if let Some((_, old)) = replaced
            && let Err(error) = tokio::task::spawn_blocking(move || old.shutdown()).await
        {
            tracing::warn!(%error, "mDNS advert withdrawal stopped");
        }
        match wanted {
            Some(advert) => self.announce(state, advert),
            None => self.settled().mdns = MdnsState::Off,
        }
    }

    fn announce(&self, state: &AppState, advert: Advert) {
        let generation = {
            let mut settled = self.settled();
            settled.advert += 1;
            settled.mdns = MdnsState::On {
                instance: advert.instance.clone(),
            };
            settled.advert
        };
        let report = reporter(
            Arc::downgrade(&self.settled),
            generation,
            Arc::downgrade(&state.engine),
        );
        match Advertiser::start(&advert, report) {
            Ok(advertiser) => {
                let mut live = self.live();
                if self.closed.load(Ordering::Acquire) {
                    drop(live);
                    tokio::task::spawn_blocking(move || advertiser.shutdown());
                } else {
                    live.advertiser = Some((advert, advertiser));
                }
            }
            Err(reason) => {
                tracing::warn!(%reason, "mDNS advert failed");
                let mut settled = self.settled();
                if settled.advert == generation {
                    settled.mdns = MdnsState::Failed { reason };
                }
            }
        }
    }
}

fn reporter(
    settled: Weak<Mutex<Settled>>,
    generation: u64,
    engine: Weak<Engine>,
) -> impl Fn(MdnsState) + Send + 'static {
    move |mdns| {
        let Some(settled) = settled.upgrade() else {
            return;
        };
        {
            let mut settled = settled.lock().unwrap_or_else(PoisonError::into_inner);
            if settled.advert != generation {
                return;
            }
            settled.mdns = mdns;
        }
        if let Some(engine) = engine.upgrade() {
            engine.emit_scope(StateScope::Phones);
        }
    }
}

async fn on_store<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce(&Store) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, AccessError> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || work(&store))
        .await
        .map_err(|error| AccessError::Stopped(error.to_string()))?
        .map_err(AccessError::from)
}

fn stored<T: serde::de::DeserializeOwned + Default>(
    store: &Store,
    key: &str,
) -> Result<T, StoreError> {
    Ok(store
        .meta(key)?
        .map(|text| serde_json::from_str(&text))
        .transpose()?
        .unwrap_or_default())
}

fn stored_access(store: &Store) -> Result<PhoneAccess, StoreError> {
    stored(store, ACCESS_KEY)
}

pub(crate) fn stored_listeners(store: &Store) -> Result<Vec<ListenerRecord>, StoreError> {
    stored(store, LISTENERS_KEY)
}

pub(crate) fn save_listeners(store: &Store, records: &[ListenerRecord]) -> Result<(), StoreError> {
    store.put_meta(LISTENERS_KEY, &serde_json::to_string(records)?)
}

#[cfg(test)]
mod tests;
