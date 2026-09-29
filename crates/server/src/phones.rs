use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock},
    time::Duration,
};

use jiff::Timestamp;
use sdrmm_wire::{NodeBody, PatchGraph, Phone, PhoneToken, PositionSource, StateScope};

use crate::{
    AppState, Store, StoreError,
    auth::bytes_eq,
    rest::AppError,
    store::{PhoneRow, rfc3339},
};

pub mod cli;
pub(crate) mod gate;
mod mdns;
mod pairing;
pub(crate) mod scope;
mod sessions;
mod token;

pub(crate) use pairing::PairError;
pub(crate) use scope::{phone_command, phone_event, phone_may};
pub(crate) use sessions::SessionGuard;

const FLUSH_EVERY: Duration = Duration::from_secs(60);

pub(crate) struct Phones {
    store: Arc<Store>,
    known: RwLock<HashMap<String, [u8; 32]>>,
    sessions: sessions::Sessions,
    seen: Mutex<HashMap<String, Timestamp>>,
    stored: Mutex<()>,
}

pub(crate) enum Verified {
    Paired,
    Unknown,
    Refused,
}

impl Phones {
    pub(crate) fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            known: RwLock::new(HashMap::new()),
            sessions: sessions::Sessions::default(),
            seen: Mutex::new(HashMap::new()),
            stored: Mutex::new(()),
        }
    }

    fn in_step_with_the_store(&self) -> MutexGuard<'_, ()> {
        self.stored.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn seen(&self) -> MutexGuard<'_, HashMap<String, Timestamp>> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cached(&self, id: &str) -> Option<[u8; 32]> {
        self.known
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .copied()
    }

    fn remember(&self, id: &str, secret_sha256: [u8; 32]) {
        self.known
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), secret_sha256);
    }

    fn forget(&self, id: &str) {
        self.known
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }

    fn secret_of(&self, id: &str) -> Option<[u8; 32]> {
        if let Some(secret) = self.cached(id) {
            return Some(secret);
        }
        let _step = self.in_step_with_the_store();
        match self.store.phone(id) {
            Ok(row) => {
                self.remember(id, row.secret_sha256);
                Some(row.secret_sha256)
            }
            Err(StoreError::PhoneNotFound(_)) => None,
            Err(error) => {
                tracing::warn!(%error, phone = id, "could not read a paired phone");
                None
            }
        }
    }

    pub(crate) fn verify_cached(&self, token: &PhoneToken) -> Verified {
        match self.cached(&token.phone) {
            Some(stored) if matches(token, &stored) => Verified::Paired,
            Some(_) => Verified::Refused,
            None => Verified::Unknown,
        }
    }

    pub(crate) fn verify(&self, token: &PhoneToken) -> bool {
        self.secret_of(&token.phone)
            .is_some_and(|stored| matches(token, &stored))
    }

    pub(crate) fn known(&self, id: &str) -> bool {
        self.secret_of(id).is_some()
    }

    pub(crate) fn touch(&self, phone: &str) {
        self.seen().insert(phone.to_owned(), Timestamp::now());
    }

    pub(crate) fn flush_seen(&self) -> Result<(), StoreError> {
        let pending: Vec<(String, Timestamp)> = self.seen().drain().collect();
        if pending.is_empty() {
            return Ok(());
        }
        let rows: Vec<(String, String)> = pending
            .iter()
            .map(|(id, at)| (id.clone(), rfc3339(*at)))
            .collect();
        let written = self.store.touch_phones(&rows);
        if written.is_err() {
            let mut seen = self.seen();
            for (id, at) in pending {
                let kept = seen.entry(id).or_insert(at);
                *kept = (*kept).max(at);
            }
        }
        written
    }

    pub(crate) fn list(&self, graph: Option<&PatchGraph>) -> Result<Vec<Phone>, StoreError> {
        let rows = self.store.phones()?;
        Ok(rows
            .into_iter()
            .map(|row| self.describe(row, graph))
            .collect())
    }

    pub(crate) fn one(&self, id: &str, graph: Option<&PatchGraph>) -> Result<Phone, StoreError> {
        Ok(self.describe(self.store.phone(id)?, graph))
    }

    fn describe(&self, row: PhoneRow, graph: Option<&PatchGraph>) -> Phone {
        let fresh = self.seen().get(&row.id).map(|at| rfc3339(*at));
        let last_seen = match (row.last_seen, fresh) {
            (Some(stored), Some(fresh)) => Some(stored.max(fresh)),
            (stored, fresh) => stored.or(fresh),
        };
        Phone {
            online: self.online(&row.id),
            gps_nodes: gps_nodes(graph, &row.id),
            id: row.id,
            name: row.name,
            platform: row.platform,
            created_at: row.created_at,
            last_seen,
        }
    }

    pub(crate) fn rename(
        &self,
        id: &str,
        name: &str,
        graph: Option<&PatchGraph>,
    ) -> Result<Phone, PairError> {
        let name = name.trim();
        if !sdrmm_wire::phone::valid_phone_name(name) {
            return Err(PairError::Name);
        }
        self.store.rename_phone(id, name)?;
        Ok(self.one(id, graph)?)
    }

    pub(crate) fn revoke(&self, state: &AppState, id: &str) -> Result<(), AppError> {
        self.remove(id)?;
        state.engine.emit_scope(StateScope::Phones);
        Ok(())
    }

    pub(crate) fn remove(&self, id: &str) -> Result<(), StoreError> {
        {
            let _step = self.in_step_with_the_store();
            self.store.delete_phone(id)?;
            self.forget(id);
        }
        self.seen().remove(id);
        self.sessions.revoke(id);
        Ok(())
    }

    pub(crate) fn join(&self, phone: &str) -> (SessionGuard, bool) {
        let joined = self.sessions.join(phone);
        if self.cached(phone).is_none() {
            self.sessions.revoke(phone);
        }
        joined
    }

    pub(crate) fn leave(&self, guard: SessionGuard) -> bool {
        self.sessions.leave(guard)
    }

    pub(crate) fn online(&self, phone: &str) -> bool {
        self.sessions.online(phone)
    }
}

fn matches(token: &PhoneToken, stored: &[u8; 32]) -> bool {
    bytes_eq(&token::hash(token.secret()), stored)
}

fn gps_nodes(graph: Option<&PatchGraph>, phone: &str) -> Vec<String> {
    graph
        .map(|graph| {
            graph
                .nodes
                .iter()
                .filter(|node| {
                    matches!(
                        &node.body,
                        NodeBody::Gps(gps) if matches!(
                            &gps.source,
                            Some(PositionSource::Phone { phone: bound }) if bound == phone
                        )
                    )
                })
                .map(|node| node.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn spawn_flusher(state: &AppState) {
    let phones = Arc::downgrade(&state.phones);
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("no runtime in context: when phones were last seen is not saved");
        return;
    };
    let _guard = handle.enter();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(FLUSH_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tick.tick().await;
            let Some(phones) = phones.upgrade() else {
                break;
            };
            flush(phones).await;
        }
    });
}

pub(crate) async fn flush(phones: Arc<Phones>) {
    match tokio::task::spawn_blocking(move || phones.flush_seen()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!(%error, "could not save when phones were last seen"),
        Err(error) => tracing::warn!(%error, "saving when phones were last seen stopped"),
    }
}

#[cfg(test)]
pub(crate) mod tests;
