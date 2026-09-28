use std::{
    collections::HashMap,
    sync::{
        Weak,
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::Duration,
};

use sdrmm_engine::Engine;
use sdrmm_wire::{
    DecodedRecord, DecoderEvent, DfEstimate, DfFusionState, EventOrigin, FusionGridOwned,
    NO_CHANNEL, PositionFix, ServerEvent, SurfaceFrame,
};

use super::{FusionHub, FusionOutcome, Job, Refusal, Sighting, now_s};
use crate::surfaces::SurfaceHub;

const IDLE_WAIT: Duration = Duration::from_millis(250);
const REFUSAL_NOTICE_S: f64 = 1.0;
const GUIDE_REFRESH_S: f64 = 5.0;

#[derive(Clone, Copy, PartialEq, Eq)]
struct GuideMark {
    revision: Option<u32>,
    no_guide_position: bool,
    no_bearings: bool,
}

impl GuideMark {
    fn of(state: &DfFusionState) -> Self {
        Self {
            revision: state.nav.map(|nav| nav.revision),
            no_guide_position: state.no_guide_position,
            no_bearings: state.no_bearings,
        }
    }
}

pub(super) struct Worker {
    hub: Weak<FusionHub>,
    engine: Weak<Engine>,
    surfaces: Weak<SurfaceHub>,
    refusals: HashMap<String, f64>,
    guides: HashMap<String, (GuideMark, f64)>,
}

impl Worker {
    pub(super) fn new(
        hub: Weak<FusionHub>,
        engine: Weak<Engine>,
        surfaces: Weak<SurfaceHub>,
    ) -> Self {
        Self {
            hub,
            engine,
            surfaces,
            refusals: HashMap::new(),
            guides: HashMap::new(),
        }
    }

    pub(super) fn run(mut self, jobs: &Receiver<Job>) {
        loop {
            let job = match jobs.recv_timeout(IDLE_WAIT) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let (Some(hub), Some(engine)) = (self.hub.upgrade(), self.engine.upgrade()) else {
                return;
            };
            let surfaces = self.surfaces.upgrade();
            let surfaces = surfaces.as_deref();
            match job {
                Some(Job::Bearing(sighting)) => {
                    self.observe(&hub, &engine, surfaces, &sighting);
                }
                Some(Job::Guide { node, fix, now_s }) => {
                    self.guide(&hub, &engine, surfaces, &node, fix.as_deref(), now_s);
                }
                None => {}
            }
            for (node, frame) in hub.due_frames(now_s()) {
                publish_frame(surfaces, &node, frame);
            }
        }
    }

    fn observe(
        &mut self,
        hub: &FusionHub,
        engine: &Engine,
        surfaces: Option<&SurfaceHub>,
        sighting: &Sighting,
    ) {
        match hub.observe(
            &sighting.node,
            &sighting.bearing,
            sighting.now_s,
            &sighting.at,
        ) {
            Ok(outcome) => {
                if let Some(estimate) = outcome.first_fix {
                    engine.publish_decoded(fix_record(sighting, estimate));
                }
                deliver(engine, surfaces, &sighting.node, outcome);
            }
            Err(refusal) => self.refused(hub, engine, sighting, refusal),
        }
    }

    fn refused(&mut self, hub: &FusionHub, engine: &Engine, sighting: &Sighting, refusal: Refusal) {
        tracing::debug!(node = sighting.node, ?refusal, "bearing refused by fusion");
        let due = self
            .refusals
            .get(&sighting.node)
            .is_none_or(|told| sighting.now_s - told >= REFUSAL_NOTICE_S);
        if due && let Some(state) = hub.state(&sighting.node) {
            self.refusals.insert(sighting.node.clone(), sighting.now_s);
            emit(engine, &sighting.node, state);
        }
    }

    fn guide(
        &mut self,
        hub: &FusionHub,
        engine: &Engine,
        surfaces: Option<&SurfaceHub>,
        node: &str,
        fix: Option<&PositionFix>,
        now_s: f64,
    ) {
        let Some(outcome) = hub.guide(node, fix, now_s) else {
            self.guides.remove(node);
            return;
        };
        if let Some(frame) = outcome.grid_frame {
            publish_frame(surfaces, node, frame);
        }
        let mark = GuideMark::of(&outcome.state);
        let due = self
            .guides
            .get(node)
            .is_none_or(|(told, at)| *told != mark || now_s - at >= GUIDE_REFRESH_S);
        if due {
            self.guides.insert(node.to_owned(), (mark, now_s));
            emit(engine, node, outcome.state);
        }
    }
}

fn fix_record(sighting: &Sighting, estimate: DfEstimate) -> DecodedRecord {
    DecodedRecord {
        origin: Some(EventOrigin {
            node: sighting.node.clone(),
            transmission: 0,
        }),
        device_set: sighting.device_set,
        channel: NO_CHANNEL,
        at: sighting.at.clone(),
        freq_hz: sighting.bearing.freq_hz.unwrap_or(0.0),
        event: DecoderEvent::DfFix(estimate),
        sinks: Vec::new(),
    }
}

fn emit(engine: &Engine, node: &str, state: DfFusionState) {
    engine.emit_event(ServerEvent::DfFusionUpdate {
        node: node.to_owned(),
        state: Box::new(state),
    });
}

fn publish_frame(surfaces: Option<&SurfaceHub>, node: &str, frame: FusionGridOwned) {
    if let Some(surfaces) = surfaces {
        let seq = frame.seq;
        surfaces.publish(
            node,
            seq,
            std::sync::Arc::new(SurfaceFrame::FusionGrid(frame)),
        );
    }
}

pub(super) fn deliver(
    engine: &Engine,
    surfaces: Option<&SurfaceHub>,
    node: &str,
    outcome: FusionOutcome,
) {
    if let Some(frame) = outcome.grid_frame {
        publish_frame(surfaces, node, frame);
    }
    emit(engine, node, outcome.state);
}
