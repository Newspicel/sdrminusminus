mod lifecycle;
mod members;
mod pose;
mod processors;
mod recording;
mod status;
mod tuning;

#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Weak},
};

use sdrmm_channels::ChannelError;
use sdrmm_device::lock;
use sdrmm_wire::{
    ArrayCal, ArrayCalSource, ArrayFailure, ArrayGain, ArrayTune, PositionFix, ServerEvent,
    StateScope,
};
use tokio::sync::broadcast;

use self::{members::Leased, pose::PoseTrack, processors::ProcessorRecord};
use crate::{
    Engine, EngineError, Inner,
    array::{
        ArrayControl, ArrayEvent, ArrayRecording, ArrayRuntime, ArrayShape, ArraySpec, Command,
        ControlCommand, ControlConfig, LaneRef, LiveFrame, NoiseSwitch, RuntimeSetup, StatusBoard,
        SyncContext, TapPort, TierDecision, tuner::GainMenu,
    },
};

const AUTO_NEEDS_CAL: &str = "Auto gain needs cal";
const CAL_OFF: &str = "Cal is off";

pub(crate) struct ArrayState {
    spec: ArraySpec,
    runtime: Option<ArrayRuntime>,
    tier: TierDecision,
    processors: BTreeMap<String, ProcessorRecord>,
    board: Arc<StatusBoard>,
    anchor: Option<u32>,
    tune: ArrayTune,
    frame: LiveFrame,
    shape: ArrayShape,
    config: ControlConfig,
    leases: Vec<Option<Lease>>,
    failure: Option<ArrayFailure>,
    pose: PoseTrack,
    recording: Option<ArrayRecording>,
    gain_menu: Option<GainMenu>,
    drift: Option<f64>,
}

impl ArrayState {
    fn runtime(&self) -> Result<&ArrayRuntime, EngineError> {
        self.runtime.as_ref().ok_or_else(stopped)
    }

    fn send(&self, command: Command) -> Result<(), EngineError> {
        self.runtime()?.send(command)
    }

    fn members(&self) -> BTreeSet<u32> {
        self.spec
            .lanes
            .iter()
            .flatten()
            .map(|lane| lane.device_set)
            .collect()
    }

    fn uses(&self, ds: u32) -> bool {
        self.spec
            .lanes
            .iter()
            .flatten()
            .any(|lane| lane.device_set == ds)
    }

    fn slots_on(&self, ds: u32) -> Vec<usize> {
        self.spec
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.is_some_and(|lane| lane.device_set == ds))
            .map(|(slot, _)| slot)
            .collect()
    }

    fn tell(&self, command: ControlCommand) -> Result<(), EngineError> {
        let told = self.runtime()?.control(command);
        if let Err(error) = &told {
            self.board.stopped(error.to_string());
        }
        told
    }

    fn switch_noise(&self, noise: Option<NoiseSwitch>) {
        if let Err(error) = self.tell(ControlCommand::NoiseSwitch(noise)) {
            tracing::warn!(array = %self.spec.node, %error, "array controller missed its noise source");
        }
    }

    fn configure(&mut self, config: ControlConfig) {
        if config == self.config {
            return;
        }
        self.config = config.clone();
        if let Err(error) = self.tell(ControlCommand::Configure(Box::new(config))) {
            tracing::warn!(array = %self.spec.node, %error, "array controller missed a new configuration");
        }
    }

    fn swap_lanes(&mut self, changed: &[usize], leased: Leased) -> Result<(), EngineError> {
        if changed.is_empty() {
            return Ok(());
        }
        let (mut feeds, mut leases) = leased;
        let mut swaps = Vec::with_capacity(changed.len());
        let mut lost = Vec::new();
        for &slot in changed {
            if let Some(old) = self.leases.get_mut(slot).and_then(Option::take) {
                old.release();
            }
            let feed = feeds.get_mut(slot).and_then(Option::take);
            let lease = leases.get_mut(slot).and_then(Option::take);
            match (feed, lease, self.leases.get_mut(slot)) {
                (Some(feed), Some(lease), Some(held)) => {
                    *held = Some(lease);
                    swaps.push((slot, feed));
                }
                _ => lost.push(slot),
            }
        }
        if !swaps.is_empty() {
            self.send(Command::SwapFeeds { slots: swaps })?;
        }
        if !lost.is_empty() {
            self.send(Command::LanesLost { slots: lost })?;
        }
        self.tell(ControlCommand::Resync { coarse: true })
    }

    fn reframe(&mut self, frame: LiveFrame) -> Result<(), EngineError> {
        if frame == self.frame {
            return Ok(());
        }
        self.send(Command::Frame {
            frame: Box::new(frame.clone()),
        })?;
        self.frame = frame;
        Ok(())
    }
}

pub(crate) struct Lease {
    port: Arc<TapPort>,
    id: u64,
}

impl Lease {
    fn release(&self) {
        self.port.release(self.id);
        self.port.collect();
    }
}

#[derive(Default)]
struct Carried {
    processors: BTreeMap<String, ProcessorRecord>,
    pose: PoseTrack,
}

fn stopped() -> EngineError {
    EngineError::Array(ArrayFailure::Stopped {
        message: "Stopped".to_owned(),
    })
}

fn check_tune(tune: &ArrayTune, cal: &ArrayCal) -> Result<(), EngineError> {
    if !tune.center_hz.is_finite() || tune.center_hz <= 0.0 {
        return Err(ChannelError::Refused("Center out of range").into());
    }
    if let Some(problem) = tune.gain.problem() {
        return Err(ChannelError::Refused(problem).into());
    }
    if tune.gain == ArrayGain::Auto && cal.source == ArrayCalSource::Off {
        return Err(EngineError::Processor(AUTO_NEEDS_CAL.to_owned()));
    }
    Ok(())
}

fn anchor_of(lanes: &[Option<LaneRef>]) -> Option<u32> {
    lanes.iter().flatten().next().map(|lane| lane.device_set)
}

fn rehold(
    inner: &mut Inner,
    node: &str,
    old: &[Option<LaneRef>],
    new: &[Option<LaneRef>],
    changed: &[usize],
) {
    for lane in changed
        .iter()
        .filter_map(|slot| old.get(*slot).copied().flatten())
    {
        if let Some(device) = inner.device_sets.get_mut(&lane.device_set)
            && device
                .held
                .get(&lane.stream)
                .is_some_and(|array| array == node)
        {
            device.held.remove(&lane.stream);
        }
    }
    for lane in changed
        .iter()
        .filter_map(|slot| new.get(*slot).copied().flatten())
    {
        if let Some(device) = inner.device_sets.get_mut(&lane.device_set) {
            device.held.insert(lane.stream, node.to_owned());
        }
    }
}

fn find<'a>(inner: &'a Inner, node: &str) -> Result<&'a ArrayState, EngineError> {
    inner
        .arrays
        .get(node)
        .ok_or_else(|| EngineError::ArrayNotFound(node.to_owned()))
}

fn find_mut<'a>(inner: &'a mut Inner, node: &str) -> Result<&'a mut ArrayState, EngineError> {
    inner
        .arrays
        .get_mut(node)
        .ok_or_else(|| EngineError::ArrayNotFound(node.to_owned()))
}

impl Engine {
    pub fn apply_array(self: &Arc<Self>, spec: ArraySpec) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        let current = self.lock().arrays.get(&spec.node).map(|state| {
            let running = state
                .runtime
                .as_ref()
                .is_some_and(|runtime| !runtime.is_finished());
            (state.spec.clone(), running)
        });
        match current {
            None => self.start_array(spec, Carried::default()),
            Some((current, true)) if current == spec => Ok(()),
            Some((current, true))
                if current.lanes.len() == spec.lanes.len()
                    && anchor_of(&current.lanes) == anchor_of(&spec.lanes) =>
            {
                self.revise_array(&spec)
            }
            Some(_) => self.restart_array(spec),
        }
    }

    pub fn remove_array(&self, node: &str) -> Result<(), EngineError> {
        let _edits = lock(&self.array_edits);
        self.drop_array(node)
    }

    pub fn retain_arrays(&self, drawn: &[String]) {
        let _edits = lock(&self.array_edits);
        let gone: Vec<String> = self
            .lock()
            .arrays
            .keys()
            .filter(|node| !drawn.contains(node))
            .cloned()
            .collect();
        for node in gone {
            if let Err(error) = self.drop_array(&node) {
                tracing::warn!(array = %node, %error, "an array left the patch but did not stop cleanly");
            }
        }
    }

    pub fn calibrate_array(&self, node: &str) -> Result<(), EngineError> {
        let inner = self.lock();
        let state = find(&inner, node)?;
        if state.spec.settings.cal.source == ArrayCalSource::Off {
            return Err(EngineError::Processor(CAL_OFF.to_owned()));
        }
        state.tell(ControlCommand::Recalibrate)
    }

    pub fn update_array_pose(
        &self,
        node: &str,
        fix: Option<PositionFix>,
        received_ns: i64,
    ) -> Result<(), EngineError> {
        let mut inner = self.lock();
        let state = find_mut(&mut inner, node)?;
        let sample = state.pose.sample(fix.as_ref(), received_ns);
        let sent = state.send(Command::Pose { sample });
        if sent.is_err() {
            state.board.add_events_lost(1);
        }
        sent
    }

    #[must_use]
    pub fn subscribe_arrays(&self) -> broadcast::Receiver<ArrayEvent> {
        self.array_tx.subscribe()
    }

    #[cfg(feature = "probe")]
    pub fn hold_array(
        &self,
        node: &str,
        hold: impl FnOnce() + Send + 'static,
    ) -> Result<(), EngineError> {
        find(&self.lock(), node)?.send(Command::Hold(Box::new(hold)))
    }

    fn start_array(self: &Arc<Self>, spec: ArraySpec, carried: Carried) -> Result<(), EngineError> {
        let survey = members::survey(&self.lock(), &spec)?;
        let tune = spec.tune.unwrap_or_else(|| survey.default_tune());
        check_tune(&tune, &spec.settings.cal)?;
        let plan = survey.plan(spec.settings.tuning, &tune)?;
        self.apply_plan(&plan, None)?;
        let survey = members::survey(&self.lock(), &spec)?;
        let tier = survey.tier(spec.settings.declared, false);
        let (feeds, leases) = survey.lease()?;
        let frame = survey.frame(&spec, &tier, &plan.lane_centers_hz, tune.center_hz);
        let board = Arc::new(StatusBoard::new(spec.lanes.len()));
        board.control().gain_db = plan.gain_db.or_else(|| survey.current_gain_db());
        let config = ControlConfig {
            cal: spec.settings.cal,
            gain: tune.gain,
            needs_time: false,
            needs_phase: false,
            tier,
            sample_rate: frame.sample_rate,
        };
        let engine: Weak<Self> = Arc::downgrade(self);
        let control: Weak<dyn ArrayControl> = engine;
        let started = ArrayRuntime::start(RuntimeSetup {
            node: spec.node.clone(),
            feeds,
            frame: frame.clone(),
            board: board.clone(),
            control,
            config: config.clone(),
            events: self.array_tx.clone(),
        });
        let runtime = match started {
            Ok(runtime) => runtime,
            Err(error) => {
                leases.iter().flatten().for_each(Lease::release);
                return Err(error);
            }
        };
        let state = ArrayState {
            runtime: Some(runtime),
            failure: survey.failure(spec.settings.declared),
            shape: survey.shape(&spec),
            anchor: survey.anchor(),
            gain_menu: survey.gain_menu(),
            drift: None,
            processors: BTreeMap::new(),
            pose: carried.pose,
            recording: None,
            spec,
            tier,
            board,
            tune,
            frame,
            config,
            leases,
        };
        state.switch_noise(survey.noise());
        let node = state.spec.node.clone();
        let members = self.install_array(state);
        for record in carried.processors.into_values() {
            self.reinstall(&node, record);
        }
        self.announce(&members);
        Ok(())
    }

    fn install_array(&self, state: ArrayState) -> BTreeSet<u32> {
        let mut inner = self.lock();
        let node = state.spec.node.clone();
        for lane in state.spec.lanes.iter().flatten() {
            if let Some(device) = inner.device_sets.get_mut(&lane.device_set) {
                device.held.insert(lane.stream, node.clone());
            }
        }
        let members = state.members();
        inner.arrays.insert(node, state);
        inner.revision += 1;
        members
    }

    fn restart_array(self: &Arc<Self>, spec: ArraySpec) -> Result<(), EngineError> {
        let Some(state) = self.detach_array(&spec.node, false) else {
            return self.start_array(spec, Carried::default());
        };
        let members = state.members();
        let carried = self.teardown(state, false);
        let kept: Vec<ProcessorRecord> = carried.processors.values().cloned().collect();
        let started = self.start_array(spec, carried);
        if started.is_err() {
            self.forget_processors(&kept);
            self.announce(&members);
        }
        started
    }

    fn revise_array(&self, spec: &ArraySpec) -> Result<(), EngineError> {
        let survey = members::survey(&self.lock(), spec)?;
        let (old, current_tune, drift) = {
            let inner = self.lock();
            let state = find(&inner, &spec.node)?;
            (state.spec.clone(), state.tune, state.drift)
        };
        let tune = spec.tune.unwrap_or(current_tune);
        check_tune(&tune, &spec.settings.cal)?;
        let changed: Vec<usize> = (0..spec.lanes.len())
            .filter(|slot| old.lanes.get(*slot) != spec.lanes.get(*slot))
            .collect();
        let mode_changed = spec.settings.tuning != old.settings.tuning;
        let retune = !changed.is_empty() || tune != current_tune || mode_changed;
        let (plan, survey) = if retune {
            let plan = survey.plan(spec.settings.tuning, &tune)?;
            self.apply_plan(&plan, Some(&spec.node))?;
            (Some(plan), members::survey(&self.lock(), spec)?)
        } else {
            (None, survey)
        };
        let tier = survey.tier(spec.settings.declared, drift.is_some());
        let leased = survey.lease_where(|slot| changed.contains(&slot))?;
        {
            let mut inner = self.lock();
            rehold(&mut inner, &spec.node, &old.lanes, &spec.lanes, &changed);
            let state = find_mut(&mut inner, &spec.node)?;
            state.swap_lanes(&changed, leased)?;
            if !changed.is_empty() {
                state.switch_noise(survey.noise());
            }
            let lane_centers = plan.as_ref().map_or_else(
                || state.frame.lane_centers_hz.clone(),
                |plan| plan.lane_centers_hz.clone(),
            );
            if let Some(gain_db) = plan.as_ref().and_then(|plan| plan.gain_db) {
                state.board.control().gain_db = Some(gain_db);
            }
            let mut frame = survey.frame(spec, &tier, &lane_centers, tune.center_hz);
            frame.needs_time = state.frame.needs_time;
            state.spec = spec.clone();
            state.tier = tier;
            state.tune = tune;
            state.shape = survey.shape(spec);
            state.failure = survey.failure(spec.settings.declared);
            state.gain_menu = survey.gain_menu();
            let config = ControlConfig {
                cal: spec.settings.cal,
                gain: tune.gain,
                tier,
                sample_rate: frame.sample_rate,
                ..state.config.clone()
            };
            state.configure(config);
            state.reframe(frame)?;
            inner.revision += 1;
        }
        if mode_changed {
            self.reinstall_all(&spec.node, |_| true);
        } else if spec.settings.geometry != old.settings.geometry {
            self.reinstall_all(&spec.node, ProcessorRecord::needs_shape);
        }
        if retune {
            self.refresh_lanes(&spec.node);
        }
        let mut members = survey.device_sets();
        members.extend(old.lanes.iter().flatten().map(|lane| lane.device_set));
        self.announce(&members);
        Ok(())
    }

    fn detach_array(&self, node: &str, forget: bool) -> Option<ArrayState> {
        let mut inner = self.lock();
        let state = inner.arrays.remove(node)?;
        if forget {
            for processor in state.processors.keys() {
                inner.processor_index.remove(processor);
            }
            processors::prune_steer_boxes(&mut inner);
        }
        inner.revision += 1;
        Some(state)
    }

    fn drop_array(&self, node: &str) -> Result<(), EngineError> {
        let state = self
            .detach_array(node, true)
            .ok_or_else(|| EngineError::ArrayNotFound(node.to_owned()))?;
        let members = state.members();
        drop(self.teardown(state, true));
        self.announce(&members);
        Ok(())
    }

    fn teardown(&self, mut state: ArrayState, close_lanes: bool) -> Carried {
        if let Some(runtime) = state.runtime.take()
            && let Some(exit) = runtime.stop()
        {
            drop(exit.feeds);
            for host in exit.hosts {
                host.retire();
            }
        }
        if let Some(recording) = state.recording.take() {
            self.finish_recording(&state.spec.node, recording);
        }
        state.leases.iter().flatten().for_each(Lease::release);
        if close_lanes {
            for record in state.processors.values() {
                for lane in &record.lanes {
                    self.close_lane(lane.device_set, lane.stream);
                }
            }
        }
        {
            let mut inner = self.lock();
            for device in inner.device_sets.values_mut() {
                device.held.retain(|_, array| *array != state.spec.node);
            }
            inner.revision += 1;
        }
        Carried {
            processors: std::mem::take(&mut state.processors),
            pose: std::mem::take(&mut state.pose),
        }
    }

    fn announce(&self, device_sets: &BTreeSet<u32>) {
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::Arrays,
        });
        for ds in device_sets {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(*ds),
            });
        }
    }

    pub(crate) fn shutdown_arrays(&self) {
        let _edits = lock(&self.array_edits);
        let nodes: Vec<String> = self.lock().arrays.keys().cloned().collect();
        for node in nodes {
            if let Err(error) = self.drop_array(&node) {
                tracing::warn!(array = %node, %error, "an array did not stop cleanly at shutdown");
            }
        }
    }

    fn arrays_on(&self, ds: u32) -> Vec<String> {
        self.lock()
            .arrays
            .iter()
            .filter(|(_, state)| state.uses(ds))
            .map(|(node, _)| node.clone())
            .collect()
    }
}

impl ArrayControl for Engine {
    fn switch_array_noise(&self, node: &str, device_set: u32, on: bool) -> Result<(), EngineError> {
        Self::switch_array_noise(self, node, device_set, on)
    }

    fn step_array_gain(&self, node: &str, db: f64) -> Result<(), EngineError> {
        Self::step_array_gain(self, node, db)
    }

    fn sync_context(&self, node: &str) -> Result<SyncContext, EngineError> {
        Self::sync_context(self, node)
    }

    fn clock_drift(&self, node: &str, ppm: Option<f64>) -> Result<(), EngineError> {
        Self::clock_drift(self, node, ppm)
    }

    fn rebuild_processors(&self, node: &str) -> Result<(), EngineError> {
        Self::rebuild_processors(self, node)
    }
}
