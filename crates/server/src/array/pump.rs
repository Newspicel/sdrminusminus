use std::{collections::HashMap, ops::ControlFlow, sync::Arc, time::Duration};

use sdrmm_engine::ArrayEvent;
use sdrmm_wire::{ArrayCalRecord, ArrayStatus, ProcessorReading, ServerEvent, SurfaceFrame};
use tokio::{
    sync::broadcast::{self, error::RecvError},
    time::MissedTickBehavior,
};

use crate::AppState;

const STATUS_EVERY: Duration = Duration::from_millis(500);

pub(super) async fn run(
    state: AppState,
    mut events: broadcast::Receiver<ArrayEvent>,
    mut positions: broadcast::Receiver<ServerEvent>,
) {
    let mut emitted: HashMap<String, ArrayStatus> = HashMap::new();
    let mut tick = tokio::time::interval(STATUS_EVERY);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            event = events.recv() => {
                if handle(&state, event).is_break() {
                    break;
                }
            }
            position = positions.recv() => match position {
                Ok(ServerEvent::PositionChanged { node, fix, .. }) => {
                    super::forward_pose(&state, &node, fix.as_ref());
                }
                Ok(_) => {}
                Err(RecvError::Lagged(count)) => {
                    tracing::warn!(count, "positions skipped by arrays; the latest fix stands in");
                    super::resend_poses(&state);
                }
                Err(RecvError::Closed) => break,
            },
            _ = tick.tick() => publish_statuses(&state, &mut emitted),
        }
    }
}

pub(crate) fn handle(state: &AppState, received: Result<ArrayEvent, RecvError>) -> ControlFlow<()> {
    match received {
        Ok(ArrayEvent::Report { processor, reading }) => report(state, processor, &reading),
        Ok(ArrayEvent::Surface {
            processor,
            seq,
            surface,
        }) => draw(state, &processor, seq, surface),
        Ok(ArrayEvent::Solved { array, record }) => keep_solution(state, array, record),
        Err(RecvError::Lagged(count)) => {
            tracing::warn!(count, "array updates lost");
            state.engine.emit_event(ServerEvent::Error {
                message: format!("array updates lost: {count}"),
            });
        }
        Err(RecvError::Closed) => return ControlFlow::Break(()),
    }
    ControlFlow::Continue(())
}

fn report(state: &AppState, processor: String, reading: &ProcessorReading) {
    if let ProcessorReading::PassiveRadar(update) = reading
        && !update.events.is_empty()
    {
        tracing::warn!(
            node = %processor,
            "radar events wait for the radar hub: {}",
            update.events.len()
        );
    }
    state.engine.emit_event(ServerEvent::ProcessorUpdate {
        node: processor,
        reading: Box::new(reading.clone()),
    });
}

fn draw(state: &AppState, processor: &str, seq: u32, surface: Arc<SurfaceFrame>) {
    if state.arrays.holds_processor(processor) {
        state.surfaces.publish(processor, seq, surface);
    } else {
        tracing::debug!(processor, seq, "a surface outlived its processor");
    }
}

fn keep_solution(state: &AppState, array: String, record: ArrayCalRecord) {
    let store = state.store.clone();
    let engine = state.engine.clone();
    let save = move || {
        if let Err(error) = store.put_array_calibration(&record) {
            tracing::warn!(%error, array, "array calibration not saved");
            engine.emit_event(ServerEvent::Error {
                message: format!("calibration of {array} not saved: {error}"),
            });
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            drop(runtime.spawn_blocking(save));
        }
        Err(_) => save(),
    }
}

fn publish_statuses(state: &AppState, emitted: &mut HashMap<String, ArrayStatus>) {
    let statuses = state.engine.array_statuses();
    emitted.retain(|node, _| statuses.iter().any(|status| status.node == *node));
    for status in statuses {
        if emitted.get(&status.node) == Some(&status) {
            continue;
        }
        emitted.insert(status.node.clone(), status.clone());
        state.engine.emit_event(ServerEvent::ArrayUpdate {
            status: Box::new(status),
        });
    }
}
