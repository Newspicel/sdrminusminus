use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use sdrmm_channels::coherent::{CoherentCtx, coherent_descriptor};
use sdrmm_wire::{CalParams, CalSource, Coherence, CoherentParams};
use tokio::sync::broadcast::{self, error::TryRecvError};

use crate::{
    Engine, EngineError,
    coherent::{
        CoherentCommand, CoherentHost, CoherentRuntime, CoherentSinks, CoherentStart,
        CoherentUpdate, SurfaceUpdate,
    },
    runtime::CaptureRuntime,
    sample_rate_of,
};

const UPDATE_CHANNEL_CAP: usize = 64;
const SURFACE_CHANNEL_CAP: usize = 8;

/// How long a solve gets with the reference switched in. One that has not converged by then will
/// not, and an array listening to its own noise source is deaf to everything else.
const REFERENCE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the pipeline is given to fill with what the switch changed, on the way in and on the
/// way out. Everything the array reads in between is a measurement of itself.
const REFERENCE_SETTLE: Duration = Duration::from_millis(250);
const REFERENCE_POLL: Duration = Duration::from_millis(20);

pub(crate) struct CoherentState {
    pub(crate) runtime: CoherentRuntime,
    pub(crate) updates: broadcast::Sender<CoherentUpdate>,
    pub(crate) surfaces: broadcast::Sender<SurfaceUpdate>,
    pub(crate) nodes: BTreeMap<u32, CoherentParams>,
    /// Set while a calibration owns the radio's reference switch, so a second one does not start
    /// on top of it and put the antennas back halfway through the first.
    pub(crate) calibrating: Arc<AtomicBool>,
}

/// Everything the sequence needs to run without holding the engine open while it waits.
struct Reference {
    runtime: Arc<Mutex<CaptureRuntime>>,
    commands: mpsc::Sender<CoherentCommand>,
    updates: broadcast::Receiver<CoherentUpdate>,
    busy: Arc<AtomicBool>,
}

impl Reference {
    fn switch(&self, on: bool) -> Result<(), sdrmm_device::DeviceError> {
        crate::lock_runtime(&self.runtime).set_noise_source(on)
    }

    fn tell(&self, on: bool) {
        let _ = self.commands.send(CoherentCommand::Reference(on));
    }

    fn restart(&self) {
        let _ = self.commands.send(CoherentCommand::Recalibrate);
    }

    /// Waits for the aggregator to report a state, which it does as soon as it reaches one
    /// rather than on the next interval.
    fn wait(&mut self, wanted: Wanted) -> bool {
        let deadline = Instant::now() + REFERENCE_TIMEOUT;
        while Instant::now() < deadline {
            match self.updates.try_recv() {
                Ok(update) if wanted.reached(&update.cal) => return true,
                Ok(_) | Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty) => std::thread::sleep(REFERENCE_POLL),
                Err(TryRecvError::Closed) => return false,
            }
        }
        false
    }
}

/// Switches the radio's own reference into the lanes, waits for the answer, and puts the array
/// back on its antennas whether or not one arrived.
///
/// The order is what makes the answer worth having. The old solution goes first, so nothing
/// measured against the antennas can be mistaken for the new one; the reference is given time to
/// reach the lanes before the array is told to believe it; and it is believed for a moment after
/// the switch opens again, because the samples already in flight still carry it.
fn solve_against_reference(mut reference: Reference, ds: u32) {
    reference.restart();
    if !reference.wait(Wanted::Cleared) {
        tracing::warn!(
            device_set = ds,
            "the array never let go of its old calibration"
        );
        return;
    }
    if let Err(error) = reference.switch(true) {
        tracing::warn!(device_set = ds, %error, "the radio would not switch its reference in");
        return;
    }
    std::thread::sleep(REFERENCE_SETTLE);
    reference.tell(true);
    let solved = reference.wait(Wanted::Solved);
    if let Err(error) = reference.switch(false) {
        tracing::error!(device_set = ds, %error, "the radio is still on its own reference");
    }
    std::thread::sleep(REFERENCE_SETTLE);
    reference.tell(false);
    if solved {
        tracing::info!(
            device_set = ds,
            "the array solved against its own reference"
        );
    } else {
        tracing::warn!(
            device_set = ds,
            "no solution against the reference; the lanes are left uncalibrated"
        );
    }
}

/// What the sequence is waiting for the aggregator to say.
#[derive(Clone, Copy)]
enum Wanted {
    Cleared,
    Solved,
}

impl Wanted {
    fn reached(self, cal: &sdrmm_wire::CalState) -> bool {
        match self {
            Self::Cleared => !cal.solved || cal.phase_unknown,
            Self::Solved => cal.solved && !cal.phase_unknown,
        }
    }
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
        if state.rate_patches > 0 {
            return Err(EngineError::Coherent(
                "wait for the sample rate change before starting coherent processors".into(),
            ));
        }
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
                switched_reference: state.capabilities.noise_source,
            })?;
            state.coherent = Some(CoherentState {
                runtime,
                updates: broadcast::channel(UPDATE_CHANNEL_CAP).0,
                surfaces: broadcast::channel(SURFACE_CHANNEL_CAP).0,
                nodes: BTreeMap::new(),
                calibrating: Arc::new(AtomicBool::new(false)),
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
        drop(inner);
        self.calibrate_against_reference(ds);
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
        drop(inner);
        self.calibrate_against_reference(ds);
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
    ///
    /// A radio that carries its own reference then solves against it without anyone reaching for
    /// a switch, which is the only way a bank of tuners is usable at all: they come up at a new
    /// set of phases every time they are retuned.
    pub fn recalibrate_coherent(&self, ds: u32) -> Result<(), EngineError> {
        {
            let inner = self.lock();
            let state = inner
                .device_sets
                .get(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let coherent = state.coherent.as_ref().ok_or_else(|| {
                EngineError::Coherent("no coherent processor is running".to_string())
            })?;
            coherent.runtime.send(CoherentCommand::Recalibrate);
        }
        self.calibrate_against_reference(ds);
        Ok(())
    }

    /// Solves the calibration against the radio's own reference, when it has one and something
    /// running on it asked to be calibrated against one.
    ///
    /// Switching the reference in takes the array off its antennas, so the sequence runs on its
    /// own thread and nothing waits on it. An operator who asked for a signal calibration instead
    /// is left alone: what to solve against is their call, not ours.
    pub(crate) fn calibrate_against_reference(&self, ds: u32) {
        let Some(reference) = self.reference_for(ds) else {
            return;
        };
        if reference.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let busy = reference.busy.clone();
        let done = busy.clone();
        let spawned = std::thread::Builder::new()
            .name("sdrmm-calibrate".to_string())
            .spawn(move || {
                solve_against_reference(reference, ds);
                done.store(false, Ordering::Release);
            });
        if let Err(error) = spawned {
            busy.store(false, Ordering::Release);
            tracing::warn!(device_set = ds, %error, "could not start the calibration");
        }
    }

    fn reference_for(&self, ds: u32) -> Option<Reference> {
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        if !state.capabilities.noise_source {
            return None;
        }
        if state.scanner.is_some() || state.hunt.is_some() {
            return None;
        }
        let coherent = state.coherent.as_ref()?;
        let wanted = coherent
            .nodes
            .values()
            .any(|params| cal_of(params).source == CalSource::Noise);
        wanted.then(|| Reference {
            runtime: state.runtime.clone(),
            commands: coherent.runtime.sender(),
            updates: coherent.updates.subscribe(),
            busy: coherent.calibrating.clone(),
        })
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
        drop(inner);
        if scrambles {
            self.calibrate_against_reference(ds);
        }
    }
}

fn cal_of(params: &CoherentParams) -> CalParams {
    match params {
        CoherentParams::Df(df) => df.cal,
        CoherentParams::Combiner(combiner) => combiner.cal,
        CoherentParams::PassiveRadar(_) => CalParams::default(),
    }
}
