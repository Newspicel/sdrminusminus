use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError, atomic::Ordering},
    time::Duration,
};

use sdrmm_wire::{PositionFix, PositionSource, phone::POSE_SILENT_AFTER_MS};
use tokio::time::Instant;

use super::{GpsHub, PositionState, RouteState, WAITING, limit_error};
use crate::AppState;

pub(super) const PHONE_OFFLINE: &str = "phone offline";
pub(super) const PHONE_SILENT: &str = "phone silent";
pub(super) const PHONE_NOT_PAIRED: &str = "phone not paired";
const TOO_FAST: &str = "pose updates are limited to 20 Hz per phone";
const WATCH_EVERY: Duration = Duration::from_secs(1);
const SILENT_AFTER: Duration = Duration::from_millis(POSE_SILENT_AFTER_MS);

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn validate_update(fix: Option<&PositionFix>, error: Option<&str>) -> Result<(), String> {
    if fix.is_some() == error.is_some() {
        return Err("a pose needs either a fix or an error".to_owned());
    }
    if let Some(fix) = fix {
        fix.validate().map_err(str::to_owned)?;
    }
    if error.is_some_and(|error| error.trim().is_empty()) {
        return Err("a pose error must not be empty".to_owned());
    }
    Ok(())
}

impl GpsHub {
    pub(crate) fn publish_pose(
        &self,
        state: &AppState,
        phone: &str,
        fix: Option<PositionFix>,
        error: Option<String>,
    ) -> Result<usize, String> {
        validate_update(fix.as_ref(), error.as_deref())?;
        if !state.phones.known(phone) {
            return Err(PHONE_NOT_PAIRED.to_owned());
        }
        let next = PositionState {
            fix,
            error: error.map(limit_error),
        };
        let bound = self.bound_to(phone);
        let now = Instant::now();
        if !self.repeats(&bound, &next) {
            let mut published = locked(&self.pose_at);
            if published
                .get(phone)
                .is_some_and(|last| now.saturating_duration_since(*last) < self.pose_interval())
            {
                return Err(TOO_FAST.to_owned());
            }
            published.insert(phone.to_owned(), now);
        }
        locked(&self.pose_seen).insert(phone.to_owned(), now);
        for node in &bound {
            self.publish_state(state, node, next.fix.clone(), next.error.clone());
        }
        Ok(bound.len())
    }

    fn repeats(&self, bound: &[String], next: &PositionState) -> bool {
        let latest = locked(&self.latest);
        !bound.is_empty() && bound.iter().all(|node| latest.get(node) == Some(next))
    }

    fn pose_interval(&self) -> Duration {
        Duration::from_millis(self.pose_interval_ms.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn set_pose_interval(&self, interval: Duration) {
        self.pose_interval_ms.store(
            u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub(crate) fn phone_online(&self, state: &AppState, phone: &str) {
        let mut online = locked(&self.online);
        if !state.phones.online(phone) {
            return;
        }
        online.insert(phone.to_owned());
        for node in self.bound_to(phone) {
            let stale = locked(&self.latest).get(&node).is_some_and(|current| {
                current.fix.is_none()
                    && matches!(
                        current.error.as_deref(),
                        Some(PHONE_OFFLINE | PHONE_NOT_PAIRED)
                    )
            });
            if stale {
                self.publish_state(state, &node, None, Some(WAITING.to_owned()));
            }
        }
    }

    pub(crate) fn phone_offline(&self, state: &AppState, phone: &str) {
        let mut online = locked(&self.online);
        if state.phones.online(phone) {
            return;
        }
        online.remove(phone);
        locked(&self.pose_seen).remove(phone);
        locked(&self.pose_at).remove(phone);
        let standing = if state.phones.known(phone) {
            PHONE_OFFLINE
        } else {
            PHONE_NOT_PAIRED
        };
        for node in self.bound_to(phone) {
            self.publish_state(state, &node, None, Some(standing.to_owned()));
        }
    }

    pub(crate) fn bound_to(&self, phone: &str) -> Vec<String> {
        let mut nodes: Vec<String> = locked(&self.configuration)
            .sources
            .iter()
            .filter(|(_, source)| {
                matches!(source, PositionSource::Phone { phone: bound } if bound == phone)
            })
            .map(|(node, _)| node.clone())
            .collect();
        nodes.sort_unstable();
        nodes
    }

    fn is_online(&self, phone: &str) -> bool {
        locked(&self.online).contains(phone)
    }

    pub(super) fn publish_phone_standing(&self, state: &AppState, node: &str, phone: &str) {
        let standing = if !state.phones.known(phone) {
            PHONE_NOT_PAIRED
        } else if !self.is_online(phone) {
            PHONE_OFFLINE
        } else {
            WAITING
        };
        let holds_news = locked(&self.latest).get(node).is_some_and(|current| {
            current.fix.is_some()
                || current
                    .error
                    .as_deref()
                    .is_some_and(|error| error != PHONE_OFFLINE && error != PHONE_NOT_PAIRED)
        });
        if standing != WAITING || !holds_news {
            self.publish_state(state, node, None, Some(standing.to_owned()));
        }
    }

    pub(crate) fn spawn_watchdog(self: &Arc<Self>, state: &AppState) {
        let hub = Arc::downgrade(self);
        let route = RouteState::from(state);
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!("no runtime in context: silent phones will not be reported");
            return;
        };
        let _guard = handle.enter();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(WATCH_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let Some(hub) = hub.upgrade() else {
                    break;
                };
                hub.report_silent(&route, Instant::now());
            }
        });
    }

    fn report_silent(&self, route: &RouteState, now: Instant) {
        let silent: Vec<String> = {
            let seen: HashMap<String, Instant> = locked(&self.pose_seen).clone();
            locked(&self.online)
                .iter()
                .filter(|phone| {
                    seen.get(*phone)
                        .is_some_and(|at| now.saturating_duration_since(*at) >= SILENT_AFTER)
                })
                .cloned()
                .collect()
        };
        for phone in silent {
            for node in self.bound_to(&phone) {
                if self.fix(&node).is_some() {
                    self.publish_routed(route.clone(), &node, None, Some(PHONE_SILENT.to_owned()));
                }
            }
        }
    }
}
