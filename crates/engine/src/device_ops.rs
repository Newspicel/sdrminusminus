use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex},
    time::Instant,
};

use sdrmm_device::DeviceError;
use sdrmm_wire::{
    AgcSetting, Capabilities, DeviceSetStatus, DeviceSettings, GainValue, ServerEvent, StateScope,
    StreamScope, Tuning,
};

use crate::{
    ChannelMedia, DEFAULT_CENTER_HZ, DeviceSetState, Engine, EngineError, FaultGate,
    RatePatchGuard, RebuildEntry, dc_block, fault_kind, hotplug, ids_of, lock_runtime,
    planning::{plan_center, validate_streams},
    refusal,
    runtime::{CaptureRuntime, DeviceRuntime},
    sample_rate_of, teardown_set,
};

#[derive(Default)]
struct SinkPoll {
    grown: Vec<(u32, u64, u64, bool)>,
    recording: Vec<(u32, String)>,
    audio: Vec<(u32, u32, String)>,
    baseband: Vec<(u32, u32, String)>,
    export: Vec<(u32, String)>,
    history: Vec<(u32, String)>,
    changed: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FrontEnd {
    gains: Vec<GainValue>,
    agc: Option<AgcSetting>,
    streams: Vec<(u32, Vec<GainValue>, Option<AgcSetting>)>,
}

pub(crate) fn front_end(settings: &DeviceSettings) -> FrontEnd {
    FrontEnd {
        gains: settings.gains.clone(),
        agc: settings.agc.clone(),
        streams: settings
            .streams
            .iter()
            .map(|stream| (stream.stream, stream.gains.clone(), stream.agc.clone()))
            .collect(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LaneSetup {
    pub(crate) tuned: (Option<f64>, Option<Tuning>),
    pub(crate) front_end: FrontEnd,
}

pub(crate) fn lane_setup(settings: &DeviceSettings, stream: u32, scope: &StreamScope) -> LaneSetup {
    let lane = settings.for_stream(stream, scope);
    LaneSetup {
        tuned: (lane.center_hz, lane.tuning),
        front_end: front_end(&lane),
    }
}

fn reached(
    settings: &DeviceSettings,
    delta: &DeviceSettings,
    stream: u32,
    scope: &StreamScope,
) -> LaneSetup {
    let mut lane = settings.for_stream(stream, scope);
    lane.merge_from(&delta.for_stream(stream, scope));
    lane_setup(&lane, stream, scope)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Patched {
    pub(crate) rate_changed: bool,
    pub(crate) capabilities_changed: bool,
}

fn take_the_wheel(mut delta: DeviceSettings) -> DeviceSettings {
    if delta.center_hz.is_some() && delta.tuning.is_none() {
        delta.tuning = Some(Tuning::Manual);
    }
    for stream in &mut delta.streams {
        if stream.center_hz.is_some() && stream.tuning.is_none() {
            stream.tuning = Some(Tuning::Manual);
        }
    }
    delta
}

impl Engine {
    pub(crate) fn hotplug_tick(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        self.report_sinks(self.poll_sinks());
        self.read_agc_gains();
        self.probe_bus(known, missing_once, gate, woken)
    }

    fn read_agc_gains(&self) {
        let running: Vec<(u32, bool, Arc<DeviceRuntime>)> = self
            .lock()
            .device_sets
            .iter()
            .filter(|(_, state)| state.status == DeviceSetStatus::Running)
            .map(|(id, state)| (*id, state.runs_agc(), state.runtime.clone()))
            .collect();
        for (ds, agc, runtime) in running {
            let gains = if agc {
                match lock_runtime(&runtime).agc_gains() {
                    Ok(gains) => gains,
                    Err(error) => {
                        tracing::warn!(ds, %error, "reading back the AGC gain failed");
                        Vec::new()
                    }
                }
            } else {
                Vec::new()
            };
            let changed = self.lock().device_sets.get_mut(&ds).is_some_and(|state| {
                let changed = state.agc_gains != gains;
                state.agc_gains = gains;
                changed
            });
            if changed {
                self.emit(ServerEvent::StateChanged {
                    scope: StateScope::DeviceSet(ds),
                });
            }
        }
    }

    fn poll_sinks(&self) -> SinkPoll {
        let mut inner = self.lock();
        let mut grown: Vec<(u32, u64, u64, bool)> = Vec::new();
        let mut rec_faults: Vec<(u32, String)> = Vec::new();
        let mut audio_rec_faults: Vec<(u32, u32, String)> = Vec::new();
        let mut export_faults: Vec<(u32, String)> = Vec::new();
        let mut baseband_faults: Vec<(u32, u32, String)> = Vec::new();
        let mut history_faults: Vec<(u32, String)> = Vec::new();
        let mut changed: Vec<u32> = Vec::new();
        for (id, s) in inner.device_sets.iter_mut() {
            let now = s.overruns_total();
            let delta = now.saturating_sub(s.overruns_seen);
            s.overruns_seen = now;
            let mut dirty = delta > 0;
            let loss = s.loss_since_poll(delta, Instant::now());
            let began = s.loss.is_none() && loss.is_some();
            if loss != s.loss {
                s.loss = loss;
                dirty = true;
            }
            let clipping = s.take_clipping();
            if clipping != s.clipping {
                s.clipping = clipping;
                dirty = true;
            }
            if delta > 0 {
                grown.push((*id, delta, s.take_worst_stall_ms(), began));
            }
            if let Some(rec) = &mut s.recording {
                let samples = rec.shared.samples();
                if samples != rec.samples_seen {
                    rec.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = rec.shared.error()
                    && !rec.error_seen
                {
                    rec.error_seen = true;
                    rec_faults.push((*id, error));
                    dirty = true;
                }
            }
            for (ch, recording) in &mut s.audio_recordings {
                let frames = recording.shared.frames();
                if frames != recording.frames_seen {
                    recording.frames_seen = frames;
                    dirty = true;
                }
                if let Some(error) = recording.shared.error()
                    && !recording.error_seen
                {
                    recording.error_seen = true;
                    audio_rec_faults.push((*id, ch.channel, error));
                    dirty = true;
                }
            }
            for (ch, recording) in &mut s.baseband_recordings {
                let samples = recording.shared.samples();
                if samples != recording.samples_seen {
                    recording.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = recording.shared.error()
                    && !recording.error_seen
                {
                    recording.error_seen = true;
                    baseband_faults.push((*id, *ch, error));
                    dirty = true;
                }
            }
            for (ch, export) in &mut s.channel_exports {
                let clients = export.shared.clients();
                if clients != export.clients_seen {
                    export.clients_seen = clients;
                    dirty = true;
                }
                let samples = export.shared.samples();
                if samples != export.samples_seen {
                    export.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = export.shared.error()
                    && !export.error_seen
                {
                    export.error_seen = true;
                    baseband_faults.push((*id, *ch, error));
                    dirty = true;
                }
            }
            if let Some(history) = &mut s.time_machine {
                let held = history.handle.shared().held();
                if held != history.held_seen {
                    history.held_seen = held;
                    dirty = true;
                }
                if let Some(error) = history.handle.shared().error()
                    && !history.error_seen
                {
                    history.error_seen = true;
                    history_faults.push((*id, error));
                    dirty = true;
                }
                if history.capture.is_some() && !history.handle.shared().capturing() {
                    history.capture = None;
                    dirty = true;
                }
            }
            if let Some(export) = &mut s.network_export {
                let clients = export.shared.clients();
                if clients != export.clients_seen {
                    export.clients_seen = clients;
                    dirty = true;
                }
                let samples = export.shared.samples();
                if samples != export.samples_seen {
                    export.samples_seen = samples;
                    dirty = true;
                }
                if let Some(error) = export.shared.error()
                    && !export.error_seen
                {
                    export.error_seen = true;
                    export_faults.push((*id, error));
                    dirty = true;
                }
            }
            if dirty {
                changed.push(*id);
            }
        }
        if !changed.is_empty() {
            inner.revision += 1;
        }
        SinkPoll {
            grown,
            recording: rec_faults,
            audio: audio_rec_faults,
            baseband: baseband_faults,
            export: export_faults,
            history: history_faults,
            changed,
        }
    }

    fn report_sinks(&self, poll: SinkPoll) {
        for (ds, dropped, stalled_ms, began) in poll.grown {
            if began {
                tracing::warn!(
                    ds,
                    dropped,
                    stalled_ms,
                    "capture loss: reported device gaps, full queues, or stale samples"
                );
            } else {
                tracing::debug!(ds, dropped, stalled_ms, "capture loss continues");
            }
        }
        for (ds, error) in poll.recording {
            tracing::warn!(ds, error = %error, "recording fault");
        }
        for (ds, channel, error) in poll.audio {
            tracing::warn!(ds, channel, error = %error, "audio recording fault");
        }
        for (ds, error) in poll.export {
            tracing::warn!(ds, error = %error, "network export fault");
        }
        for (ds, channel, error) in poll.baseband {
            tracing::warn!(ds, channel, error = %error, "channel baseband sink fault");
        }
        for (ds, error) in poll.history {
            tracing::warn!(ds, error = %error, "time machine fault");
        }
        for ds in poll.changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
    }

    fn probe_bus(
        &self,
        known: &mut Option<Vec<String>>,
        missing_once: &mut HashSet<u32>,
        gate: &mut hotplug::ProbeGate,
        woken: bool,
    ) -> bool {
        let Some(reason) = gate.should_probe(sdrmm_device::usb::fingerprint(), woken) else {
            return false;
        };
        if reason == hotplug::Probe::BusChanged {
            self.lock_discovery().expire();
        }

        let mut ids = ids_of(&self.registry.probe_all());
        if self.wants_a_deeper_look(&ids) {
            ids = ids_of(&self.registry.probe_all_deep());
        }

        let (absent, returned): (HashSet<u32>, Vec<u32>) = {
            let inner = self.lock();
            let absent = inner
                .device_sets
                .iter()
                .filter(|(_, s)| {
                    s.status == DeviceSetStatus::Running && !ids.contains(&s.info.id())
                })
                .map(|(id, _)| *id)
                .collect();
            let returned = inner
                .device_sets
                .iter()
                .filter(|(_, s)| s.status == DeviceSetStatus::Error && ids.contains(&s.info.id()))
                .map(|(id, _)| *id)
                .collect();
            (absent, returned)
        };
        for ds in absent.intersection(missing_once) {
            self.mark_device_fault(
                *ds,
                DeviceError::Io("device disappeared from probe".to_string()),
            );
        }
        *missing_once = absent;
        for ds in returned {
            self.reconnect(ds);
        }

        let changed = known.as_ref().is_some_and(|prev| *prev != ids);
        *known = Some(ids);
        if changed || reason == hotplug::Probe::BusChanged {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Devices,
            });
        }
        changed
    }

    fn wants_a_deeper_look(&self, ids: &[String]) -> bool {
        let inner = self.lock();
        inner.device_sets.values().any(|s| match s.status {
            DeviceSetStatus::Running => !ids.contains(&s.info.id()),
            DeviceSetStatus::Error => true,
            DeviceSetStatus::Idle => false,
        })
    }

    pub(crate) fn reconnect(&self, ds: u32) {
        let stored = {
            let inner = self.lock();
            let Some(state) = inner.device_sets.get(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error {
                return;
            }
            (state.info.id(), state.settings.clone())
        };
        let (device_id, stored_settings) = stored;

        let opened = self
            .registry
            .open(&device_id)
            .and_then(|(info, mut device)| {
                device.apply(&stored_settings.to_hardware())?;
                Ok((info, device))
            });
        let (info, device) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let capabilities = device.capabilities().shifted_by(stored_settings.offset());
        let playback = device.playback();
        let mut settings = stored_settings.clone();
        settings.merge_from(&DeviceSettings::from_hardware(
            device.settings().clone(),
            stored_settings.offset_hz,
        ));
        let blocking = dc_block(&capabilities, &settings);
        let gate = Arc::new(Mutex::new(FaultGate::Pending(None)));
        let fault_tx = self.fault_tx.clone();
        let handler_gate = gate.clone();
        let runtime = match CaptureRuntime::start(device, &settings, blocking, move |err| {
            let mut gate = handler_gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &mut *gate {
                FaultGate::Pending(slot) => *slot = Some(err),
                FaultGate::Armed => {
                    let _ = fault_tx.send((ds, err));
                }
            }
        }) {
            Ok(runtime) => runtime,
            Err(e) => {
                self.note_reconnect_failure(ds, &e.to_string());
                return;
            }
        };
        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let stalls = runtime.stall_counters();
        let clip_meters = runtime.clip_meters();
        let runtime = Arc::new(DeviceRuntime::new(runtime));

        let (old_runtime, rebuilds, early_fault) = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            };
            if state.status != DeviceSetStatus::Error {
                drop(inner);
                lock_runtime(&runtime).stop();
                return;
            }
            let early_fault = match std::mem::replace(
                &mut *gate
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                FaultGate::Armed,
            ) {
                FaultGate::Pending(slot) => slot,
                FaultGate::Armed => None,
            };
            let old_runtime = std::mem::replace(&mut state.runtime, runtime);
            state.cmd_txs = cmd_txs;
            state.overruns = overruns;
            state.overruns_seen = 0;
            state.overruns_polled = None;
            state.loss = None;
            state.stalls = stalls;
            state.clip_meters = clip_meters;
            state.clipping.clear();
            state.info = info;
            state.capabilities = capabilities;
            state.settings = settings;
            state.status = DeviceSetStatus::Running;
            state.error = None;
            state.playback = playback;
            let rebuilds: Vec<RebuildEntry> = state
                .channels
                .iter()
                .filter_map(|c| {
                    state.media.get(&c.id).map(|m| RebuildEntry {
                        id: c.id,
                        stream: c.stream,
                        settings: c.settings.clone(),
                        sinks: m.sinks.clone(),
                    })
                })
                .collect();
            inner.revision += 1;
            (old_runtime, rebuilds, early_fault)
        };
        lock_runtime(&old_runtime).stop();
        drop(old_runtime);

        let mut dead: Vec<ChannelMedia> = Vec::new();
        self.arrays_after_reconnect(ds);
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        if let Some(err) = early_fault {
            tracing::warn!(ds, error = %err, "reconnected capture died immediately");
            self.mark_device_fault(ds, err);
            return;
        }
        tracing::info!(ds, device = %device_id, "device set reconnected after replug");
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }

    fn note_reconnect_failure(&self, ds: u32, reason: &str) {
        let message = format!("device present but not reopenable: {reason}");
        let changed = {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.status != DeviceSetStatus::Error || state.error.as_deref() == Some(&message) {
                false
            } else {
                state.error = Some(message);
                inner.revision += 1;
                true
            }
        };
        if changed {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(ds),
            });
        }
    }

    pub fn create_device_set(&self, device_id: &str) -> Result<u32, EngineError> {
        self.refuse_reopen(device_id)?;
        let (info, device) = self.registry.open(device_id)?;
        self.create_opened_set(info, device)
    }

    pub(crate) fn create_opened_set(
        &self,
        info: sdrmm_wire::DeviceInfo,
        device: Box<dyn sdrmm_device::SdrDevice>,
    ) -> Result<u32, EngineError> {
        if let Err(already) = self.refuse_reopen(&info.id()) {
            drop(device);
            return Err(already);
        }
        let capabilities = device.capabilities().clone();
        let settings = DeviceSettings {
            tuning: Some(Tuning::default()),
            ..device.settings().clone()
        };
        let playback = device.playback();

        let id = {
            let mut inner = self.lock();
            let id = inner.next_ds_id;
            inner.next_ds_id += 1;
            inner.creating.insert(id);
            id
        };
        let fault_tx = self.fault_tx.clone();
        let started = CaptureRuntime::start(
            device,
            &settings,
            dc_block(&capabilities, &settings),
            move |err| {
                let _ = fault_tx.send((id, err));
            },
        );
        let runtime = match started {
            Ok(runtime) => runtime,
            Err(e) => {
                let mut inner = self.lock();
                inner.creating.remove(&id);
                inner.pending_faults.remove(&id);
                return Err(e.into());
            }
        };

        let cmd_txs = runtime.command_senders();
        let overruns = runtime.overruns_counters();
        let stalls = runtime.stall_counters();
        let clip_meters = runtime.clip_meters();
        let faulted = {
            let mut inner = self.lock();
            inner.creating.remove(&id);
            let pending = inner.pending_faults.remove(&id);
            inner.device_sets.insert(
                id,
                DeviceSetState {
                    info,
                    capabilities,
                    settings,
                    status: if pending.is_some() {
                        DeviceSetStatus::Error
                    } else {
                        DeviceSetStatus::Running
                    },
                    channels: Vec::new(),
                    media: HashMap::new(),
                    next_channel_id: 1,
                    error: pending.as_ref().map(ToString::to_string),
                    fault: pending.as_ref().map(fault_kind),
                    refused: None,
                    recording: None,
                    audio_recordings: HashMap::new(),
                    baseband_recordings: HashMap::new(),
                    channel_exports: HashMap::new(),
                    network_export: None,
                    time_machine: None,
                    scanners: HashMap::new(),
                    hunts: HashMap::new(),
                    rate_patches: 0,
                    cmd_txs,
                    overruns,
                    overruns_seen: 0,
                    overruns_polled: None,
                    loss: None,
                    stalls,
                    clip_meters,
                    clipping: Vec::new(),
                    agc_gains: Vec::new(),
                    playback,
                    runtime: Arc::new(DeviceRuntime::new(runtime)),
                    held: BTreeMap::new(),
                    virtual_lanes: BTreeMap::new(),
                },
            );
            inner.revision += 1;
            pending.is_some()
        };
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if faulted {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::DeviceSet(id),
            });
        }
        Ok(id)
    }

    pub fn write_serial(
        &self,
        device_id: &str,
        serial: Option<&str>,
    ) -> Result<String, EngineError> {
        self.refuse_reopen(device_id)?;
        Ok(self.registry.write_serial(device_id, serial)?)
    }

    pub(crate) fn refuse_reopen(&self, device_id: &str) -> Result<(), EngineError> {
        let inner = self.lock();
        match inner
            .device_sets
            .iter()
            .find(|(_, set)| set.info.id() == device_id)
        {
            Some((id, _)) => Err(EngineError::DeviceAlreadyOpen(device_id.to_owned(), *id)),
            None => Ok(()),
        }
    }

    pub fn remove_device_set(&self, ds: u32) -> Result<(), EngineError> {
        let removed = {
            let mut inner = self.lock();
            let removed = inner.device_sets.remove(&ds);
            if removed.is_some() {
                inner.revision += 1;
            }
            removed
        };
        let removed = removed.ok_or(EngineError::DeviceSetNotFound(ds))?;
        self.arrays_lanes_lost(ds);
        let finalized = teardown_set(removed);
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown_arrays();
        let removed: Vec<DeviceSetState> = {
            let mut inner = self.lock();
            if inner.device_sets.is_empty() {
                return;
            }
            inner.revision += 1;
            std::mem::take(&mut inner.device_sets)
                .into_values()
                .collect()
        };
        let mut finalized = false;
        for set in removed {
            finalized |= teardown_set(set);
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::All,
        });
        if finalized {
            self.emit(ServerEvent::StateChanged {
                scope: StateScope::Recordings,
            });
        }
    }

    pub fn patch_device(&self, ds: u32, delta: DeviceSettings) -> Result<(), EngineError> {
        if delta != DeviceSettings::default() {
            let delta = take_the_wheel(delta);
            self.refuse_held(ds, &delta)?;
            self.patch_device_from(ds, delta)?;
        }
        self.settle_tuning(ds);
        Ok(())
    }

    fn refuse_held(&self, ds: u32, delta: &DeviceSettings) -> Result<(), EngineError> {
        let inner = self.lock();
        let Some(state) = inner.device_sets.get(&ds) else {
            return Ok(());
        };
        if state.held.is_empty() {
            return Ok(());
        }
        let mut delta = delta.clone();
        state.settings.carry_offset(&mut delta);
        let mut after = state.settings.clone();
        after.merge_from(&delta);
        let scope = state.capabilities.per_stream;
        match state.held.iter().find(|(stream, _)| {
            let now = lane_setup(&state.settings, **stream, &scope);
            now != lane_setup(&after, **stream, &scope)
                || now != reached(&state.settings, &delta, **stream, &scope)
        }) {
            Some((_, array)) => Err(EngineError::Held {
                array: array.clone(),
            }),
            None => Ok(()),
        }
    }

    fn runtime_of(&self, ds: u32) -> Option<Arc<DeviceRuntime>> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(|state| state.runtime.clone())
    }

    #[must_use]
    pub fn capabilities(&self, ds: u32) -> Option<Capabilities> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(DeviceSetState::hardware_capabilities)
    }

    pub(crate) fn settle_tuning(&self, ds: u32) -> bool {
        let Some(delta) = self.auto_center(ds) else {
            return false;
        };
        match self.patch_device_from(ds, delta) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(ds, error = %e, "auto tuning could not move the radio");
                false
            }
        }
    }

    fn auto_center(&self, ds: u32) -> Option<DeviceSettings> {
        let inner = self.lock();
        let state = inner.device_sets.get(&ds)?;
        if !state.tunes_freely() {
            return None;
        }
        plan_center(&state.capabilities, &state.settings, &state.channels)
    }

    pub(crate) fn patch_device_from(
        &self,
        ds: u32,
        delta: DeviceSettings,
    ) -> Result<(), EngineError> {
        let patched = self.patch_device_quietly(ds, delta)?;
        if patched.rate_changed {
            self.arrays_rate_changed(ds);
        }
        if patched.capabilities_changed {
            self.arrays_capabilities_changed(ds);
        }
        Ok(())
    }

    pub(crate) fn patch_device_quietly(
        &self,
        ds: u32,
        mut delta: DeviceSettings,
    ) -> Result<Patched, EngineError> {
        let serialized = self
            .runtime_of(ds)
            .ok_or(EngineError::DeviceSetNotFound(ds))?;
        let _patching = serialized.patching();
        if serialized.sweeping() {
            return Err(EngineError::Scan(
                "the radio is sweeping in firmware; stop the scan first".to_string(),
            ));
        }
        if delta.rx_streams.is_some() && !self.arrays_on(ds).is_empty() {
            return Err(EngineError::Device(DeviceError::InUse(
                "an Array holds this radio's lanes; remove it first".to_string(),
            )));
        }
        let lanes_before = self.lanes_of(ds);
        let (runtime, hardware, _rate_guard) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            state.settings.carry_offset(&mut delta);
            let (hardware, rate_change) = state.validate_patch(&delta)?;
            let runtime = state.runtime.clone();
            let guard = rate_change.then(|| {
                state.rate_patches += 1;
                RatePatchGuard { engine: self, ds }
            });
            (runtime, hardware, guard)
        };
        let applied = runtime.apply(&hardware);
        self.note_refusal(ds, &hardware, applied.as_ref().err());
        let actual = applied?.map(|actual| DeviceSettings::from_hardware(actual, delta.offset_hz));
        let (settings, blocking, patched, rebuilds) = {
            let mut inner = self.lock();
            let state = inner
                .device_sets
                .get_mut(&ds)
                .ok_or(EngineError::DeviceSetNotFound(ds))?;
            let old_rate = sample_rate_of(&state.settings);
            let locked_by_export = state.network_export.is_some();
            let owner = if locked_by_export {
                Some(("exporting", "stop the export first"))
            } else if state.recording.is_some() {
                Some(("recording", "stop the recording first"))
            } else if state.time_machine.is_some() {
                Some(("holding history", "disarm the time machine first"))
            } else {
                None
            };
            if let Some((owner, remedy)) = owner
                && delta.sample_rate.is_some_and(|r| r != old_rate)
            {
                drop(inner);
                let revert = DeviceSettings {
                    sample_rate: Some(old_rate),
                    ..DeviceSettings::default()
                };
                if let Err(e) = runtime.apply(&revert) {
                    let message = format!(
                        "sample rate is locked while {owner}, and reverting the device to \
                         {old_rate} Hz failed: {e}"
                    );
                    return Err(if locked_by_export {
                        EngineError::NetworkExport(message)
                    } else {
                        EngineError::Recording(message)
                    });
                }
                let message = format!("sample rate is locked while {owner}; {remedy}");
                return Err(if locked_by_export {
                    EngineError::NetworkExport(message)
                } else {
                    EngineError::Recording(message)
                });
            }
            state.settings.merge_from(&delta);
            if let Some(actual) = &actual {
                state.settings.merge_from(actual);
            }
            let export_center = state.network_export.as_ref().map(|export| {
                state
                    .settings
                    .for_stream(export.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(export), Some(center_hz)) = (state.network_export.as_mut(), export_center)
            {
                export.center_hz = center_hz;
            }
            let history_center = state.time_machine.as_ref().map(|history| {
                state
                    .settings
                    .for_stream(history.stream, &state.capabilities.per_stream)
                    .center_hz
                    .unwrap_or(DEFAULT_CENTER_HZ)
                    .round() as i64
            });
            if let (Some(history), Some(center_hz)) = (state.time_machine.as_mut(), history_center)
            {
                history.center_hz = center_hz;
            }
            let rate = sample_rate_of(&state.settings);
            let rebuilds: Vec<RebuildEntry> = if rate == old_rate {
                Vec::new()
            } else {
                state
                    .channels
                    .iter()
                    .filter_map(|c| {
                        state.media.get(&c.id).map(|m| RebuildEntry {
                            id: c.id,
                            stream: c.stream,
                            settings: c.settings.clone(),
                            sinks: m.sinks.clone(),
                        })
                    })
                    .collect()
            };
            let old_capabilities = state.capabilities.clone();
            if let Some(current) = lock_runtime(&state.runtime).capabilities() {
                state.capabilities = current.shifted_by(state.settings.offset());
            }
            let settings = state.settings.clone();
            let blocking = dc_block(&state.capabilities, &settings);
            let patched = Patched {
                rate_changed: rate != old_rate,
                capabilities_changed: state.capabilities != old_capabilities,
            };
            inner.revision += 1;
            (settings, blocking, patched, rebuilds)
        };
        lock_runtime(&runtime).set_meta(&settings, blocking);
        let mut dead: Vec<ChannelMedia> = Vec::new();
        for rebuild in rebuilds {
            self.rebuild_channel(ds, rebuild, &mut dead);
        }
        for handle in dead {
            handle.shutdown();
        }
        if self.lanes_of(ds) != lanes_before
            && let Err(error) = self.restart_lanes(ds)
        {
            self.mark_device_fault(ds, DeviceError::Io(format!("lane restart: {error}")));
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
        Ok(patched)
    }

    fn lanes_of(&self, ds: u32) -> Option<u32> {
        self.lock()
            .device_sets
            .get(&ds)
            .map(|state| state.capabilities.rx_streams)
    }

    fn note_refusal(&self, ds: u32, hardware: &DeviceSettings, error: Option<&DeviceError>) {
        let refused = error.map(|error| refusal::refused(hardware, error));
        {
            let mut inner = self.lock();
            let Some(state) = inner.device_sets.get_mut(&ds) else {
                return;
            };
            if state.refused == refused {
                return;
            }
            state.refused = refused;
            inner.revision += 1;
        }
        self.emit(ServerEvent::StateChanged {
            scope: StateScope::DeviceSet(ds),
        });
    }
}

impl DeviceSetState {
    pub(crate) fn tunes_freely(&self) -> bool {
        self.recording.is_none() && !self.runtime.sweeping() && self.held.is_empty()
    }

    fn validate_patch(
        &self,
        delta: &DeviceSettings,
    ) -> Result<(DeviceSettings, bool), EngineError> {
        let hardware = delta.to_hardware();
        if let Some(lanes) = delta
            .rx_streams
            .filter(|lanes| *lanes != self.capabilities.rx_streams)
        {
            self.validate_lane_change(lanes)?;
        }
        validate_streams(&self.hardware_capabilities(), &hardware)?;
        let rate_change = delta
            .sample_rate
            .is_some_and(|rate| rate != sample_rate_of(&self.settings));
        if rate_change {
            self.validate_rate_change()?;
        }
        Ok((hardware, rate_change))
    }

    fn validate_lane_change(&self, lanes: u32) -> Result<(), EngineError> {
        if !self.capabilities.rx_stream_choices.contains(&lanes) {
            return Err(EngineError::Device(DeviceError::Unsupported(format!(
                "this radio streams {:?} lanes, got {lanes}",
                self.capabilities.rx_stream_choices
            ))));
        }
        let busy = self
            .channels
            .iter()
            .map(|channel| channel.stream)
            .chain(self.recording.as_ref().map(|recording| recording.stream))
            .chain(self.network_export.as_ref().map(|export| export.stream))
            .chain(self.time_machine.as_ref().map(|history| history.stream))
            .filter(|stream| *stream >= lanes)
            .min();
        match busy {
            Some(stream) => Err(EngineError::Device(DeviceError::InUse(format!(
                "iq{} is in use; unwire it first",
                stream + 1
            )))),
            None => Ok(()),
        }
    }

    fn validate_rate_change(&self) -> Result<(), EngineError> {
        if self.network_export.is_some() {
            return Err(EngineError::NetworkExport(
                "sample rate is locked while exporting; stop the export first".to_string(),
            ));
        }
        if self.recording.is_some() {
            return Err(EngineError::Recording(
                "sample rate is locked while recording; stop the recording first".to_string(),
            ));
        }
        if self.time_machine.is_some() {
            return Err(EngineError::Recording(
                "sample rate is locked while the time machine holds history; disarm it first"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
