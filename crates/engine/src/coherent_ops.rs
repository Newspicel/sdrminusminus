use std::collections::BTreeMap;

use sdrmm_channels::coherent::{CoherentCtx, coherent_descriptor};
use sdrmm_wire::{CalParams, Coherence, CoherentParams};
use tokio::sync::broadcast;

use crate::{
    Engine, EngineError,
    coherent::{
        CoherentCommand, CoherentHost, CoherentRuntime, CoherentSinks, CoherentStart,
        CoherentUpdate, SurfaceUpdate,
    },
    sample_rate_of,
};

const UPDATE_CHANNEL_CAP: usize = 64;
const SURFACE_CHANNEL_CAP: usize = 8;

pub(crate) struct CoherentState {
    pub(crate) runtime: CoherentRuntime,
    pub(crate) updates: broadcast::Sender<CoherentUpdate>,
    pub(crate) surfaces: broadcast::Sender<SurfaceUpdate>,
    pub(crate) nodes: BTreeMap<u32, CoherentParams>,
}

impl Engine {
    /// Puts a coherent processor on a radio's lanes, starting the aggregator if this is the first
    /// one. The node id is drawn from the same counter as channels, so a decoded record can name
    /// either without the two ever colliding.
    pub fn add_coherent(
        &self,
        ds: u32,
        params: CoherentParams,
        lanes: Vec<u32>,
    ) -> Result<u32, EngineError> {
        if !params.valid() {
            return Err(EngineError::Coherent(format!(
                "{} settings are outside their allowed ranges",
                params.type_id()
            )));
        }
        let descriptor = coherent_descriptor(params.type_id()).ok_or_else(|| {
            EngineError::Coherent(format!("unknown processor {}", params.type_id()))
        })?;
        let mut inner = self.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let tier = state.capabilities.coherence;
        if tier == Coherence::None {
            return Err(EngineError::Coherent(
                "this radio's lanes share neither a clock nor a synthesizer, so nothing coherent \
                 can run on them"
                    .to_string(),
            ));
        }
        let streams = state.rx_streams();
        if streams < descriptor.min_lanes {
            return Err(EngineError::Coherent(format!(
                "{} needs at least {} lanes, this radio has {streams}",
                descriptor.name, descriptor.min_lanes
            )));
        }
        if lanes.iter().any(|lane| *lane >= streams) {
            return Err(EngineError::Coherent(format!(
                "an element is wired to a lane this radio does not have: {lanes:?}"
            )));
        }
        let elements = lanes.len();
        let sample_rate = sample_rate_of(&state.settings);
        let center_hz = state.settings.center_hz.unwrap_or(crate::DEFAULT_CENTER_HZ);
        let cal = cal_of(&params);
        if state.coherent.is_none() {
            let taps = crate::lock_runtime(&state.runtime)
                .take_coherent()
                .ok_or_else(|| {
                    EngineError::Coherent(
                        "this radio has no coherent taps; it is not streaming all its lanes"
                            .to_string(),
                    )
                })?;
            let runtime = CoherentRuntime::start(CoherentStart {
                set: ds,
                taps,
                tier,
                center_hz,
                cal,
            })?;
            state.coherent = Some(CoherentState {
                runtime,
                updates: broadcast::channel(UPDATE_CHANNEL_CAP).0,
                surfaces: broadcast::channel(SURFACE_CHANNEL_CAP).0,
                nodes: BTreeMap::new(),
            });
        }
        let node = state.next_channel_id;
        state.next_channel_id += 1;
        let sinks = {
            let coherent = state.coherent.as_ref().ok_or_else(|| {
                EngineError::Coherent("the coherent runtime went away".to_string())
            })?;
            CoherentSinks {
                updates: coherent.updates.clone(),
                surfaces: coherent.surfaces.clone(),
                decoded: self.decoded_sink(ds, node),
            }
        };
        let host = CoherentHost::build(
            node,
            CoherentCtx {
                lanes: elements,
                sample_rate,
                center_hz,
            },
            &params,
            sinks,
            lanes,
        )?;
        let Some(coherent) = state.coherent.as_mut() else {
            return Err(EngineError::Coherent(
                "the coherent runtime went away".to_string(),
            ));
        };
        coherent.runtime.send(CoherentCommand::Cal {
            params: Box::new(cal),
        });
        coherent.runtime.send(CoherentCommand::Add { node, host });
        coherent.nodes.insert(node, params);
        inner.revision += 1;
        Ok(node)
    }

    pub fn apply_coherent(
        &self,
        ds: u32,
        node: u32,
        params: CoherentParams,
        lanes: Vec<u32>,
    ) -> Result<(), EngineError> {
        if !params.valid() {
            return Err(EngineError::Coherent(format!(
                "{} settings are outside their allowed ranges",
                params.type_id()
            )));
        }
        let mut inner = self.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let streams = state.rx_streams();
        if lanes.iter().any(|lane| *lane >= streams) {
            return Err(EngineError::Coherent(format!(
                "an element is wired to a lane this radio does not have: {lanes:?}"
            )));
        }
        let elements = lanes.len();
        let sample_rate = sample_rate_of(&state.settings);
        let center_hz = state.settings.center_hz.unwrap_or(crate::DEFAULT_CENTER_HZ);
        let cal = cal_of(&params);
        let decoded = self.decoded_sink(ds, node);
        let coherent = state
            .coherent
            .as_mut()
            .ok_or_else(|| EngineError::Coherent("no coherent processor is running".to_string()))?;
        let existing = coherent
            .nodes
            .get(&node)
            .ok_or_else(|| EngineError::Coherent(format!("no coherent node {node}")))?;
        if existing.type_id() != params.type_id() {
            return Err(EngineError::Coherent(
                "a coherent node cannot change what it is; remove it and add the other".to_string(),
            ));
        }
        let sinks = CoherentSinks {
            updates: coherent.updates.clone(),
            surfaces: coherent.surfaces.clone(),
            decoded,
        };
        let host = CoherentHost::build(
            node,
            CoherentCtx {
                lanes: elements,
                sample_rate,
                center_hz,
            },
            &params,
            sinks,
            lanes,
        )?;
        coherent.runtime.send(CoherentCommand::Cal {
            params: Box::new(cal),
        });
        coherent.runtime.send(CoherentCommand::Add { node, host });
        coherent.nodes.insert(node, params);
        inner.revision += 1;
        Ok(())
    }

    /// Takes one processor off the lanes, and stops the aggregator once the last one goes so the
    /// taps are back where the next node can pick them up.
    pub fn remove_coherent(&self, ds: u32, node: u32) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let state = inner
            .device_sets
            .get_mut(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let Some(coherent) = state.coherent.as_mut() else {
            return Ok(());
        };
        coherent.nodes.remove(&node);
        coherent.runtime.send(CoherentCommand::Remove { node });
        if coherent.nodes.is_empty() {
            let Some(coherent) = state.coherent.take() else {
                return Ok(());
            };
            let taps = coherent.runtime.stop();
            crate::lock_runtime(&state.runtime).return_coherent(taps);
        }
        inner.revision += 1;
        Ok(())
    }

    /// Throws the calibration away and solves it again from scratch, which is what an operator
    /// asks for after moving an antenna or switching the splitter in.
    pub fn recalibrate_coherent(&self, ds: u32) -> Result<(), EngineError> {
        let inner = self.lock();
        let state = inner
            .device_sets
            .get(&ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let coherent = state
            .coherent
            .as_ref()
            .ok_or_else(|| EngineError::Coherent("no coherent processor is running".to_string()))?;
        coherent.runtime.send(CoherentCommand::Recalibrate);
        Ok(())
    }

    #[must_use]
    pub fn subscribe_coherent(&self, ds: u32) -> Option<broadcast::Receiver<CoherentUpdate>> {
        let inner = self.lock();
        inner
            .device_sets
            .get(&ds)?
            .coherent
            .as_ref()
            .map(|coherent| coherent.updates.subscribe())
    }

    #[must_use]
    pub fn subscribe_surfaces(&self, ds: u32) -> Option<broadcast::Receiver<SurfaceUpdate>> {
        let inner = self.lock();
        inner
            .device_sets
            .get(&ds)?
            .coherent
            .as_ref()
            .map(|coherent| coherent.surfaces.subscribe())
    }

    #[must_use]
    pub fn coherence_of(&self, ds: u32) -> Coherence {
        self.lock()
            .device_sets
            .get(&ds)
            .map_or(Coherence::None, |state| state.capabilities.coherence)
    }

    #[must_use]
    pub fn coherent_nodes(&self, ds: u32) -> Vec<u32> {
        self.lock()
            .device_sets
            .get(&ds)
            .and_then(|state| state.coherent.as_ref())
            .map(|coherent| coherent.nodes.keys().copied().collect())
            .unwrap_or_default()
    }

    #[must_use]
    pub fn coherent_realignments(&self, ds: u32) -> u64 {
        self.lock()
            .device_sets
            .get(&ds)
            .and_then(|state| state.coherent.as_ref())
            .map_or(0, |coherent| coherent.runtime.realignments())
    }

    /// Tells the aggregator the front end moved. A shared synthesizer keeps its phase across a
    /// retune; separate ones do not, so the solution is thrown away and rebuilt.
    pub(crate) fn notify_coherent_meta(&self, ds: u32, center_hz: f64, retuned: bool) {
        let inner = self.lock();
        let Some(state) = inner.device_sets.get(&ds) else {
            return;
        };
        let Some(coherent) = state.coherent.as_ref() else {
            return;
        };
        let scrambles = retuned && !state.capabilities.coherence.has_phase();
        coherent.runtime.send(CoherentCommand::Meta {
            center_hz,
            retuned: scrambles,
        });
    }
}

fn cal_of(params: &CoherentParams) -> CalParams {
    match params {
        CoherentParams::Df(df) => df.cal,
        CoherentParams::Combiner(combiner) => combiner.cal,
        CoherentParams::PassiveRadar(_) => CalParams::default(),
    }
}
