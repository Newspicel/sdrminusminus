use std::time::Duration;

use sdrmm_wire::{ServerEvent, StateScope, SurveyUpdate};
use tokio::sync::broadcast::{
    self,
    error::{RecvError, TryRecvError},
};

use crate::AppState;

pub(super) const DEBOUNCE: Duration = Duration::from_millis(250);

pub(crate) fn spawn(state: &AppState) {
    let events = state.engine.subscribe_events();
    let last = match super::missions(state) {
        Ok(listing) => Some(listing.revision),
        Err(error) => {
            tracing::warn!(%error, "missions could not be listed at startup");
            None
        }
    };
    let watched = state.clone();
    let _detached = crate::spawn_task("sdrmm-missions", move || run(watched, events, last));
}

async fn run(state: AppState, mut events: broadcast::Receiver<ServerEvent>, mut last: Option<u64>) {
    loop {
        match events.recv().await {
            Ok(event) if !arms(&event) => continue,
            Ok(_) | Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return,
        }
        tokio::time::sleep(DEBOUNCE).await;
        if !settle(&mut events) {
            return;
        }
        last = check(&state, last).await;
    }
}

fn arms(event: &ServerEvent) -> bool {
    match event {
        ServerEvent::StateChanged { scope } => {
            !matches!(scope, StateScope::Clients | StateScope::Missions)
        }
        ServerEvent::ArrayUpdate { .. } => true,
        ServerEvent::SurveyUpdate { update, .. } => survey_moved(update),
        _ => false,
    }
}

fn survey_moved(update: &SurveyUpdate) -> bool {
    update.cell.is_some() || update.stopped.is_some() || update.target_hz.is_none()
}

fn settle(events: &mut broadcast::Receiver<ServerEvent>) -> bool {
    loop {
        match events.try_recv() {
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Closed) => return false,
        }
    }
}

async fn check(state: &AppState, last: Option<u64>) -> Option<u64> {
    let listed = state.clone();
    match tokio::task::spawn_blocking(move || super::missions(&listed)).await {
        Ok(Ok(listing)) => {
            if last != Some(listing.revision) {
                state.engine.emit_scope(StateScope::Missions);
            }
            Some(listing.revision)
        }
        Ok(Err(error)) => {
            tracing::warn!(%error, "missions could not be listed");
            last
        }
        Err(error) => {
            tracing::error!(%error, "the missions listing stopped");
            last
        }
    }
}
