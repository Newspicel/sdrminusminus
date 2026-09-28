use std::time::Instant;

use sdrmm_wire::StateScope;
use tokio::sync::watch;

use crate::{AppState, auth::Identity, phones::SessionGuard};

pub(super) struct RateBudget {
    tokens: f64,
    capacity: f64,
    per_second: f64,
    refilled: Instant,
}

impl RateBudget {
    pub(super) fn new(capacity: f64, per_second: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            per_second,
            refilled: Instant::now(),
        }
    }

    pub(super) fn take(&mut self) -> bool {
        let now = Instant::now();
        let earned = now.duration_since(self.refilled).as_secs_f64() * self.per_second;
        self.tokens = (self.tokens + earned).min(self.capacity);
        self.refilled = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

pub(super) struct PhoneLink {
    guard: SessionGuard,
}

impl PhoneLink {
    pub(super) async fn join(state: &AppState, identity: &Identity) -> Option<Self> {
        let phone = identity.phone()?.to_owned();
        let (guard, first) = state.phones.join(&phone);
        if first {
            let app = state.clone();
            let marked =
                tokio::task::spawn_blocking(move || app.gps.phone_online(&app, &phone)).await;
            if let Err(error) = marked {
                tracing::warn!(%error, "marking a phone online stopped");
            }
            state.engine.emit_scope(StateScope::Phones);
        }
        Some(Self { guard })
    }

    pub(super) fn revoked(&self) -> watch::Receiver<bool> {
        self.guard.revoked.clone()
    }

    pub(super) async fn leave(self, state: &AppState) {
        let phone = self.guard.phone.clone();
        state.phones.touch(&phone);
        if state.phones.leave(self.guard) {
            let app = state.clone();
            let marked =
                tokio::task::spawn_blocking(move || app.gps.phone_offline(&app, &phone)).await;
            if let Err(error) = marked {
                tracing::warn!(%error, "marking a phone offline stopped");
            }
        }
        crate::phones::flush(state.phones.clone()).await;
        state.engine.emit_scope(StateScope::Phones);
    }
}

pub(super) async fn revoked(signal: &mut Option<watch::Receiver<bool>>) {
    if let Some(signal) = signal {
        let fired = signal.wait_for(|revoked| *revoked).await.is_ok();
        if fired {
            return;
        }
    }
    std::future::pending::<()>().await;
}
