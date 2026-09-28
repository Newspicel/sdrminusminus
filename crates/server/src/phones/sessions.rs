use std::{
    collections::HashMap,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::watch;

pub(crate) struct SessionGuard {
    pub(crate) phone: String,
    id: u64,
    pub(crate) revoked: watch::Receiver<bool>,
}

#[derive(Default)]
pub(super) struct Sessions {
    live: Mutex<HashMap<String, HashMap<u64, watch::Sender<bool>>>>,
    next: AtomicU64,
}

impl Sessions {
    fn live(&self) -> MutexGuard<'_, HashMap<String, HashMap<u64, watch::Sender<bool>>>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn join(&self, phone: &str) -> (SessionGuard, bool) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (sender, revoked) = watch::channel(false);
        let mut live = self.live();
        let sockets = live.entry(phone.to_owned()).or_default();
        let first = sockets.is_empty();
        sockets.insert(id, sender);
        (
            SessionGuard {
                phone: phone.to_owned(),
                id,
                revoked,
            },
            first,
        )
    }

    pub(super) fn leave(&self, guard: SessionGuard) -> bool {
        let mut live = self.live();
        let Some(sockets) = live.get_mut(&guard.phone) else {
            return false;
        };
        if sockets.remove(&guard.id).is_none() {
            return false;
        }
        let last = sockets.is_empty();
        if last {
            live.remove(&guard.phone);
        }
        last
    }

    pub(super) fn online(&self, phone: &str) -> bool {
        self.live()
            .get(phone)
            .is_some_and(|sockets| !sockets.is_empty())
    }

    pub(super) fn revoke(&self, phone: &str) {
        if let Some(sockets) = self.live().get(phone) {
            for sender in sockets.values() {
                sender.send_replace(true);
            }
        }
    }
}
